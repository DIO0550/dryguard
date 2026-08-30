//! ステージを呼ぶ順序。
//!
//! ここが持つのは順序だけで、読み込みは `Location`、切り出しは `Chunk` にある。
//! `syntax` は I/O を持たないので、読んだ結果を渡す形になる
//! （rules/coding.md 禁止事項 / rules/architecture.md「3 ステージのパイプライン」）。

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::classification::signal::{
    CallerDomainOverlap, ImportOverlap, MeasuredCallerDomains, SemanticsUnavailable, Signals,
    StructuralSimilarity, TypeSignatureMatch,
};
use crate::classification::{Classification, classification_of, is_structurally_similar};
use crate::codebase::{CodebaseError, source_of, typescript_paths_of};
use crate::location::Location;
use crate::lsp::{
    Client, ClientError, DocumentError, ServerCommand, Session, SourceDocument, WorkspaceError,
    WorkspaceRoot,
};
use crate::semantics::caller_domain::{CallerDomainsOutcome, caller_domains_outcome_of};
use crate::semantics::resolved_type::{TypeDeclaration, resolved_types_of, type_declarations_of};
use crate::semantics::type_signature::{TypeSignatureOutcome, type_signature_outcome_of};
use crate::source_position::SourcePosition;
use crate::syntax::chunk::{Chunk, ChunkingError, FileChunks};
use crate::syntax::module_distance::ModuleDistance;
use crate::syntax::tree::{Grammar, ParseError, SyntaxTree};
use crate::threshold::Threshold;

/// 比較する 2 箇所から切り出したチャンクと、その元になったファイルの中身。
///
/// **中身まで持つ**のは、Stage 2 が Stage 1 と同じ版をサーバへ見せるため。
/// 読み直すと、間で編集されたときに構造のシグナルと意味のシグナルが別の版から出て、
/// 覚えてある名前の位置が別の識別子を指すことすらある。読む回数が 1 回で済む利点もある。
#[derive(Debug)]
pub struct ChunkPair {
    chunk_a: Chunk,
    source_a: String,
    chunk_b: Chunk,
    source_b: String,
}

impl ChunkPair {
    /// 先に指定されたほうのチャンク。
    pub fn chunk_a(&self) -> &Chunk {
        &self.chunk_a
    }

    /// 後に指定されたほうのチャンク。
    pub fn chunk_b(&self) -> &Chunk {
        &self.chunk_b
    }
}

/// 比較する 2 箇所から、チャンクの組を取り出す。
///
/// **同じファイルの 2 箇所なら 1 回しか読まない。** 2 回読むと、間で編集されたときに
/// 1 つのファイルに 2 つの版ができる。`lsp::Session::open_document` は URI で
/// 重複を畳むのでサーバが見るのは先の版だけになり、**後のチャンクの名前の位置だけが
/// 別の版から来る**。
///
/// 綴りが違って同じファイルを指す場合（`./a.ts` と `a.ts`）は 2 回読む。パスを絶対に
/// 直すのは `lsp::uri` の担当で、そこへ寄せると `pipeline` がパスの解決を持つことになる。
///
/// # Errors
///
/// どちらかのファイルが読めない / どちらかのチャンクを切り出せないとき。
/// どちらの位置で失敗したかはエラーが持つ。
pub fn chunk_pair_of(
    location_a: &Location,
    location_b: &Location,
) -> Result<ChunkPair, ChunkPairError> {
    let in_one_file = location_a.path() == location_b.path();
    if in_one_file {
        return chunk_pair_in_one_file_of(location_a, location_b);
    }

    let (chunk_a, source_a) = chunk_at(location_a)?;
    let (chunk_b, source_b) = chunk_at(location_b)?;

    Ok(ChunkPair {
        chunk_a,
        source_a,
        chunk_b,
        source_b,
    })
}

/// 1 つのファイルを読んで、その中の 2 箇所を切り出す。
///
/// # Errors
///
/// ファイルを読めない / どちらかのチャンクを切り出せないとき。
fn chunk_pair_in_one_file_of(
    location_a: &Location,
    location_b: &Location,
) -> Result<ChunkPair, ChunkPairError> {
    let source = read_source_at(location_a)?;
    let tree = syntax_tree_at(location_a, &source)?;

    Ok(ChunkPair {
        chunk_a: enclosing_chunk_at(location_a, &tree)?,
        source_a: source.clone(),
        chunk_b: enclosing_chunk_at(location_b, &tree)?,
        source_b: source,
    })
}

/// その位置のファイルを読んで、指定行を含む関数と、読んだ中身を返す。
///
/// パースはファイルにつき 1 回にする。チャンクの範囲も import の集合も同じ木から
/// 採るので、ソースを渡す形のままだと 1 箇所につき 2 回パースすることになる。
fn chunk_at(location: &Location) -> Result<(Chunk, String), ChunkPairError> {
    let source = read_source_at(location)?;
    let chunk = {
        let tree = syntax_tree_at(location, &source)?;

        enclosing_chunk_at(location, &tree)?
    };

    Ok((chunk, source))
}

/// その位置のファイルを読む。読める拡張子でなければ、そこで断る。
///
/// # Errors
///
/// 拡張子から grammar を選べない / ファイルを読めないとき。
fn read_source_at(location: &Location) -> Result<String, ChunkPairError> {
    // grammar を先に確かめる。読めない拡張子のファイルを読んでから断ると、
    // 読み込みの失敗と拡張子の失敗がどちらも「読めない」として並ぶ。
    Grammar::of_path(location.path()).ok_or_else(|| ChunkPairError::UnreadableExtension {
        location: location.clone(),
    })?;

    location
        .read_source()
        .map_err(|cause| ChunkPairError::SourceUnreadable {
            location: location.clone(),
            cause,
        })
}

/// 読んだ中身を、その位置の拡張子が決める grammar で構文木にする。
///
/// # Errors
///
/// 拡張子から grammar を選べない / 構文木にできないとき。
fn syntax_tree_at<'a>(
    location: &Location,
    source: &'a str,
) -> Result<SyntaxTree<'a>, ChunkPairError> {
    let grammar =
        Grammar::of_path(location.path()).ok_or_else(|| ChunkPairError::UnreadableExtension {
            location: location.clone(),
        })?;

    SyntaxTree::from_source(source, grammar).map_err(|cause| ChunkPairError::SourceUnparsable {
        location: location.clone(),
        cause,
    })
}

