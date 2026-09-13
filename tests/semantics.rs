//! 候補ペアのチャンクから、LSP へ問い合わせて型シグネチャと参照元を比べるところまで。
//!
//! ステージをつないだ結果はここで見る（rules/testing.md「ステージをまたぐテストと
//! 単体のテストを分ける」）。名前の位置は `syntax::chunk`、応答の読み取りは
//! `lsp::hover` / `lsp::references`、正規化と比較は `semantics::type_signature` /
//! `semantics::caller_domain` のモジュール内テストにある。
//!
//! **どのテストも実サーバを要するので `#[ignore]` を付ける。** サーバの入っていない
//! 開発機で黙って通さないため（rules/testing.md「LSP を要するテストは、飛ばしたことが
//! 分かる形にする」）。CI はサーバを入れて `--ignored` で走らせる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dryguard::classification::signal::{CallerDomainOverlap, TypeSignatureMatch};
use dryguard::classification::verdict::Verdict;
use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::codebase::source_of;
use dryguard::location::Location;
use dryguard::lsp::{
    Client, ReferencesOutcome, ServerCommand, Session, SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::{MeasuredPair, chunk_pair_of, measured_pair_of};
use dryguard::report::text_of;
use dryguard::semantics::caller_domain::CallerDomains;
use dryguard::semantics::resolved_type::traced_type_names_of;
use dryguard::semantics::type_signature::{
    OverloadSet, TypeSignatureOutcome, type_signature_outcome_of,
};
use dryguard::syntax::chunk::Chunk;

/// `tests/fixtures/` 配下の位置。
///
/// カレントディレクトリではなくマニフェストの位置から組み立てる
/// （テストの実行位置に依存させない）。
///
/// `expect` ではなく `panic!` で落とすのは、`clippy.toml` の `allow-expect-in-tests` が
/// `#[test]` の関数と `#[cfg(test)]` のモジュールしか見ないため。統合テストの
/// ヘルパー関数はそのどちらでもなく、`expect` は本番コードと同じく落とされる。
fn fixture(relative_path: &str, line: usize) -> Location {
    let text = format!(
        "{}/tests/fixtures/{relative_path}:{line}",
        env!("CARGO_MANIFEST_DIR")
    );

    let Ok(location) = text.parse() else {
        panic!("テストが組み立てる位置は解釈できる: {text}");
    };
    location
}

/// そのファイルを、サーバに開かせる形にする。
fn document(path: &Path) -> SourceDocument {
    let Ok(text) = source_of(path) else {
        panic!("テストが指すファイルは読める: {}", path.display());
    };
    let Ok(document) = SourceDocument::new(path, text) else {
        panic!("テストが指すファイルは開かせられる: {}", path.display());
    };
    document
}

/// 2 つのチャンクを開かせられる、握手を終えたサーバ。
///
/// 根は `WorkspaceRoot::enclosing` が渡されたパスから決める。**テスト側で広げない**
/// （広げると、本番が作らない設定でテストが通る）。`tests/fixtures/references/` は
/// 候補ペアの共通の祖先（`src/`）に tsconfig.json を置いてあり、そこが根になる。
///
/// **印を上へ探す側は通らない。** ここで見たいのは応答の読み取りと正規化なので、
/// 根の決め方まで本番と揃える必要は無い。印より下が根になるペアを本番の経路で
/// 通すのは `test_compare_with_a_pair_below_the_project_marker_still_sees_every_caller`。
fn session_over(paths: &[PathBuf]) -> Session {
    let Ok(root) = WorkspaceRoot::enclosing(paths) else {
        panic!("テストが渡すパスからは根を決められる");
    };
    let Ok(client) = Client::start(&ServerCommand::typescript()) else {
        panic!("typescript-language-server を起動できる");
    };
    let Ok(session) = client.handshake(&root) else {
        panic!("サーバと握手できる");
    };
    session
}

/// そのチャンクの型シグネチャを、サーバに尋ねて正規化したもの。
///
/// **宣言までは辿るが、エイリアスは開かない。** ここで見たいのは返った綴りを正規化して
/// 比べるところまでで、開いた綴りを差し込んだ形は `measured_with_an_lsp` を使うテストが見る。
///
/// **辿るところまでは省けない。** 書かれた型名を尋ねずに渡すと、比較に残る綴りの型名が
/// 「そもそも尋ねていない」に当たり、正規化まで進まずに `UntracedTypeName` になる
/// （`rules/architecture.md`「どこまでを「取れなかった」に数えるか」）。**本番でも
/// 書かれた型名は必ず尋ねる**ので、尋ねていない状態を渡すほうが実態から離れている。
fn type_signature_of(session: &mut Session, chunk: &Chunk) -> OverloadSet {
    let document = document(chunk.path());
    if session.open_document(&document).is_err() {
        panic!("ファイルを開かせられる: {}", chunk.path().display());
    }

    let Some(position) = chunk.name_position() else {
        panic!(
            "テストが指すチャンクは名前を持つ: {}",
            chunk.path().display()
        );
    };
    let Ok(traced) = traced_type_names_of(session, &document, chunk.type_references()) else {
        panic!("書かれた型名を尋ねられる: {}", chunk.path().display());
    };
    let asked = type_signature_outcome_of(
        session,
        &document,
        position,
        chunk.overload_name_positions(),
        &traced,
    );
    let Ok(TypeSignatureOutcome::Normalized(overloads)) = asked else {
        panic!(
            "サーバが返した綴りは読み取れる: {} ({asked:?})",
            chunk.path().display()
        );
    };
    overloads
}

/// 2 箇所のチャンクの型シグネチャが単一化できるか、実サーバに尋ねて確かめる。
fn unifiable(location_a: &Location, location_b: &Location) -> bool {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
    ]);
    let signature_a = type_signature_of(&mut session, pair.chunk_a());
    let signature_b = type_signature_of(&mut session, pair.chunk_b());
    let unifiable = signature_a.is_unifiable_with(&signature_b);

    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }
    unifiable
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_date_formatting_functions_have_unifiable_type_signatures() {
    // 計画の出力イメージで EXTRACT-CANDIDATE 側に置かれているペア
    // （`docs/dryguard-plan.md`「出力イメージ」）。どちらも `(Date) => string`
    let formats_a_date = fixture("utils/formatDate.ts", 3);
    let helps_with_dates = fixture("report/dateHelper.ts", 3);

    assert!(unifiable(&formats_a_date, &helps_with_dates));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_taking_types_from_separate_domains_are_not_unifiable() {
    // 対照は上のテスト。構造は同じだが、受け取る型が別ドメインのもので
    // `(Invoice) => number` と `(Stock) => number` になる
    //
    // **`references/src/` 側の同じ組を使う。** こちらは共通の祖先に tsconfig.json が
    // あるので、書かれた型名の宣言までサーバが答える。印の無い `tests/fixtures/` 直下の
    // 組では `NoDeclarationSite` になり、**本番でも比較まで進まない**（実測）
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let reorders_stock = fixture("references/src/inventory/reorder.ts", 5);

    assert!(!unifiable(&discounts_an_invoice, &reorders_stock));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_with_the_same_overload_set_are_unifiable() {
    // どちらも hover は `(value: string): string (+1 overload)` を返す。
    // 隠れている 1 本まで揃えて初めて、重なることを言い切れる
    let parses = fixture("overloads/parse.ts", 3);
    let decodes = fixture("overloads/decode.ts", 3);

    assert!(unifiable(&parses, &decodes));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_differing_only_in_a_hidden_overload_are_not_unifiable() {
    // 対照は上のテスト。**表示される 1 本は上のペアと同じ綴り**で、違うのは
    // 要約に畳まれた側だけ（`number` と `Date`）。1 本だけを比べると単一化可能に出る
    let parses = fixture("overloads/parse.ts", 3);
    let reads = fixture("overloads/read.ts", 3);

    assert!(!unifiable(&parses, &reads));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_declaring_the_same_overloads_in_another_order_are_not_unifiable() {
    // 対照は 2 つ上のテスト。中身は同じで並びだけが違う。TypeScript は書かれた順に
    // 突き合わせるので、`string | number` を渡した呼び出しの解決先が変わる
    let parses = fixture("overloads/parse.ts", 3);
    let scans = fixture("overloads/scan.ts", 3);

    assert!(!unifiable(&parses, &scans));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_taking_the_same_union_in_another_order_are_unifiable() {
    // hover は共用体を書かれた順のまま返す（`string | number` / `number | string`）。
    // 綴りの一致で比べると、同じ型を受ける 2 つが別物になる
    let either = fixture("spellings/either.ts", 3);
    let reversed = fixture("spellings/reversed.ts", 2);

    assert!(unifiable(&either, &reversed));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_taking_unions_of_different_members_are_not_unifiable() {
    // 対照は上のテスト。並べ替えても、共用体の中身が違えば重ならない
    let either = fixture("spellings/either.ts", 3);
    let widened = fixture("spellings/widened.ts", 2);

    assert!(!unifiable(&either, &widened));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_a_chunk_wrapped_in_transparent_expressions_is_unifiable_with_the_same_type_written_plainly()
{
    // 包みを抜けないと名前の位置が取れず、シグネチャがそもそも付かない。
    // 抜けた先の名前に hover が答えるところまでを見る
    let asserted = fixture("wrapped/asserted.ts", 3);
    let plain = fixture("wrapped/plain.ts", 2);

    assert!(unifiable(&asserted, &plain));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_a_chunk_wrapped_in_transparent_expressions_is_not_unifiable_with_another_type() {
    // 対照は上のテスト。包みを抜けた先で本当に尋ねていれば、型が違えば重ならない
    let asserted = fixture("wrapped/asserted.ts", 3);
    let widened = fixture("wrapped/widened.ts", 2);

    assert!(!unifiable(&asserted, &widened));
}

/// そのチャンクの呼び出し元のファイル。サーバに尋ねて集める。
fn reference_paths_of(session: &mut Session, chunk: &Chunk) -> Vec<PathBuf> {
    let document = document(chunk.path());
    if session.open_document(&document).is_err() {
        panic!("ファイルを開かせられる: {}", chunk.path().display());
    }

    let Some(position) = chunk.name_position() else {
        panic!(
            "テストが指すチャンクは名前を持つ: {}",
            chunk.path().display()
        );
    };
    let outcome = session.references(&document, position);
    let Ok(ReferencesOutcome::Answered(reference_paths)) = outcome else {
        panic!(
            "名前の位置には参照元が返る: {} ({outcome:?})",
            chunk.path().display()
        );
    };
    reference_paths
}

/// そのチャンクの呼び出し元が属するドメイン。サーバに尋ねて数える。
fn caller_domains_of(session: &mut Session, chunk: &Chunk) -> CallerDomains {
    let reference_paths = reference_paths_of(session, chunk);

    let Some(caller_domains) = CallerDomains::from_reference_paths(&reference_paths) else {
        panic!("返った参照元は 1 件以上ある: {reference_paths:?}");
    };
    caller_domains
}

/// 参照元のファイル名（重複を畳んだもの）。**どのファイルが返ったか**を見るために使う。
fn reference_file_names(reference_paths: &[PathBuf]) -> BTreeSet<String> {
    reference_paths
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .collect()
}

/// 2 箇所のチャンクの呼び出し元ドメインがどれだけ重なるか、実サーバに尋ねて測る。
fn caller_domain_overlap(location_a: &Location, location_b: &Location) -> f64 {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
    ]);
    let domains_a = caller_domains_of(&mut session, pair.chunk_a());
    let domains_b = caller_domains_of(&mut session, pair.chunk_b());
    let overlap = domains_a.jaccard(&domains_b).value();

    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }
    overlap
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_called_from_separate_domains_share_no_caller_domains() {
    // 計画の出力イメージで DO-NOT-EXTRACT 側に置かれているペア
    // （`docs/dryguard-plan.md`「出力イメージ」）。applyDiscount は billing の
    // 2 ファイルから、reorderAmount は inventory の 1 ファイルから呼ばれている
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let reorders_stock = fixture("references/src/inventory/reorder.ts", 5);

    assert_eq!(
        caller_domain_overlap(&discounts_an_invoice, &reorders_stock),
        0.0
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_called_from_the_same_domain_share_their_caller_domains() {
    // 対照は上のテスト。**ディレクトリは utils と report で分かれている**が、
    // どちらも report/monthly.ts から呼ばれている。ここが Phase 0 の
    // ディレクトリ距離との違いで、置き場所ではなく実際に誰が使っているかを見る
    let formats_a_date = fixture("references/src/utils/formatDate.ts", 3);
    let helps_with_dates = fixture("references/src/report/dateHelper.ts", 3);

    assert_eq!(
        caller_domain_overlap(&formats_a_date, &helps_with_dates),
        1.0
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_caller_domains_asked_after_a_type_signature_are_still_complete() {
    // #29 が組む順（hover → references）を 1 つのセッションで通す。**先の hover で
    // サーバの作業を覚える**ので、その作業が終わる前に references を送ると、
    // 読み込み中に計算された答え（呼び出し元が欠けている）を受け取る
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let reorders_stock = fixture("references/src/inventory/reorder.ts", 5);
    let Ok(pair) = chunk_pair_of(&discounts_an_invoice, &reorders_stock) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    // 候補ペアの 2 箇所だけを渡す。`compare` が本番で作る根と同じ決め方になる
    let mut session = session_over(&[
        discounts_an_invoice.path().to_path_buf(),
        reorders_stock.path().to_path_buf(),
    ]);
    let _signature = type_signature_of(&mut session, pair.chunk_a());
    let reference_paths = reference_paths_of(&mut session, pair.chunk_a());
    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }

    // **ドメインに畳む前のファイルで見る。** 畳んでからだと、片方しか返らなくても
    // 「billing の 1 ドメイン」になり、欠けたことがテストから消える
    assert_eq!(
        reference_file_names(&reference_paths),
        BTreeSet::from(["invoice.ts".to_owned(), "statement.ts".to_owned()])
    );
}

/// 計画の出力イメージで `EXTRACT-CANDIDATE` 側に置かれているペア。
///
/// どちらも `(Date) => string` で、`utils/pad` に依存し、`report/monthly.ts` から呼ばれる。
fn shared_utility_pair() -> (Location, Location) {
    (
        fixture("references/src/utils/formatDate.ts", 3),
        fixture("references/src/report/dateHelper.ts", 3),
    )
}

/// 計画の出力イメージで `DO-NOT-EXTRACT` 側に置かれているペア。
///
/// 構造は同じだが `(Invoice) => number` と `(Stock) => number` で、呼び出し元も分かれる。
fn accidental_duplication_pair() -> (Location, Location) {
    (
        fixture("references/src/billing/discount.ts", 5),
        fixture("references/src/inventory/reorder.ts", 5),
    )
}

/// 2 箇所を切り出して、実サーバに Stage 2 を尋ねるところまで。
fn measured_with_an_lsp(location_a: &Location, location_b: &Location) -> MeasuredPair {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let measured = measured_pair_of(
        &pair,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        &ServerCommand::typescript(),
    );
    if let Some(error) = measured.semantics_error() {
        panic!("実サーバには尋ねられる: {error}");
    }
    measured
}

/// 先に指定したチャンクの、ドメインごとの参照元の件数。測れていなければ落とす。
fn references_per_domain_of_a(measured: &MeasuredPair) -> Vec<(String, usize)> {
    let CallerDomainOverlap::Measured(callers) = measured.signals().caller_domain_overlap() else {
        panic!(
            "実サーバは参照元を返す: {:?}",
            measured.signals().caller_domain_overlap()
        );
    };

    callers
        .callers_a()
        .references_per_domain()
        .iter()
        .map(|(domain, count)| {
            let name = domain
                .directory()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();

            (name, *count)
        })
        .collect()
}

/// 測れた呼び出し元ドメインの重なり。測れていなければ落とす。
fn caller_domain_overlap_value(measured: &MeasuredPair) -> f64 {
    let CallerDomainOverlap::Measured(callers) = measured.signals().caller_domain_overlap() else {
        panic!(
            "実サーバは参照元を返す: {:?}",
            measured.signals().caller_domain_overlap()
        );
    };

    callers.overlap().value()
}

/// `tests/fixtures/excluded-project/` の同じ形のペア。範囲の内と外だけが違う。
///
/// 木の根に `tsconfig.json` があり、`exclude` が `src/legacy` だけを外している。
/// **どちらのペアも印の下にある**ので、印の有無だけを見ていると区別が付かない。
///
/// 呼び出し元は 2 つ置いてある。`invoice.ts` はペアの両方から import されているので
/// サーバが組み立てるプロジェクトにも入るが、`statement.ts` は**どちらからも
/// import されていない**ので入らない。範囲から外れた側で参照元を数えると、
/// **`statement.ts` のぶんだけ黙って目減りする**。
fn pair_in(area: &str) -> (Location, Location) {
    (
        fixture(&format!("excluded-project/src/{area}/discount.ts"), 5),
        fixture(&format!("excluded-project/src/{area}/rebate.ts"), 5),
    )
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_a_pair_excluded_by_the_project_config_does_not_measure_its_callers() {
    // 祖先に `tsconfig.json` があるので `is_marked` は `true` を返すが、
    // `exclude` で外れているためサーバは呼び出し元を取りこぼす。**印の有無だけで
    // 尋ねると、揃っていない参照元を揃ったものとして受け取る**
    let (discounts_an_invoice, rebates_an_invoice) = pair_in("legacy");

    let measured = measured_with_an_lsp(&discounts_an_invoice, &rebates_an_invoice);

    assert_eq!(
        measured.signals().caller_domain_overlap(),
        &CallerDomainOverlap::OutsideProject {
            markers: vec!["tsconfig.json".to_owned(), "jsconfig.json".to_owned()],
        }
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_a_pair_inside_the_project_config_still_measures_its_callers() {
    // 対照は上のテスト。**同じ木・同じ tsconfig.json・同じ形のペア**で、
    // 置かれているディレクトリだけが `exclude` の外にある。これが測れないと、
    // 範囲を見る変更が範囲内のペアまで落としていることになる
    let (discounts_an_invoice, rebates_an_invoice) = pair_in("current");

    let measured = measured_with_an_lsp(&discounts_an_invoice, &rebates_an_invoice);

    assert_eq!(
        references_per_domain_of_a(&measured),
        vec![("current".to_owned(), 4)],
        "invoice.ts と statement.ts が import と呼び出しで 2 回ずつ挙がる"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_a_pair_in_a_referenced_project_still_measures_its_callers() {
    // solution-style の木。根の `tsconfig.json` は `files: []` で、`references` が
    // 指す `tsconfig.app.json` が実際にファイルを持つ。サーバが名乗るのは印の名前で
    // ない `tsconfig.app.json` なので、**印のファイル名で絞ると範囲内のペアまで
    // 範囲外と答える**（参照元は揃っているのに落とすことになる）
    let discounts_an_invoice = fixture("solution-project/src/discount.ts", 5);
    let rebates_an_invoice = fixture("solution-project/src/rebate.ts", 5);

    let measured = measured_with_an_lsp(&discounts_an_invoice, &rebates_an_invoice);

    assert_eq!(
        references_per_domain_of_a(&measured),
        vec![("src".to_owned(), 4)],
        "invoice.ts と statement.ts が import と呼び出しで 2 回ずつ挙がる"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_a_pair_below_the_project_marker_still_sees_every_caller() {
    // 候補ペアの 2 ファイルがどちらも `src/billing/` にあるので、共通の祖先は
    // **tsconfig.json のある `src/` より 1 段下**になる。そこを根にすると、サーバは
    // 開いたファイルとその import 先だけでプロジェクトを組み立て、`applyDiscount` の
    // 呼び出し元が 1 件も返らない。
    //
    // 数えるのは出現回数なので、invoice.ts / statement.ts が import と呼び出しで
    // 2 回ずつ挙がって 4。**取りこぼすと 2 以下に落ちる**ので、揃っているかが値に出る
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let rebates_an_invoice = fixture("references/src/billing/rebate.ts", 5);

    let measured = measured_with_an_lsp(&discounts_an_invoice, &rebates_an_invoice);

    assert_eq!(
        references_per_domain_of_a(&measured),
        vec![("billing".to_owned(), 4)]
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_finds_the_shared_utility_pair_unifiable() {
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_finds_the_accidental_duplication_not_unifiable() {
    // 対照は上のテスト。構造は似ているが、受け取る型が別ドメインのもの
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_find_an_accessor_unifiable_with_a_method() {
    // hover は `(setter) Holder.handler: (message: string) => void` を返す。綴りのまま
    // 割ると `(string) => void` になり、**下のテストが単一化可能と示すメソッドと
    // 同じ形**になってしまう（偽陽性）。チャンクはアクセサ関数なので、書ける型は
    // 綴りが言うとおりの `(string) => void` で、呼べる型とは重ならない。
    //
    // **セッターで書く。** ゲッターは引数を取れないので本体をメソッドと同じ形にできず、
    // 構造類似度が閾値に届かずに候補ペアから外れる（綴りを読む手前で降りるため、
    // このテストが見たいものに届かない）。ゲッターの綴りは
    // `semantics::type_signature` のモジュール内テストが見る
    let holds_a_handler = fixture("accessors/holder.ts", 2);
    let notifies = fixture("accessors/notifier.ts", 2);

    let measured = measured_with_an_lsp(&holds_a_handler, &notifies);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_finds_two_setters_of_one_type_unifiable() {
    // 対照は 1 つ上のテスト。**アクセサ同士なら測れる。** 書ける型が同じ 2 つは
    // 重なるので、「アクセサだから測れない」とは答えない
    let holds_a_handler = fixture("accessors/holder.ts", 2);
    let relays = fixture("accessors/relay.ts", 2);

    let measured = measured_with_an_lsp(&holds_a_handler, &relays);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_still_compares_a_property_holding_a_function_type() {
    // 対照は上のテスト。hover が返す綴りの形は同じ（`(property) Keeper.held:
    // (value: string) => void`）だが、**チャンクはアロー関数自身**なのでメンバーの型が
    // そのままチャンクの型になる。**ここが単一化可能に出ることが、上のペアを
    // 落とさなければ偽陽性になることの根拠**でもある
    //
    // 3 つのフィクスチャは本体を揃えてある。揃えないと構造類似度が閾値に届かず、
    // **綴りを読む手前で降りて**どちらのテストも見たいものに届かない
    let holds_a_handler = fixture("accessors/keeper.ts", 2);
    let notifies = fixture("accessors/notifier.ts", 2);

    let measured = measured_with_an_lsp(&holds_a_handler, &notifies);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_written_on_a_parameter() {
    // hover が返すのは `function scaleAmount(amount: Amount, factor: number): Amount` で、
    // `Amount` は展開されない。解決しないと `(Amount, number) => Amount` と
    // `(number, number) => number` になり、単一化不能と出る
    let scales_an_amount = fixture("references/src/billing/scale.ts", 3);
    let scales_a_total = fixture("references/src/report/total.ts", 1);

    let measured = measured_with_an_lsp(&scales_an_amount, &scales_a_total);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_qualified_type_alias() {
    // hover は `money.Amount` と修飾ごと返す。**尋ねる先は末尾の `Amount`**（先頭の
    // `money` は名前空間なので宣言が返らない）で、**差し替えるのは修飾ごと**
    // （末尾だけだと `money.number` という綴りになる）
    let scales_a_qualified_amount = fixture("references/src/billing/qualified.ts", 5);
    let scales_a_total = fixture("references/src/report/total.ts", 1);

    let measured = measured_with_an_lsp(&scales_a_qualified_amount, &scales_a_total);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_that_replaces_the_whole_signature() {
    // 呼び出し可能なエイリアスで注釈すると、hover は `const halveAmount: Scaling` と
    // 綴り全体をエイリアス名 1 語で返す。**引数リストが無いので、解決を綴りを読む前に
    // 差し込まないと入口に入れない**（`type_signature_outcome_of` が読み解けないと答える）
    let halves_an_amount = fixture("references/src/billing/scale.ts", 7);
    let halves_a_total = fixture("references/src/report/total.ts", 5);

    let measured = measured_with_an_lsp(&halves_an_amount, &halves_a_total);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_traces_a_type_name_written_only_in_the_wrapper() {
    // 言い切った型は包みの側にしか書かれていない。名前を探す側だけが包みを抜けると、
    // `Shape` が解決の対象に入らず宣言の場所も付かない。**別々のファイルの `Shape` が
    // 同じ綴りで並び、単一化可能に出る**（偽陽性）
    let shaped_a = fixture("wrapped/shapedA.ts", 8);
    let shaped_b = fixture("wrapped/shapedB.ts", 6);

    let measured = measured_with_an_lsp(&shaped_a, &shaped_b);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_declared_outside_the_pair_root() {
    // 候補ペアが同じディレクトリにあると、根はそのディレクトリになる。エイリアスは
    // 兄弟ディレクトリで宣言されているので、**根の下だけを開かせる形では解決できない**
    let scales_by_rate = fixture("references/src/report/scaled.ts", 3);
    let scales_by_number = fixture("references/src/report/scaled.ts", 7);

    let measured = measured_with_an_lsp(&scales_by_rate, &scales_by_number);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_declared_by_a_dependency() {
    // `PropertyKey` は `lib.es5.d.ts`（約 1 MB・依存の置き場の下）で
    // `string | number | symbol` として宣言されている。開かせる相手を絞ると、
    // 書き下した綴りと比べる側が解決できない
    let keyed_by_property = fixture("references/src/report/keyed.ts", 1);
    let keyed_by_union = fixture("references/src/report/keyed.ts", 5);

    let measured = measured_with_an_lsp(&keyed_by_property, &keyed_by_union);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_aliases_of_different_types_spelled_alike() {
    // どちらのファイルも自分だけの `Local` を宣言していて、中身は別物
    // （`{ amount: number }` と `{ label: string }`）。**`interface` は hover が
    // 構造を展開しない**ので、宣言の綴りは両側とも `type ... = Local` になる
    let boxed = fixture("references/src/billing/boxed.ts", 7);
    let wrapped = fixture("references/src/inventory/boxed.ts", 7);

    let measured = measured_with_an_lsp(&boxed, &wrapped);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_generic_aliases_of_different_types_spelled_alike() {
    // 対照は 1 つ上のテスト。**総称型の参照でも同じことが起きる**。どちらのファイルも
    // 自分だけの `interface Local<T>` を宣言していて、開いた綴りは両側とも `Local<string>`
    let charged = fixture("references/src/billing/generic.ts", 7);
    let tagged = fixture("references/src/inventory/generic.ts", 8);

    let measured = measured_with_an_lsp(&charged, &tagged);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_interfaces_of_different_types_spelled_alike() {
    // どちらのファイルも自分だけの `interface User` を export していて、中身は別物
    // （`{ invoiceId: string }` と `{ rowCount: number }`）。**hover はどちらも `User` と
    // 返す**ので、綴りだけを比べると別ドメインの 2 つが単一化可能に出る
    let labels_an_invoice_user = fixture("references/src/billing/user.ts", 5);
    let labels_a_report_user = fixture("references/src/report/user.ts", 5);

    let measured = measured_with_an_lsp(&labels_an_invoice_user, &labels_a_report_user);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_inferred_return_types_spelled_alike() {
    // どちらのファイルも自分だけの `class Receipt` を export していて、中身は別物
    // （`{ invoiceId: string }` と `{ rowCount: number }`）。**戻り値の注釈を省いているので、
    // hover が綴る `Receipt` は構文木のどこにも無く**、尋ねる位置を作れない
    // （`syntax::type_reference`）。綴りのまま比べると単一化可能に出る（偽陽性）
    let builds_a_billing_receipt = fixture("references/src/billing/inferred.ts", 5);
    let builds_a_report_receipt = fixture("references/src/report/inferred.ts", 5);

    let measured = measured_with_an_lsp(&builds_a_billing_receipt, &builds_a_report_receipt);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::UntracedTypeName
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_local_values_spelled_alike_after_typeof() {
    // どちらのファイルも自分だけの `localValue` を持っていて、中身は別物
    // （`{ invoiceId: string }` と `{ rowCount: number }`）。**hover はどちらも
    // `typeof localValue` と返す**が、`localValue` は値の名前で型名のノードにならないので
    // 尋ねる位置を作れない（`syntax::type_reference`）。綴りのまま比べると
    // 単一化可能に出る（偽陽性）
    let holds_a_billing_shape = fixture("references/src/billing/localShape.ts", 3);
    let holds_a_report_shape = fixture("references/src/report/localShape.ts", 3);

    let measured = measured_with_an_lsp(&holds_a_billing_shape, &holds_a_report_shape);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::SiteDependentSpelling
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_unifies_two_generic_functions_of_the_same_shape() {
    // 対照は上のテスト。**型変数にも辿った記録は無いが、辿る相手が居ない。**
    // これを「尋ねていない」に数えると、戻り値を注釈したジェネリック関数まで
    // まとめて測れない側へ落ちる（`rules/architecture.md`
    // 「どこまでを「取れなかった」に数えるか」）
    let takes_the_first = fixture("references/src/billing/firstOf.ts", 1);
    let takes_the_head = fixture("references/src/inventory/headOf.ts", 1);

    let measured = measured_with_an_lsp(&takes_the_first, &takes_the_head);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_unifies_two_generic_functions_binding_a_name_inside_a_conditional_type()
{
    // 条件型は綴りのまま持つので、`infer U` の `U` は綴りの中で束縛されている。
    // **戻り値まで注釈してあるのに測れない側へ落ちないこと**を見る（型変数と同じく
    // 辿る相手が居ない。`syntax::type_structure` の `SpelledBinders`）
    let unwraps_charged = fixture("references/src/billing/unwrapped.ts", 1);
    let unwraps_stocked = fixture("references/src/inventory/unwrapped.ts", 1);

    let measured = measured_with_an_lsp(&unwraps_charged, &unwraps_stocked);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_constructors_of_classes_spelled_alike() {
    // hover はコンストラクタに `constructor Carton(amount: string): Carton` を返す。
    // **戻り値の `Carton` はメソッドのノードの中に書かれていない**が、囲むクラスの宣言に
    // 書かれているので尋ねる位置はある。**測れないにせず、宣言の場所で別の記号と分ける**
    // （コンストラクタに戻り値の注釈は書けないので、測れないにすると直し先が無くなる）
    let boxes_a_charge = fixture("references/src/billing/carton.ts", 4);
    let boxes_a_stock = fixture("references/src/inventory/carton.ts", 4);

    let measured = measured_with_an_lsp(&boxes_a_charge, &boxes_a_stock);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_measures_a_constructor_of_a_class_declaring_a_constrained_variable() {
    // hover は `constructor Crated<T extends Shape>(value: T): Crated<T>` を返す。
    // **制約の `Shape` もクラスの宣言に書かれている**ので、測れない側へ落とさない
    let crates_a_charge = fixture("references/src/billing/crated.ts", 6);
    let crates_a_stock = fixture("references/src/inventory/crated.ts", 6);

    let measured = measured_with_an_lsp(&crates_a_charge, &crates_a_stock);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_unifies_two_functions_taking_one_interface_from_separate_domains() {
    // 対照は上のテスト。**綴りが同じことを拒んでいるのではなく、指している記号が別な
    // ことを拒んでいる。** こちらは report 側が billing の `User` を輸入していて、
    // 書かれたファイルは違っても宣言は 1 つ
    let labels_an_invoice_user = fixture("references/src/billing/user.ts", 5);
    let labels_an_imported_user = fixture("references/src/report/imported.ts", 3);

    let measured = measured_with_an_lsp(&labels_an_invoice_user, &labels_an_imported_user);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_measures_a_total_caller_domain_overlap_for_the_shared_utility_pair() {
    // ディレクトリは utils と report で分かれているが、どちらも report/monthly.ts から
    // 呼ばれている。**置き場所ではなく実際に誰が使っているか**を見ているのがここ
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    assert_eq!(caller_domain_overlap_value(&measured), 1.0);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_measures_no_caller_domain_overlap_for_the_accidental_duplication() {
    // 対照は上のテスト。applyDiscount は billing から、reorderAmount は inventory から呼ばれる
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    assert_eq!(caller_domain_overlap_value(&measured), 0.0);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_keeps_the_accidental_duplication_a_do_not_extract() {
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    assert_eq!(classification.verdict(), Verdict::DoNotExtract);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_keeps_the_shared_utility_pair_an_extract_candidate() {
    // 対照は上のテスト。型シグネチャの拒否権が候補側を落としていないことも、ここで見る
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_reports_the_stage2_signals_it_measured() {
    // 同じペアを LSP 無しで通したときは「測れない (LSP サーバを使えない)」が出る
    // （`tests/compare.rs`）。**判定に使われたシグナルの違いが出力から読める**のがこの Issue の完了条件
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();
    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);
    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    let text = text_of(
        &discounts_an_invoice,
        &reorders_stock,
        &classification,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
    );

    assert!(
        text.contains("型シグネチャ: 単一化不能 → 共通化しない側"),
        "測った型シグネチャが値として出る: {text}"
    );
    assert!(
        text.contains("呼び出し元ドメインの重なり 0.00 (")
            && text.contains("billing 4件 <-> ")
            && text.contains("inventory 2件) → 共通化しない側"),
        "測った重なりと両側の分布が出る: {text}"
    );
}