/// 構文木から、その位置を含む関数を切り出す。
///
/// # Errors
///
/// 指定位置を含む関数が無い / その関数に構文エラーがあるとき。
fn enclosing_chunk_at(location: &Location, tree: &SyntaxTree<'_>) -> Result<Chunk, ChunkPairError> {
    Chunk::find_enclosing(location, tree).map_err(|cause| ChunkPairError::ChunkingFailed {
        location: location.clone(),
        cause,
    })
}

/// チャンクの組から、判定の材料になるシグナルを測る。
///
/// 測れなかったシグナルは、その理由を持つバリアントで返る（値は埋めない）。
/// **測るのはここ、判定は `classification`** という分担にする
/// (`rules/architecture.md`「3 ステージのパイプライン」)。
pub fn signals_of(chunk_a: &Chunk, chunk_b: &Chunk) -> Signals {
    Signals::new(
        structural_similarity_of(chunk_a, chunk_b),
        import_overlap_of(chunk_a, chunk_b),
        ModuleDistance::between(chunk_a.path(), chunk_b.path()),
    )
}

/// 正規化トークン列の似かた。どちらかにトークンが無ければ測れない。
fn structural_similarity_of(chunk_a: &Chunk, chunk_b: &Chunk) -> StructuralSimilarity {
    let (Some(tokens_a), Some(tokens_b)) = (chunk_a.tokens(), chunk_b.tokens()) else {
        return StructuralSimilarity::NoTokens;
    };

    StructuralSimilarity::Measured(tokens_a.similarity_with(tokens_b))
}

/// 依存先集合の Jaccard 係数。どちらかのファイルに import が無ければ測れない。
fn import_overlap_of(chunk_a: &Chunk, chunk_b: &Chunk) -> ImportOverlap {
    let (Some(imports_a), Some(imports_b)) = (chunk_a.imports(), chunk_b.imports()) else {
        return ImportOverlap::NoImports;
    };

    ImportOverlap::Measured(imports_a.jaccard(imports_b))
}

/// 候補ペアについて、Stage 1 と Stage 2 の両方を測った結果。
///
/// Stage 2 へ届かなかった理由を一緒に持つ。シグナルの側は
/// [`SemanticsUnavailable`] のどれかとしか言えないが、**利用者が環境を直すには、
/// サーバが見つからないのか握手に失敗したのかまで要る**
/// (`rules/coding.md`「失敗を握りつぶして既定値へフォールバックしない」)。
#[derive(Debug)]
pub struct MeasuredPair {
    signals: Signals,
    semantics_error: Option<SemanticsError>,
}

impl MeasuredPair {
    /// 判定に渡すシグナル。
    pub fn signals(&self) -> &Signals {
        &self.signals
    }

    /// Stage 2 を尋ねられなかった理由。尋ねられた / そもそも尋ねなかったときは `None`。
    pub fn semantics_error(&self) -> Option<&SemanticsError> {
        self.semantics_error.as_ref()
    }
}

/// ペアを Stage 1 で測り、候補ペアなら Stage 2 を LSP に尋ねて重ねる。
///
/// `structural_similarity_threshold` は候補ペアと見なす構造類似度の下限、
/// `server` は起こす LSP サーバの指定（TypeScript なら [`ServerCommand::typescript`]）。
///
/// **候補ペアでなければサーバを起こさない。** 構造が似ていないペアの判定は Stage 2 で
/// 変わらないので、起動とインデックスの待ち時間だけが増える
/// （`docs/dryguard-plan.md`「候補ペアに対してだけ問い合わせる」）。
/// 尋ねなかったことは [`SemanticsUnavailable::NotACandidate`] として出る。
///
/// **サーバを使えなくても失敗にしない。** Stage 1 のシグナルだけで判定でき、
/// 届かなかったことはシグナルと [`MeasuredPair::semantics_error`] に出る
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
pub fn measured_pair_of(
    pair: &ChunkPair,
    structural_similarity_threshold: Threshold,
    server: &ServerCommand,
) -> MeasuredPair {
    let signals = signals_of(&pair.chunk_a, &pair.chunk_b);

    let asked = if is_structurally_similar(
        signals.structural_similarity(),
        structural_similarity_threshold,
    ) {
        semantics_of(pair, server)
    } else {
        AskedSemantics::unavailable(SemanticsUnavailable::NotACandidate, None)
    };

    MeasuredPair {
        signals: signals.with_semantics(asked.type_signature_match, asked.caller_domain_overlap),
        semantics_error: asked.error,
    }
}

/// Stage 2 に尋ねた結果。
///
/// **途中で落ちても、そこまでに取れたシグナルは捨てない。** 捨てると、
/// 測れていた型シグネチャが「測れない」に化けて判定が変わる（ドメインが一致していて
/// 単一化できないペアが `REVIEW` から `EXTRACT-CANDIDATE` へ動く）。
///
/// 落ちた理由も一緒に持つ。シグナルの側は [`SemanticsUnavailable`] としか言えないが、
/// 利用者が環境を直すにはそれだけでは足りない。
#[derive(Debug)]
struct AskedSemantics {
    type_signature_match: TypeSignatureMatch,
    caller_domain_overlap: CallerDomainOverlap,
    error: Option<SemanticsError>,
}

impl AskedSemantics {
    /// どちらのシグナルも取れていない形。
    ///
    /// **尋ねる前に止まったときだけこれになる。** そこまでは片方だけ取れることが
    /// ないので、2 つに別々の理由を持たせない。
    fn unavailable(reason: SemanticsUnavailable, error: Option<SemanticsError>) -> Self {
        Self {
            type_signature_match: TypeSignatureMatch::Unavailable { reason },
            caller_domain_overlap: CallerDomainOverlap::Unavailable { reason },
            error,
        }
    }

    /// 尋ねる前に止まった失敗から組み立てる。
    fn from_setup_failure(cause: SemanticsError) -> Self {
        Self::unavailable(unavailable_of(&cause), Some(cause))
    }
}

/// 失敗を、シグナルが持てる理由に直す。
///
/// **すべてを「LSP サーバを使えない」に畳まない。** 根を決められないのと
/// サーバが入っていないのとでは**利用者が直す先が違う**ので、
/// [`SemanticsError`] のバリアントを 1 対 1 で写す
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
fn unavailable_of(cause: &SemanticsError) -> SemanticsUnavailable {
    match cause {
        SemanticsError::Document(_) => SemanticsUnavailable::DocumentUnopenable,
        SemanticsError::Workspace(_) => SemanticsUnavailable::WorkspaceRootUndecidable,
        SemanticsError::Client(_) => SemanticsUnavailable::LspUnusable,
    }
}

/// サーバを起こし、2 つのチャンクぶんの Stage 2 のシグナルを尋ねて終わらせる。
///
/// 根は**候補ペアの 2 ファイルだけ**から決める。広げると、開かせないファイルまで
/// 含む位置をサーバに見せることになる（`docs/dryguard-plan.md`「Stage 2: 意味情報収集」）。
/// 根が tsconfig.json より下に来るコードベースで参照元が一部しか返らない話は Issue #125。
///
/// 尋ねる前に止まったときも失敗にしない。取れなかったことを持つ [`AskedSemantics`] を返す。
fn semantics_of(pair: &ChunkPair, server: &ServerCommand) -> AskedSemantics {
    let document_a = match document_of(&pair.chunk_a, &pair.source_a) {
        Ok(document) => document,
        Err(cause) => return AskedSemantics::from_setup_failure(cause),
    };
    let document_b = match document_of(&pair.chunk_b, &pair.source_b) {
        Ok(document) => document,
        Err(cause) => return AskedSemantics::from_setup_failure(cause),
    };
    let root = match WorkspaceRoot::enclosing(&[
        pair.chunk_a.path().to_path_buf(),
        pair.chunk_b.path().to_path_buf(),
    ]) {
        Ok(root) => root,
        Err(cause) => {
            return AskedSemantics::from_setup_failure(SemanticsError::Workspace(cause));
        }
    };

    let client = match Client::start(server) {
        Ok(client) => client,
        Err(cause) => return AskedSemantics::from_setup_failure(SemanticsError::Client(cause)),
    };
    let mut session = match client.handshake(&root) {
        Ok(session) => session,
        Err(cause) => return AskedSemantics::from_setup_failure(SemanticsError::Client(cause)),
    };

    let asked = asked_semantics_of(
        &mut session,
        &pair.chunk_a,
        &document_a,
        &pair.chunk_b,
        &document_b,
    );

    // **答えを受け取っていても、異常終了したサーバの答えは採らない。** 途中で
    // 落ちたサーバは読み込みを終えていないことがあり、参照元が欠けて返る。
    if let Err(cause) = session.shutdown() {
        return AskedSemantics::from_setup_failure(SemanticsError::Client(cause));
    }

    asked
}

/// 開かせた 2 つのドキュメントに、hover と references を尋ねる。
///
/// hover を先に送るのは、**references が先の作業の落ち着きを待つ**ため
/// （順序を入れ替えると、読み込み中に計算された答えを受け取る。
/// `tests/semantics.rs` の `test_caller_domains_asked_after_a_type_signature_are_still_complete`）。
/// 型名の解決も hover と同じ側に置くので、references は最後のまま。
///
/// **1 つが落ちても残りを尋ねる。** 途中で降りると、取れていたシグナルまで
/// 「測れない」に化ける（[`AskedSemantics`]）。
fn asked_semantics_of(
    session: &mut Session,
    chunk_a: &Chunk,
    document_a: &SourceDocument,
    chunk_b: &Chunk,
    document_b: &SourceDocument,
) -> AskedSemantics {
    // 開かせられなければ 1 つも尋ねられないので、ここは降りてよい。
    if let Err(cause) = session.open_document(document_a) {
        return AskedSemantics::from_setup_failure(SemanticsError::Client(cause));
    }
    if let Err(cause) = session.open_document(document_b) {
        return AskedSemantics::from_setup_failure(SemanticsError::Client(cause));
    }

    let (Some(position_a), Some(position_b)) = (chunk_a.name_position(), chunk_b.name_position())
    else {
        // 名前が無ければ尋ねる位置が決まらない。**サーバの失敗ではない**ので、
        // 取れなかったシグナルとして返す
        return AskedSemantics {
            type_signature_match: TypeSignatureMatch::NoName,
            caller_domain_overlap: CallerDomainOverlap::NoName,
            error: None,
        };
    };

    asked_semantics_of_outcomes(
        resolved_type_signature_outcome_of(session, chunk_a, document_a, position_a),
        resolved_type_signature_outcome_of(session, chunk_b, document_b, position_b),
        caller_domains_outcome_of(session, document_a, position_a),
        caller_domains_outcome_of(session, document_b, position_b),
    )
}

/// そのチャンクの型シグネチャを、**書かれた型名を解決してから**尋ねる。
///
/// hover が返す綴りは書かれた型名のままで、型エイリアスは展開されない。
/// 先に型名の宣言を辿って右辺を集め、綴りへ差し込んでから読む
/// （`semantics::resolved_type`）。
///
/// # Errors
///
/// 往復が失敗したとき。
fn resolved_type_signature_outcome_of(
    session: &mut Session,
    chunk: &Chunk,
    document: &SourceDocument,
    position: SourcePosition,
) -> Result<TypeSignatureOutcome, ClientError> {
    let declarations = type_declarations_of(session, document, chunk.type_references())?;
    open_declaring_documents(session, &declarations)?;
    let resolved = resolved_types_of(session, &declarations)?;

    type_signature_outcome_of(session, document, position, &resolved)
}

/// 型が宣言されているファイルを開かせる。
///
/// **開かせないと宣言の綴りが返らない。** サーバは開かせていないファイルへの hover に
/// 綴りを持たない応答を返す（typescript-language-server 6.0.0 で実測）。
///
/// **どこで宣言されていても開かせる。** 依存の置き場で宣言されたエイリアス
/// （`lib.es5.d.ts` の `PropertyKey` など）も、書き下した綴りと比べる側から見れば
/// 開く価値は同じ。**開く相手を絞ると、絞った分だけ偽陰性が残る。**
///
/// **Why not（大きいファイルを避けて絞る）**: `Date` が解決される `lib.es5.d.ts` は
/// 約 1 MB あるが、**開かせても `compare` の実測は 1.120 秒で、開かせない場合と
/// 変わらなかった**。避ける理由になるコストが出ていない。
///
/// **Why not（ワークスペースの根の下だけを開かせる）**: 根は候補ペアの 2 ファイルから
/// 決まるので、**同じディレクトリにある 2 つを比べると根がそのディレクトリになる**。
/// 兄弟ディレクトリで宣言されたエイリアスが解決できず、この Issue が直したかった
/// 偽陰性がそのまま残る。
///
/// 読めなかったファイルは飛ばす。**その型名が解決されないまま残るだけ**で、
/// 比較は綴りのまま続く（今までと同じ形）。
///
/// # Errors
///
/// 開かせる要求の送信が失敗したとき。
fn open_declaring_documents(
    session: &mut Session,
    declarations: &[TypeDeclaration],
) -> Result<(), ClientError> {
    for declaration in declarations {
        let path = declaration.site().path();

        let Ok(text) = source_of(path) else {
            continue;
        };
        let Ok(document) = SourceDocument::new(path, text) else {
            continue;
        };

        session.open_document(&document)?;
    }

    Ok(())
}

/// 4 つの問い合わせの結果を、シグナルと落ちた理由にまとめる。
///
/// **落ちた側だけを「測れない」にする。** hover が答えていて references だけが
/// 落ちた場合に両方を捨てると、比べ終わっていた型シグネチャまで失われる。
fn asked_semantics_of_outcomes(
    signature_a: Result<TypeSignatureOutcome, ClientError>,
    signature_b: Result<TypeSignatureOutcome, ClientError>,
    callers_a: Result<CallerDomainsOutcome, ClientError>,
    callers_b: Result<CallerDomainsOutcome, ClientError>,
) -> AskedSemantics {
    let type_signature_match = asked_type_signature_match_of(&signature_a, &signature_b);
    let caller_domain_overlap = asked_caller_domain_overlap_of(&callers_a, &callers_b);

    AskedSemantics {
        type_signature_match,
        caller_domain_overlap,
        // 先に落ちたほうを出す。後の失敗で上書きすると、何が起きたのかが入れ替わる。
        error: signature_a
            .err()
            .or_else(|| signature_b.err())
            .or_else(|| callers_a.err())
            .or_else(|| callers_b.err())
            .map(SemanticsError::Client),
    }
}

/// 型シグネチャを尋ねた結果から、単一化できるかのシグナルにする。
fn asked_type_signature_match_of(
    signature_a: &Result<TypeSignatureOutcome, ClientError>,
    signature_b: &Result<TypeSignatureOutcome, ClientError>,
) -> TypeSignatureMatch {
    let (Ok(signature_a), Ok(signature_b)) = (signature_a, signature_b) else {
        return TypeSignatureMatch::Unavailable {
            reason: SemanticsUnavailable::LspUnusable,
        };
    };

    type_signature_match_of(signature_a, signature_b)
}

/// 参照元を尋ねた結果から、呼び出し元ドメインの重なりのシグナルにする。
fn asked_caller_domain_overlap_of(
    callers_a: &Result<CallerDomainsOutcome, ClientError>,
    callers_b: &Result<CallerDomainsOutcome, ClientError>,
) -> CallerDomainOverlap {
    let (Ok(callers_a), Ok(callers_b)) = (callers_a, callers_b) else {
        return CallerDomainOverlap::Unavailable {
            reason: SemanticsUnavailable::LspUnusable,
        };
    };

    caller_domain_overlap_of(callers_a, callers_b)
}

/// そのチャンクのファイルを、サーバに開かせる形にする。
///
/// `source` は**そのチャンクを切り出したときに読んだ中身**（[`ChunkPair`] が持つ）。
/// チャンクの範囲だけでは足りない。**サーバは位置を行と列で受け取る**ので、
/// ファイル全体と同じ中身を見せていないと別の場所を指すことになる。
///
/// # Errors
///
/// 開かせる形にできないとき。
fn document_of(chunk: &Chunk, source: &str) -> Result<SourceDocument, SemanticsError> {
    SourceDocument::new(chunk.path(), source.to_owned()).map_err(SemanticsError::Document)
}

/// 両側の型シグネチャから、単一化できるかのシグナルにする。
///
/// **片方でも正規化できていなければ「単一化不能」にしない。** 比べていないので、
/// 取れなかった理由をそのまま出す（2 つとも取れていなければ、上の枝の理由）。
fn type_signature_match_of(
    signature_a: &TypeSignatureOutcome,
    signature_b: &TypeSignatureOutcome,
) -> TypeSignatureMatch {
    match (signature_a, signature_b) {
        (
            TypeSignatureOutcome::Normalized(normalized_a),
            TypeSignatureOutcome::Normalized(normalized_b),
        ) => unifiable_match_of(normalized_a.is_unifiable_with(normalized_b)),
        (TypeSignatureOutcome::NoTypeThere, _) | (_, TypeSignatureOutcome::NoTypeThere) => {
            TypeSignatureMatch::NoTypeThere
        }
        (TypeSignatureOutcome::UnreadableHover, _) | (_, TypeSignatureOutcome::UnreadableHover) => {
            TypeSignatureMatch::UnreadableHover
        }
        (TypeSignatureOutcome::UnreadableSignature, _)
        | (_, TypeSignatureOutcome::UnreadableSignature) => TypeSignatureMatch::UnreadableSignature,
        (TypeSignatureOutcome::HoverNotProvided, _)
        | (_, TypeSignatureOutcome::HoverNotProvided) => TypeSignatureMatch::HoverNotProvided,
    }
}

/// 単一化できたかどうかを、シグナルのバリアントにする。
fn unifiable_match_of(unifiable: bool) -> TypeSignatureMatch {
    if unifiable {
        return TypeSignatureMatch::Unifiable;
    }
    TypeSignatureMatch::NotUnifiable
}

/// 両側の呼び出し元から、ドメインの重なりのシグナルにする。
///
/// **片方でも取れていなければ 0.00 にしない。** 重なりが無いのと材料が無いのは
/// 別の話で、同じ値にすると判定も読者も区別できない。
fn caller_domain_overlap_of(
    callers_a: &CallerDomainsOutcome,
    callers_b: &CallerDomainsOutcome,
) -> CallerDomainOverlap {
    match (callers_a, callers_b) {
        (CallerDomainsOutcome::Counted(counted_a), CallerDomainsOutcome::Counted(counted_b)) => {
            CallerDomainOverlap::Measured(MeasuredCallerDomains::new(
                counted_a.clone(),
                counted_b.clone(),
            ))
        }
        (CallerDomainsOutcome::NoReferences, _) | (_, CallerDomainsOutcome::NoReferences) => {
            CallerDomainOverlap::NoReferences
        }
        (CallerDomainsOutcome::UnreadableReferences, _)
        | (_, CallerDomainsOutcome::UnreadableReferences) => {
            CallerDomainOverlap::UnreadableReferences
        }
        (CallerDomainsOutcome::ServerStillWorking, _)
        | (_, CallerDomainsOutcome::ServerStillWorking) => CallerDomainOverlap::ServerStillWorking,
        (CallerDomainsOutcome::ReferencesNotProvided, _)
        | (_, CallerDomainsOutcome::ReferencesNotProvided) => {
            CallerDomainOverlap::ReferencesNotProvided
        }
    }
}

/// Stage 2 を尋ねられなかった理由。
///
/// **1 つにまとめない。** サーバが入っていないのと、根を決められないのとで
/// **利用者が直す先が違う**
/// (`rules/coding.md`「エラー型は原因ごとにバリアントを分ける」)。
/// `classification::signal::SemanticsUnavailable` のバリアントと 1 対 1 で対応する。
#[derive(Debug)]
pub enum SemanticsError {
    /// サーバに開かせる形にできなかった。
    Document(DocumentError),
    /// ワークスペースの根を決められなかった。
    Workspace(WorkspaceError),
    /// サーバとのやりとりが失敗した。
    Client(ClientError),
}

impl fmt::Display for SemanticsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(cause) => write!(formatter, "{cause}"),
            Self::Workspace(cause) => write!(formatter, "{cause}"),
            Self::Client(cause) => write!(formatter, "{cause}"),
        }
    }
}

impl Error for SemanticsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Document(cause) => Some(cause),
            Self::Workspace(cause) => Some(cause),
            Self::Client(cause) => Some(cause),
        }
    }
}

/// コードベース全体を走査して、候補ペアを判定する。
///
/// `root` は走査を始めるディレクトリ、`structural_similarity_threshold` は
/// 候補ペアとして拾う構造類似度の下限。
///
/// 読めなかったファイルと切り出せなかった関数は、走査を止めずに結果へ残す。
/// **1 ファイルのために全体を落とすと、他のペアの判定まで失われる**。
///
/// # Errors
///
/// `root` がディレクトリでない / 途中のディレクトリを読めないとき。
pub fn scan_of(
    root: &Path,
    structural_similarity_threshold: Threshold,
) -> Result<Scan, CodebaseError> {
    let paths = typescript_paths_of(root)?;
    let file_count = paths.len();

    // ファイルごとの読み込み・パース・切り出しは互いに独立なので並列に回す。
    // 結果は `paths` と同じ並びで返るので、まとめ直しを順に行えば出力の並びは
    // 逐次で回したときと変わらない
    let chunked_files: Vec<Result<FileChunks, SkippedFile>> =
        paths.par_iter().map(|path| file_chunks_of(path)).collect();

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut skipped_files = Vec::new();
    let mut unchunkable = Vec::new();

    for (path, chunked_file) in paths.iter().zip(chunked_files) {
        let file_chunks = match chunked_file {
            Ok(file_chunks) => file_chunks,
            Err(skipped) => {
                skipped_files.push(skipped);
                continue;
            }
        };

        unchunkable.extend(
            file_chunks
                .unparsable_starts()
                .iter()
                .map(|start| Location::new(path.clone(), *start)),
        );
        chunks.extend(file_chunks.chunks().iter().cloned());
    }

    Ok(scan_of_chunks(
        &chunks,
        structural_similarity_threshold,
        ScanInputs {
            file_count,
            skipped_files,
            unchunkable,
        },
    ))
}

/// そのファイルを読んで、中にある関数・メソッドをすべて切り出す。
///
/// `path` は [`typescript_paths_of`] が集めたファイル。
///
/// # Errors
///
/// 拡張子から grammar を選べない / ファイルを読めない / 構文木にできないとき。
/// **飛ばす理由をそのまま返す**ので、呼び出し側は 1 ファイルのために走査を止めずに済む。
fn file_chunks_of(path: &Path) -> Result<FileChunks, SkippedFile> {
    let grammar = Grammar::of_path(path).ok_or_else(|| SkippedFile::UnreadableExtension {
        path: path.to_path_buf(),
    })?;

    let source = source_of(path).map_err(|cause| SkippedFile::SourceUnreadable {
        path: path.to_path_buf(),
        cause,
    })?;

    let tree = SyntaxTree::from_source(&source, grammar).map_err(|cause| {
        SkippedFile::SourceUnparsable {
            path: path.to_path_buf(),
            cause,
        }
    })?;

    Ok(FileChunks::from_tree(&tree, path))
}

/// 走査で集めた、チャンク以外の材料。
///
/// [`scan_of_chunks`] の引数をまとめるためだけの型なので公開しない。
struct ScanInputs {
    file_count: usize,
    skipped_files: Vec<SkippedFile>,
    unchunkable: Vec<Location>,
}

/// 集めたチャンクを総当たりで比べ、候補ペアだけを判定する。
///
/// チャンク 1 つ分（自分より後ろの全チャンクとの比較）を単位に並列で回す。
/// **先頭のチャンクほど比べる相手が多い**ので、区間を等分せず rayon の作業盗みに任せる。
///
/// 結果はチャンクの並び順で返るので、候補ペアの並びは逐次で回したときと変わらない。
fn scan_of_chunks(
    chunks: &[Chunk],
    structural_similarity_threshold: Threshold,
    inputs: ScanInputs,
) -> Scan {
    let compared: Vec<ComparedPairs> = chunks
        .par_iter()
        .enumerate()
        .map(|(index, chunk)| {
            ComparedPairs::from_chunk(chunk, &chunks[index + 1..], structural_similarity_threshold)
        })
        .collect();

    let compared_pair_count = compared
        .iter()
        .map(|compared| compared.compared_pair_count)
        .sum();
    let pruned_pair_count = compared
        .iter()
        .map(|compared| compared.pruned_pair_count)
        .sum();
    let candidate_pairs = compared
        .into_iter()
        .flat_map(|compared| compared.candidate_pairs)
        .collect();

    Scan {
        candidate_pairs,
        file_count: inputs.file_count,
        chunk_count: chunks.len(),
        compared_pair_count,
        pruned_pair_count,
        skipped_files: inputs.skipped_files,
        unchunkable: inputs.unchunkable,
    }
}

/// 1 つのチャンクを、それより後ろのチャンクすべてと比べた結果。
///
/// 候補ペアと比べた数を一緒に持つ。**比べた数は候補ペアの一覧から数え直せない**
/// （閾値に届かなかった組も、入れ子で比べなかった組も一覧には出ない）。
struct ComparedPairs {
    candidate_pairs: Vec<CandidatePair>,
    compared_pair_count: usize,
    pruned_pair_count: usize,
}

impl ComparedPairs {
    /// `chunk` を `following`（並びの中でそれより後ろにあるチャンク）すべてと比べる。
    ///
    /// 候補かどうかは [`is_structurally_similar`] に聞く。**同じ条件をここに書き直すと、
    /// 判定が「似ている」と見なす範囲と候補に拾う範囲が黙ってずれる**
    /// (`rules/architecture.md`「判定は 1 箇所にだけ置く」)。
    fn from_chunk(
        chunk: &Chunk,
        following: &[Chunk],
        structural_similarity_threshold: Threshold,
    ) -> Self {
        let mut candidate_pairs = Vec::new();
        let mut compared_pair_count = 0;
        let mut pruned_pair_count = 0;

        for other in following {
            if is_nested(chunk, other) {
                continue;
            }
            compared_pair_count += 1;

            if is_ruled_out_by_ceiling(chunk, other, structural_similarity_threshold) {
                pruned_pair_count += 1;
                continue;
            }

            let signals = signals_of(chunk, other);
            if !is_structurally_similar(
                signals.structural_similarity(),
                structural_similarity_threshold,
            ) {
                continue;
            }

            candidate_pairs.push(CandidatePair {
                location_a: start_of(chunk),
                location_b: start_of(other),
                classification: classification_of(&signals, structural_similarity_threshold),
            });
        }

        Self {
            candidate_pairs,
            compared_pair_count,
            pruned_pair_count,
        }
    }
}

/// 突き合わせるまでもなく、構造類似度が閾値に届かないと分かるペアか。
///
/// トークン列の長さから出る上限を [`is_structurally_similar`] に渡して聞く。
/// **同じ条件をここに書き直さない**のは [`ComparedPairs::from_chunk`] と同じ理由で、
/// 判定が「似ている」と見なす範囲と飛ばす範囲が黙ってずれるため。
///
/// **これは判定の先取りではない。** 上限は実際の類似度を下回らないので
/// (`TokenSequence::similarity_ceiling_with`)、飛ばしたペアが候補になることはない。
/// 飛ばす / 飛ばさないで候補ペアが変わらないことが、この関数を置ける条件そのもの。
///
/// どちらかにトークンが無いときは飛ばさない。**それは長さで確定したのではなく
/// 測れないケース**で、`NoTokens` として `signals_of` が構造に出す。
fn is_ruled_out_by_ceiling(chunk_a: &Chunk, chunk_b: &Chunk, threshold: Threshold) -> bool {
    let (Some(tokens_a), Some(tokens_b)) = (chunk_a.tokens(), chunk_b.tokens()) else {
        return false;
    };

    let ceiling = StructuralSimilarity::Measured(tokens_a.similarity_ceiling_with(tokens_b));

    !is_structurally_similar(ceiling, threshold)
}

/// 同じファイルにあって範囲が重なる 2 つのチャンクか（外側の関数とその中の関数）。
///
/// 入れ子の組は同じコードを二重に数えているだけなので比べない。**別のファイルに
/// 同じ形があれば見つけたい**ので、内側のチャンク自体は落とさない。
fn is_nested(chunk_a: &Chunk, chunk_b: &Chunk) -> bool {
    let same_file = chunk_a.path() == chunk_b.path();

    same_file && chunk_a.lines().overlaps(chunk_b.lines())
}

/// そのチャンクの始まりを指す位置。`compare` にそのまま渡せる形。
fn start_of(chunk: &Chunk) -> Location {
    Location::new(chunk.path().to_path_buf(), chunk.lines().start())
}

/// コードベース全体を走査した結果。
///
/// 候補ペアだけでなく、走査した量と飛ばしたものも持つ。**出た候補の数だけでは
/// 「見ていないもの」が分からない**（`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」）。
#[derive(Debug)]
pub struct Scan {
    candidate_pairs: Vec<CandidatePair>,
    file_count: usize,
    chunk_count: usize,
    compared_pair_count: usize,
    pruned_pair_count: usize,
    skipped_files: Vec<SkippedFile>,
    unchunkable: Vec<Location>,
}

impl Scan {
    /// 構造類似度が閾値に届いたペアと、その判定。列挙した順に並ぶ。
    pub fn candidate_pairs(&self) -> &[CandidatePair] {
        &self.candidate_pairs
    }

    /// 走査の対象になった TypeScript ファイルの数。
    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// 切り出せたチャンクの数。
    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    /// 実際に比べたペアの数。入れ子の組は含まない。
    ///
    /// **長さの上限だけで確定したペアもここに数える**（[`Scan::pruned_pair_count`]）。
    /// 飛ばしたのではなく安く比べただけなので、比べた数からは外さない。
    pub fn compared_pair_count(&self) -> usize {
        self.compared_pair_count
    }

    /// 比べたペアのうち、トークン列の長さから出る上限だけで確定した数。
    ///
    /// [`Scan::compared_pair_count`] の内訳で、残りが gram を突き合わせたペア。
    /// **黙って飛ばさない**のは、どれだけ省いたかが読めないと
    /// 「比べた」の意味が回ごとに変わって見えるため。
    pub fn pruned_pair_count(&self) -> usize {
        self.pruned_pair_count
    }

    /// 読めなかった・構文木にできなかったファイル。
    pub fn skipped_files(&self) -> &[SkippedFile] {
        &self.skipped_files
    }

    /// 構文エラーで切り出せなかった関数の位置。
    pub fn unchunkable(&self) -> &[Location] {
        &self.unchunkable
    }
}

/// 構造類似度が閾値に届いたペアと、その判定。
#[derive(Debug)]
pub struct CandidatePair {
    location_a: Location,
    location_b: Location,
    classification: Classification,
}

impl CandidatePair {
    /// 先に列挙したほうのチャンクの位置。
    pub fn location_a(&self) -> &Location {
        &self.location_a
    }

    /// 後に列挙したほうのチャンクの位置。
    pub fn location_b(&self) -> &Location {
        &self.location_b
    }

    /// このペアの判定と根拠。
    pub fn classification(&self) -> &Classification {
        &self.classification
    }
}

/// 走査の途中で飛ばしたファイルと、その理由。
///
/// どのファイルを飛ばしたかを残すのは、黙って落とすと**対象に入っていたのか
/// 除外されたのか**を読む側が区別できないため。
#[derive(Debug)]
pub enum SkippedFile {
    /// 拡張子から grammar を選べなかった。
    UnreadableExtension { path: PathBuf },
    /// ファイルを読めなかった。
    SourceUnreadable { path: PathBuf, cause: io::Error },
    /// ファイルは読めたが、構文木にできなかった。
    SourceUnparsable { path: PathBuf, cause: ParseError },
}

impl fmt::Display for SkippedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableExtension { path } => {
                write!(formatter, "{}: 読める拡張子ではありません", path.display())
            }
            Self::SourceUnreadable { path, cause } => {
                write!(formatter, "{}: 読めません: {cause}", path.display())
            }
            Self::SourceUnparsable { path, cause } => {
                write!(formatter, "{}: 読み解けません: {cause}", path.display())
            }
        }
    }
}

impl Error for SkippedFile {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnreadableExtension { .. } => None,
            Self::SourceUnreadable { cause, .. } => Some(cause),
            Self::SourceUnparsable { cause, .. } => Some(cause),
        }
    }
}

/// チャンクを取り出せなかった理由。
///
/// どちらの位置で失敗したかを持つ。`compare` は 2 箇所を受け取るので、
/// 位置が分からないと利用者はどちらを直せばよいか分からない。
#[derive(Debug)]
pub enum ChunkPairError {
    /// 拡張子から grammar を選べなかった。
    ///
    /// 読める拡張子でないファイルは、読めたふりをせずここで断る。TypeScript の
    /// grammar で当てても**読めた範囲だけが構造として出る**ので、判定が
    /// 「似ていない」なのか「読めていない」なのか区別できなくなる。
    UnreadableExtension { location: Location },
    /// ファイルを読めなかった。
    SourceUnreadable {
        location: Location,
        cause: io::Error,
    },
    /// ファイルは読めたが、構文木にできなかった。
    SourceUnparsable {
        location: Location,
        cause: ParseError,
    },
    /// 構文木にはできたが、チャンクを切り出せなかった。
    ChunkingFailed {
        location: Location,
        cause: ChunkingError,
    },
}

impl fmt::Display for ChunkPairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableExtension { location } => {
                write!(formatter, "{location} は読める拡張子ではありません")
            }
            Self::SourceUnreadable { location, cause } => {
                write!(formatter, "{location} のファイルを読めません: {cause}")
            }
            Self::SourceUnparsable { location, cause } => {
                write!(formatter, "{location} のファイルを読み解けません: {cause}")
            }
            Self::ChunkingFailed { location, cause } => {
                write!(formatter, "{location} から切り出せません: {cause}")
            }
        }
    }
}

impl Error for ChunkPairError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnreadableExtension { .. } => None,
            Self::SourceUnreadable { cause, .. } => Some(cause),
            Self::SourceUnparsable { cause, .. } => Some(cause),
            Self::ChunkingFailed { cause, .. } => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::classification::DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
    use crate::classification::verdict::Verdict;
    use crate::semantics::resolved_type::ResolvedTypes;
    use crate::semantics::type_signature::TypeSignature;
    use crate::similarity::Similarity;
    use crate::test_support::line;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// 尋ねる前に止まった理由が、シグナルの側でも別々のままかを見る。
    ///
    /// **例外的に内部の関数を直接呼ぶ。** 根を決められない / 開かせる形にできないは
    /// Windows のパスでしか起きない（別ドライブに共通の祖先が無い・UNC を URI に
    /// できない。Issue #112）ので、**この対応付けを外から通せる入力が Linux に無い**。
    /// 対応付けを固定しないまま置くと、すべてが「LSP サーバを使えない」に畳まれても
    /// 気づけない（`rules/testing.md`「assert は『落ちうるか』で見る」）。
    #[test]
    fn test_unavailable_of_keeps_each_setup_failure_apart() {
        let reasons = [
            unavailable_of(&SemanticsError::Workspace(WorkspaceError::NoPaths)),
            unavailable_of(&SemanticsError::Document(
                DocumentError::UnreadableExtension {
                    path: PathBuf::from("notes.md"),
                },
            )),
            unavailable_of(&SemanticsError::Client(ClientError::PipesNotWired)),
        ];

        assert_eq!(
            reasons,
            [
                SemanticsUnavailable::WorkspaceRootUndecidable,
                SemanticsUnavailable::DocumentUnopenable,
                SemanticsUnavailable::LspUnusable,
            ]
        );
    }

    /// 正規化できた型シグネチャ。
    fn normalized(signature_text: &str) -> TypeSignatureOutcome {
        let signature =
            TypeSignature::from_signature_text(signature_text, &ResolvedTypes::default())
                .expect("テストが渡す綴りは読み取れる");

        TypeSignatureOutcome::Normalized(signature)
    }

    /// hover は答えたが references が落ちた、4 つの問い合わせの結果。
    fn references_that_failed() -> AskedSemantics {
        asked_semantics_of_outcomes(
            Ok(normalized("function totalOf(values: number[]): number")),
            Ok(normalized("function sumOf(amounts: number[]): number")),
            Err(ClientError::PipesNotWired),
            Ok(CallerDomainsOutcome::NoReferences),
        )
    }

    #[test]
    fn test_asked_semantics_keep_the_type_signature_when_references_fail() {
        // 両方を捨てると、比べ終わっていた型シグネチャが「測れない」に化ける。
        // ドメインが一致するペアなら、そこで REVIEW が EXTRACT-CANDIDATE へ動く
        let asked = references_that_failed();

        assert_eq!(asked.type_signature_match, TypeSignatureMatch::Unifiable);
    }

    #[test]
    fn test_asked_semantics_mark_only_the_caller_domain_when_references_fail() {
        // 対照は上のテスト。落ちた側だけが取れない扱いになり、理由も残る
        let asked = references_that_failed();

        assert_eq!(
            asked.caller_domain_overlap,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::LspUnusable
            }
        );
        assert!(
            asked.error.is_some(),
            "落ちたことは理由として残る: {:?}",
            asked.error.map(|error| error.to_string())
        );
    }

    /// ファイルを読まずにチャンクを作る。
    ///
    /// `Chunk::find_enclosing` は構文木を引数で受けるので、実ファイルが要るのは
    /// 読み込みまで。ここで見たいのは読み込みの後ろにあるシグナルの測り方なので、
    /// パスは位置の材料としてだけ渡す。
    fn chunk_of(path: &str, number: usize, source: &str) -> Chunk {
        let location = Location::new(PathBuf::from(path), line(number));
        let tree = SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる");

        Chunk::find_enclosing(&location, &tree).expect("テストが渡す位置は関数の中を指している")
    }

    const WITH_IMPORT: &str = "import { pad } from \"./pad\";\n\
                               export function format(value: number): string {\n\
                               \x20 return pad(value);\n\
                               }\n";

    const WITHOUT_IMPORT: &str = "export function format(value: number): string {\n\
                                  \x20 return String(value);\n\
                                  }\n";

    #[test]
    fn test_signals_of_two_chunks_depending_on_the_same_module_measure_a_total_overlap() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/utils/render.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(
            signals.import_overlap(),
            ImportOverlap::Measured(measured(1.0))
        );
    }

    #[test]
    fn test_signals_of_a_chunk_whose_file_has_no_import_cannot_measure_the_overlap() {
        // 対照は上のテスト。同じ測り方で片方だけ import を外している
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/utils/render.ts", 1, WITHOUT_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(signals.import_overlap(), ImportOverlap::NoImports);
    }

    #[test]
    fn test_signals_of_two_identical_chunks_measure_a_total_structural_similarity() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/report/format.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(
            signals.structural_similarity(),
            StructuralSimilarity::Measured(measured(1.0))
        );
    }

    #[test]
    fn test_signals_of_two_chunks_in_sibling_directories_measure_both_steps() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/report/format.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(signals.module_distance().steps(), 2);
    }

    /// `tests/fixtures/` 配下のディレクトリ。
    ///
    /// カレントディレクトリではなくマニフェストの位置から組み立てる
    /// （テストの実行位置に依存させない）。
    fn fixture(relative_path: &str) -> PathBuf {
        PathBuf::from(format!(
            "{}/tests/fixtures/{relative_path}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// 既定の閾値で走査した結果。
    fn scan_of_fixture(relative_path: &str) -> Scan {
        scan_of(
            &fixture(relative_path),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        )
        .expect("フィクスチャのディレクトリは走査できる")
    }

    #[test]
    fn test_scan_of_a_codebase_reports_a_similar_pair_in_separate_domains() {
        let scan = scan_of_fixture("scan");

        let pairs: Vec<String> = scan
            .candidate_pairs()
            .iter()
            .map(|pair| format!("{} <-> {}", pair.location_a(), pair.location_b()))
            .collect();
        assert!(
            pairs
                .iter()
                .any(|pair| pair.contains("billing/discount.ts:3")
                    && pair.contains("inventory/reorder.ts:3")),
            "構造の同じ 2 関数が候補ペアとして出る: {pairs:?}"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_classifies_a_candidate_pair_without_shared_dependencies_as_do_not_extract()
     {
        let scan = scan_of_fixture("scan");

        let verdicts: Vec<Verdict> = scan
            .candidate_pairs()
            .iter()
            .map(|pair| pair.classification().verdict())
            .collect();
        assert!(
            verdicts.contains(&Verdict::DoNotExtract),
            "依存先を共有しない候補ペアに判定が付く: {verdicts:?}"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_does_not_compare_a_function_with_the_one_nested_in_it() {
        // フィクスチャのチャンクは 5 つ（discount / reorder / Badge / makeAdder と
        // その中のアロー）。総当たりなら 10 ペアだが、入れ子の 1 ペアは比較しない
        let scan = scan_of_fixture("scan");

        assert_eq!(scan.chunk_count(), 5, "切り出せたチャンクの数");
        assert_eq!(
            scan.compared_pair_count(),
            9,
            "入れ子の組（makeAdder とその中のアロー）は比べない"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_reads_a_tsx_file_without_losing_its_function() {
        // フィクスチャの src/report/Badge.tsx は JSX を返す。TypeScript の grammar で
        // 読むと関数ごと構文エラーになり、チャンクにならず unchunkable に落ちる
        let scan = scan_of_fixture("scan");

        let unchunkable: Vec<String> = scan.unchunkable().iter().map(Location::to_string).collect();
        assert!(
            unchunkable.is_empty(),
            "JSX を構文エラーとして飛ばしている: {unchunkable:?}"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_counts_the_files_it_walked() {
        let scan = scan_of_fixture("scan");

        assert_eq!(
            scan.file_count(),
            6,
            "除外したディレクトリと TypeScript でないファイルは数えない"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_with_an_unreadable_file_keeps_the_path_and_the_reason() {
        // 対照として同じディレクトリに読めるファイルを 1 つ置いてある。
        // 読めない 1 件で走査全体が止まらないことも、ここで見る
        let scan = scan_of_fixture("scan-skipped");

        let skipped: Vec<String> = scan
            .skipped_files()
            .iter()
            .map(SkippedFile::to_string)
            .collect();
        assert_eq!(skipped.len(), 1, "飛ばしたのは 1 件: {skipped:?}");
        assert!(
            skipped[0].contains("not-utf8.ts"),
            "どのファイルを飛ばしたかが残る: {skipped:?}"
        );
        assert_eq!(
            scan.chunk_count(),
            1,
            "読めたファイルの関数は切り出せている"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_with_a_broken_function_keeps_where_it_could_not_chunk() {
        // 対照は同じディレクトリの sound.ts。壊れた関数だけがチャンクから外れる
        let scan = scan_of_fixture("scan-skipped");

        let unchunkable: Vec<String> = scan.unchunkable().iter().map(Location::to_string).collect();
        assert_eq!(
            unchunkable.len(),
            1,
            "切り出せなかったのは 1 件: {unchunkable:?}"
        );
        assert!(
            unchunkable[0].ends_with("unterminated.ts:1"),
            "どの関数を飛ばしたかが位置で残る: {unchunkable:?}"
        );
    }

    #[test]
    fn test_scan_of_a_missing_directory_reports_the_root_it_was_given() {
        let root = fixture("scan/missing");

        let result = scan_of(&root, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        let Err(CodebaseError::RootNotADirectory { root: reported }) = result else {
            panic!("ディレクトリでない根は RootNotADirectory になる");
        };
        assert_eq!(reported, root);
    }
}
