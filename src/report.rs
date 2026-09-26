//! 判定と根拠を、人が読む text にする層。
//!
//! **文を組み立てるのはここだけ。** `reason` は「シグナルの値と、それに当てた閾値と、
//! それが傾けた向き」の組であって文ではない（`rules/naming.md`「このツールの語彙を固定する」）。
//! 判定側が文を持つと、判定に効いた値と向きが文字列に埋もれて後段が読めなくなる。
//!
//! 判定はしない。ラベルと根拠を受け取って並べるだけで、シグナルからラベルを決めるのは
//! `classification` にしか無い（`rules/architecture.md`「判定は 1 箇所にだけ置く」）。

mod json;

pub use json::{json_of, scan_json_of};

use crate::classification::Classification;
use crate::classification::reason::{Lean, Reason};
use crate::classification::signal::{
    CalleeDomainOverlap, CallerDomainOverlap, ImportOverlap, MeasuredCalleeDomains,
    MeasuredCallerDomains, SemanticsUnavailable, StructuralSimilarity, TypeSignatureMatch,
};
use crate::classification::verdict::Verdict;
use crate::domain_declaration::AmbiguousDomain;
use crate::location::Location;
use crate::pipeline::{Scan, SkippedFile};
use crate::semantics::callee_domain::CalleeDomains;
use crate::semantics::caller_domain::CallerDomains;
use crate::semantics::domain::Domain;
use crate::semantics::resolved_type::UnopenedReason;
use crate::semantics::type_signature::UntracedReason;
use crate::syntax::import::ImportsUnavailable;
use crate::syntax::module_distance::ModuleDistance;
use crate::threshold::Threshold;

/// 見出しの行に付ける字下げ。
const INDENT: &str = "  ";

/// `理由:` の 2 行目以降に付ける字下げ。
///
/// 見出し（`  理由: `）と同じ表示幅にして、根拠が縦に揃うようにしている。
const REASON_CONTINUATION_INDENT: &str = "        ";

/// 根拠をどこまで出すか。`--explain` が切り替える。
///
/// **既定でも判定に効いた根拠は出す。** `--explain` が増やすのは、**尋ねなかったシグナル**と
/// **各シグナルに当てた閾値**の 2 つだけ。既定を結論だけに削ると、Phase 0 から
/// 読めていた根拠が減り、「判断に理由が付く」というこのツールの価値が既定で消える
/// （`docs/dryguard-plan.md`「差別化ポイント」）。
///
/// **Why not（`bool` で受ける）**: 呼び出し側が `text_of(.., true)` になり、
/// 何が true なのかが読めない（`rules/coding.md`「値の語彙を型で閉じる」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Explanation {
    /// 尋ねたシグナルの値と傾きだけを出す（既定の出力）。
    AskedSignals,
    /// 尋ねなかったシグナルと、当てた閾値まで出す（`--explain`）。
    AllSignals,
}

/// 判定と根拠を、計画の出力イメージの形にする。
///
/// `explanation` は根拠をどこまで出すか（`--explain` が [`Explanation::AllSignals`]）。
///
/// **閾値は根拠が運んでくる。** 判定が当てた値をそのまま出すので、
/// ここが定数を読みに行くことはない（`classification::reason::Reason`）。
/// 構造類似度の行だけは既定でも閾値を併記する。併記しないと、`--threshold` の指定が
/// どこに効いたのかが出力から読めない。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
///
/// Stage 2（LSP）が要る行——型シグネチャ・呼び出し元と呼び出し先の分布——は、**尋ねたときだけ**出す。
/// 尋ねていないものを空欄やダミーで埋めずに行ごと出さない
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// **尋ねて取れなかったときは理由まで出す**（環境が悪いのか材料が無いのかで、
/// 読者が次にすることが違う）。`--explain` は**尋ねなかったことも理由付きで**出す。
pub fn text_of(
    location_a: &Location,
    location_b: &Location,
    classification: &Classification,
    explanation: Explanation,
) -> String {
    let mut lines = vec![format!(
        "[{}] {location_a} <-> {location_b}",
        classification.verdict()
    )];
    let mut reason_texts = Vec::new();

    for reason in classification.reasons() {
        match reason {
            Reason::StructuralSimilarity {
                signal,
                threshold,
                lean,
            } => lines.push(format!(
                "{INDENT}構造類似度: {} → {}",
                structural_similarity_text_of(*signal, *threshold),
                lean_text_of(*lean)
            )),
            Reason::ImportOverlap {
                signal,
                threshold,
                lean,
            } => reason_texts.push(format!(
                "{} → {}",
                import_overlap_text_of(*signal, *threshold, explanation),
                lean_text_of(*lean)
            )),
            Reason::ModuleDistance {
                signal,
                separate_directory_steps,
                lean,
            } => reason_texts.push(format!(
                "{} → {}",
                module_distance_text_of(*signal, *separate_directory_steps, explanation),
                lean_text_of(*lean)
            )),
            Reason::TypeSignatureMatch { signal, lean } => {
                lines.extend(
                    type_signature_text_of(*signal, explanation).map(|signal_text| {
                        format!(
                            "{INDENT}型シグネチャ: {signal_text} → {}",
                            lean_text_of(*lean)
                        )
                    }),
                );
            }
            Reason::CallerDomainOverlap {
                signal,
                threshold,
                lean,
            } => {
                reason_texts.extend(
                    caller_domain_overlap_text_of(signal, *threshold, explanation)
                        .map(|signal_text| format!("{signal_text} → {}", lean_text_of(*lean))),
                );
            }
            Reason::CalleeDomainOverlap {
                signal,
                threshold,
                lean,
            } => {
                reason_texts.extend(
                    callee_domain_overlap_text_of(signal, *threshold, explanation)
                        .map(|signal_text| format!("{signal_text} → {}", lean_text_of(*lean))),
                );
            }
        }
    }

    lines.extend(reason_lines_of(&reason_texts));
    lines.push(format!(
        "{INDENT}提案: {}",
        suggestion_of(classification.verdict())
    ));

    lines.join("\n")
}

/// 走査の結果を、候補ペアごとの text と走査した量にする。
///
/// `explanation` は根拠をどこまで出すか（`--explain` が [`Explanation::AllSignals`]）。
///
/// 候補ペアは [`text_of`] と同じ形で並べる。**`compare` と `scan` で同じペアの
/// 見え方が変わると、片方で見た結果をもう片方で確かめられない。**
///
/// 飛ばしたファイルと切り出せなかった関数は、あるときだけ節ごと出す。
/// 空の見出しを残すと、読む側は「何かを飛ばした」と読む。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
pub fn scan_text_of(scan: &Scan, explanation: Explanation) -> String {
    let mut blocks: Vec<String> = scan
        .candidate_pairs()
        .iter()
        .map(|pair| {
            text_of(
                pair.location_a(),
                pair.location_b(),
                pair.classification(),
                explanation,
            )
        })
        .collect();

    blocks.extend(listed_block_of(
        "読めなかったファイル:",
        scan.skipped_files().iter().map(SkippedFile::to_string),
    ));
    blocks.extend(listed_block_of(
        "構文エラーで切り出せなかった関数:",
        scan.unchunkable().iter().map(Location::to_string),
    ));
    blocks.push(walked_text_of(scan));

    blocks.join("\n\n")
}

/// 見出しと、字下げした項目の並び。項目が 1 つも無ければ節ごと作らない。
fn listed_block_of(heading: &str, items: impl Iterator<Item = String>) -> Option<String> {
    let lines: Vec<String> = std::iter::once(heading.to_owned())
        .chain(items.map(|item| format!("{INDENT}{item}")))
        .collect();

    let only_the_heading = lines.len() == 1;
    if only_the_heading {
        return None;
    }
    Some(lines.join("\n"))
}

/// 走査した量。**候補の数だけでは「見ていないもの」が分からない。**
///
/// 比べたペアの内訳（長さの上限だけで確定した数）も出す。省いた数が読めないと、
/// 同じ「比較 N ペア」がどれだけの突き合わせを指すのかが回ごとに変わって見える。
fn walked_text_of(scan: &Scan) -> String {
    format!(
        "対象 {} ファイル / チャンク {} 件 / 比較 {} ペア（うち長さで確定 {} ペア）/ 候補 {} ペア",
        scan.file_count(),
        scan.chunk_count(),
        scan.compared_pair_count(),
        scan.pruned_pair_count(),
        scan.candidate_pairs().len()
    )
}

/// 根拠の行。見出し `理由:` は最初の 1 件にだけ付け、続きは同じ桁から始める。
fn reason_lines_of(reason_texts: &[String]) -> Vec<String> {
    reason_texts
        .iter()
        .enumerate()
        .map(|(index, reason_text)| {
            let is_first_reason = index == 0;

            if is_first_reason {
                return format!("{INDENT}理由: {reason_text}");
            }
            format!("{REASON_CONTINUATION_INDENT}{reason_text}")
        })
        .collect()
}

/// 構造類似度の値。測れていなければ、その理由。
///
/// 測れていないときに閾値を並べない。比べていない値を並べると、比べた結果として読める。
fn structural_similarity_text_of(signal: StructuralSimilarity, threshold: Threshold) -> String {
    match signal {
        StructuralSimilarity::Measured(similarity) => format!("{similarity} (閾値 {threshold})"),
        StructuralSimilarity::NoTokens => "測れない (トークンが 1 つも無い)".to_owned(),
    }
}

/// 依存モジュールの重なりの値。測れていなければ、その理由。
///
/// 当てた閾値は `--explain` のときだけ併記する。**測れたときにしか併記しない**のは
/// [`structural_similarity_text_of`] と同じ理由。
///
/// **理由を「宣言が無い」と言い切らない。** 読み取れる形は文法が持つ書き方より狭く
/// (``require(`./${name}`)`` など)、宣言があるのに 1 件も集まらないことがある。
/// 言い切ると、読む側は「このファイルは何にも依存していない」と受け取る。
fn import_overlap_text_of(
    signal: ImportOverlap,
    threshold: Threshold,
    explanation: Explanation,
) -> String {
    match signal {
        ImportOverlap::Measured(overlap) => format!(
            "依存先の重なり {overlap}{}",
            applied_threshold_text_of(threshold, explanation)
        ),
        ImportOverlap::Unavailable(cause) => {
            format!(
                "依存先の重なりを測れない ({})",
                imports_unavailable_text_of(cause)
            )
        }
    }
}

/// 依存先の集合を作れなかった理由。
///
/// **利用者が次にすることで分ける。** 宣言が無いのはそのファイルがそうだという話、
/// 綴りが曖昧なのはこのツールでは測れない書き方だという話、読み取れなかったのは
/// dryguard 側の穴（`rules/architecture.md`「理由は落とさない」）。
fn imports_unavailable_text_of(cause: ImportsUnavailable) -> String {
    match cause {
        ImportsUnavailable::NoDeclarations => "依存の宣言が無いファイルがある".to_owned(),
        ImportsUnavailable::ReboundSpelling => {
            "require の綴りが読み込みを指すと言い切れないファイルがある".to_owned()
        }
        ImportsUnavailable::UnreadableDeclaration { line } => {
            format!("{line} 行目の依存の宣言を読み取れなかったファイルがある")
        }
    }
}

/// モジュール距離の値。段数は必ず取れるので、測れなかった形にはならない。
///
/// `separate_directory_steps` は別のディレクトリと見なした段数で、`--explain` の
/// ときだけ併記する。
fn module_distance_text_of(
    distance: ModuleDistance,
    separate_directory_steps: usize,
    explanation: Explanation,
) -> String {
    format!(
        "モジュール距離 {} 段{}",
        distance.steps(),
        applied_steps_text_of(separate_directory_steps, explanation)
    )
}

/// `--explain` のときだけ付ける、そのシグナルに当てた閾値。
///
/// 3 つの閾値はどれも外から動かせる（`classification::ConfiguredThresholds`）が、
/// 既定で出すのは構造類似度の行だけ。**`--threshold` はその実行の引数なので、
/// 出力だけが残る場面（エージェント・CI のログ）から辿れない。** 残る 2 つを動かすのは
/// リポジトリに残る `dryguard.toml` なので、出力に無くてもそちらを読める。
fn applied_threshold_text_of(threshold: Threshold, explanation: Explanation) -> String {
    match explanation {
        Explanation::AskedSignals => String::new(),
        Explanation::AllSignals => format!(" (閾値 {threshold})"),
    }
}

/// `--explain` のときだけ付ける、別のディレクトリと見なした段数。
///
/// [`applied_threshold_text_of`] と分けるのは、**段数は 0.0-1.0 の閾値ではない**ため
/// （単位を付けないと、同じ `(閾値 2)` が重なりの値として読める）。
fn applied_steps_text_of(separate_directory_steps: usize, explanation: Explanation) -> String {
    match explanation {
        Explanation::AskedSignals => String::new(),
        Explanation::AllSignals => format!(" (閾値 {separate_directory_steps} 段)"),
    }
}

/// 型シグネチャの単一化の可否。測れていなければ、その理由。
///
/// LSP に尋ねていないときだけ `None` を返し、**行ごと出さない**。空欄やダミーで
/// 埋めると、読む側は測った結果としてそれを読む
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// **測れなかったのとは別**なので、そちらは理由まで出す。
fn type_signature_text_of(signal: TypeSignatureMatch, explanation: Explanation) -> Option<String> {
    let text = match signal {
        TypeSignatureMatch::Unifiable => "単一化可能",
        TypeSignatureMatch::NotUnifiable => "単一化不能",
        TypeSignatureMatch::Unavailable { reason } => {
            semantics_unavailable_text_of(reason, explanation)?
        }
        TypeSignatureMatch::NoName => "測れない (チャンクが名前を持たない)",
        TypeSignatureMatch::NoTypeThere => "測れない (サーバがその位置に型を持たない)",
        TypeSignatureMatch::UnreadableHover => "測れない (hover の応答を読めない)",
        TypeSignatureMatch::ServerStillWorking => {
            "測れない (サーバが作業中で hover の答えが落ち着かない)"
        }
        TypeSignatureMatch::UnreadableSignature => "測れない (返った綴りを読み解けない)",
        TypeSignatureMatch::HoverNotProvided => "測れない (サーバが hover を提供していない)",
        TypeSignatureMatch::UnopenedTypeName { reason } => unopened_text_of(reason),
        TypeSignatureMatch::UntracedTypeName { reason } => untraced_text_of(reason),
        // **ここだけ直す先を出さない。** 形ごとに直し方が違い、`this` 型は
        // **囲むクラスの外へ出せない**（型エイリアスに移せず、クラス名に置き換えると
        // 部分型での振る舞いが変わる）。1 つの文にまとめると、直せない相手へ
        // 直せると言うことになる
        TypeSignatureMatch::SiteDependentSpelling => {
            "測れない (比較に残る綴りが書かれた場所で決まる)"
        }
        // 本数は測れなかった相手そのものなので、綴りに埋め込む。**「オーバーロードが
        // 揃わない」だけでは、利用者はどれだけ足りないのかを見られない**
        TypeSignatureMatch::OverloadSetMiscounted { counted, found } => {
            return Some(format!(
                "測れない (サーバは {counted} 本のオーバーロードを数えたが、揃えられたのは {found} 本)"
            ));
        }
    };

    Some(text.to_owned())
}

/// 比較に残る型名を尋ねていない理由。
///
/// **言い切る側と言い切らない側を分ける。** 注釈が省かれているのは構文木から
/// 確かめてあるので直す先を出せるが、記録が無いことしか言えないほうは
/// **dryguard が集め損ねただけかもしれない**
/// （`semantics::type_signature::UntracedReason`）。
fn untraced_text_of(reason: UntracedReason) -> &'static str {
    match reason {
        // **直す先を出す。** 開けなかったのとは違い、サーバの側でできることは無く、
        // 対象のコードに注釈を書くと尋ねる位置ができる
        UntracedReason::OmittedTypeAnnotation => {
            "測れない (型注釈が省かれている: 注釈を書くと辿れる)"
        }
        // **直す先を言い切らない。** 引数の数が揃わないと**どの引数が省いたのかを
        // 言えない**ので、注釈はもう書かれているかもしれない
        UntracedReason::UnalignedParameters => {
            "測れない (引数の数が綴りと揃わない: どの引数の型名を尋ねていないかを言えない)"
        }
        // **直す先を言い切らない。** 型名を集める場所の一覧は TypeScript の文法が持つ
        // 形の数だけ増え続けるので、「書かれていない」と出すと**注釈を書いてある
        // 利用者へ嘘の案内を出す**ことになる
        UntracedReason::NoTracedRecord => {
            "測れない (比較に残る型名を尋ねていない: ソースに書かれていないか、dryguard が集め損ねている)"
        }
    }
}

/// 比較に残る型名を開けなかった理由。
///
/// **理由まで出す。** どれなのかで利用者が次にすることが違う
/// （`semantics::resolved_type::UnopenedReason`）。
fn unopened_text_of(reason: UnopenedReason) -> &'static str {
    match reason {
        UnopenedReason::TypeDefinitionNotProvided => {
            "測れない (比較に残る型名を開けない: サーバが typeDefinition を提供していない)"
        }
        UnopenedReason::NoDeclarationSite => {
            "測れない (比較に残る型名を開けない: サーバが宣言の場所を答えない)"
        }
        UnopenedReason::UnreadableTypeDefinition => {
            "測れない (比較に残る型名を開けない: typeDefinition の応答を読めない)"
        }
        UnopenedReason::UnreadableDeclaringDocument => {
            "測れない (比較に残る型名を開けない: 宣言のファイルを読めない)"
        }
        UnopenedReason::NoSpellingAtDeclaration => {
            "測れない (比較に残る型名を開けない: サーバが宣言の位置に型を持たない)"
        }
        UnopenedReason::UnreadableDeclarationHover => {
            "測れない (比較に残る型名を開けない: 宣言の位置の hover の応答を読めない)"
        }
        UnopenedReason::HoverNotProvided => {
            "測れない (比較に残る型名を開けない: サーバが hover を提供していない)"
        }
        UnopenedReason::ServerStillWorking => {
            "測れない (比較に残る型名を開けない: サーバが作業中で宣言の位置の hover の答えが落ち着かない)"
        }
        UnopenedReason::UnopenableAlias => {
            "測れない (比較に残る型名を開けない: エイリアスの右辺を差し込める形にできない)"
        }
    }
}

/// 呼び出し元ドメインの重なりの値と分布。測れていなければ、その理由。
///
/// 尋ねていないときに `None` を返すのは [`type_signature_text_of`] と同じ理由。
fn caller_domain_overlap_text_of(
    signal: &CallerDomainOverlap,
    threshold: Threshold,
    explanation: Explanation,
) -> Option<String> {
    let unmeasured = match signal {
        CallerDomainOverlap::Measured(measured) => {
            return Some(measured_caller_domains_text_of(
                measured,
                threshold,
                explanation,
            ));
        }
        CallerDomainOverlap::Unavailable { reason } => {
            let unavailable = semantics_unavailable_text_of(*reason, explanation)?;

            return Some(format!("呼び出し元ドメインの重なりを{unavailable}"));
        }
        CallerDomainOverlap::NoName => "チャンクが名前を持たない",
        // 利用者が直せるので、次にすることまで出す。**印の綴りはシグナルが運んだものを
        // 使う**（ここに書き写すと、サーバを足したときにそのサーバに無い名前を勧める）。
        CallerDomainOverlap::ProjectUnrooted { markers } => {
            return Some(format!(
                "呼び出し元ドメインの重なりを測れない ({})",
                unrooted_project_text_of(markers)
            ));
        }
        // こちらも利用者が直せる。直す相手が印そのものなので、置く話ではなく
        // 範囲の話として出す。
        CallerDomainOverlap::OutsideProject { markers } => {
            return Some(format!(
                "呼び出し元ドメインの重なりを測れない ({})",
                outside_project_text_of(markers)
            ));
        }
        CallerDomainOverlap::ProjectMembershipNotProvided => {
            "サーバがプロジェクトの所属を答えられない"
        }
        CallerDomainOverlap::NoReferences => "参照元が 1 件も返らない",
        CallerDomainOverlap::UnreadableReferences => "読めない URI が混じっている",
        CallerDomainOverlap::ServerStillWorking => "サーバが作業中で答えが落ち着かない",
        CallerDomainOverlap::ReferencesNotProvided => "サーバが references を提供していない",
        CallerDomainOverlap::AmbiguousDomain(ambiguous) => {
            return Some(format!(
                "呼び出し元ドメインの重なりを測れない (参照元の {})",
                ambiguous_domain_text_of(ambiguous)
            ));
        }
    };

    Some(format!(
        "呼び出し元ドメインの重なりを測れない ({unmeasured})"
    ))
}

/// 呼び出し先ドメインの重なりの値と分布。測れていなければ、その理由。
///
/// 尋ねていないときに `None` を返すのは [`type_signature_text_of`] と同じ理由。
///
/// **測れなかった理由を 1 つの文に畳まない。** callHierarchy が使えなかったときほど
/// 判定は残りのシグナルに寄るので、どのシグナルで出た判定なのかを読む側が知れないと
/// **判定の重みを誤る**。理由ごとに直す先も違う（サーバを替える / 待つ / 対象のコード）。
fn callee_domain_overlap_text_of(
    signal: &CalleeDomainOverlap,
    threshold: Threshold,
    explanation: Explanation,
) -> Option<String> {
    let unmeasured = match signal {
        CalleeDomainOverlap::Measured(measured) => {
            return Some(measured_callee_domains_text_of(
                measured,
                threshold,
                explanation,
            ));
        }
        CalleeDomainOverlap::Unavailable { reason } => {
            let unavailable = semantics_unavailable_text_of(*reason, explanation)?;

            return Some(format!("呼び出し先ドメインの重なりを{unavailable}"));
        }
        CalleeDomainOverlap::NoName => "チャンクが名前を持たない",
        CalleeDomainOverlap::NoCallHierarchyItem => "サーバが呼び出し関係の起点を返さない",
        // 数は曖昧さそのもの。**どれだけ曖昧だったか**を落とさない
        CalleeDomainOverlap::SeveralCallHierarchyItems { count } => {
            return Some(format!(
                "呼び出し先ドメインの重なりを測れない (呼び出し関係の起点が {count} 個返り、どれか決められない)"
            ));
        }
        CalleeDomainOverlap::NoCallees => "呼び出し先が 1 件も返らない",
        CalleeDomainOverlap::OnlyExternalCallees => {
            "呼び出し先がどれも依存パッケージ (node_modules) の中にある"
        }
        CalleeDomainOverlap::UnreadableCallees => "読めない URI が混じっている",
        CalleeDomainOverlap::ServerStillWorking => "サーバが作業中で答えが落ち着かない",
        CalleeDomainOverlap::CallHierarchyNotProvided => "サーバが callHierarchy を提供していない",
        CalleeDomainOverlap::AmbiguousDomain(ambiguous) => {
            return Some(format!(
                "呼び出し先ドメインの重なりを測れない (呼び出し先の {})",
                ambiguous_domain_text_of(ambiguous)
            ));
        }
    };

    Some(format!(
        "呼び出し先ドメインの重なりを測れない ({unmeasured})"
    ))
}

/// プロジェクトの印が見つからなかったことと、置けば揃う印の名前。
///
/// `markers` はそのサーバが探した印の名前（`lsp::ServerCommand::project_markers`）。
/// **空なら名前を出さない。** 印で範囲を決めないサーバではそもそもここへ来ないが、
/// 来たときに「何も置かなくてよい」を「何かを置け」と読ませない。
fn unrooted_project_text_of(markers: &[String]) -> String {
    if markers.is_empty() {
        return "プロジェクトの印が見つからない".to_owned();
    }

    format!(
        "プロジェクトの印が見つからない: {} のどれかを置くと参照元が揃う",
        markers.join(" / ")
    )
}

/// 印の範囲から外れていることと、範囲を持つ印の名前。
///
/// `markers` はそのサーバが探した印の名前（`lsp::ServerCommand::project_markers`）。
/// **空なら名前を出さない。** 印で範囲を決めないサーバではそもそもここへ来ないが、
/// 来たときに「無い印の範囲を直せ」と読ませない。
///
/// **範囲を絞っている項目名までは出さない。** `files` / `include` / `exclude` のどれが
/// 効いたかも、`references` の先の設定ファイルが絞ったのかも、印のファイルを読まないと
/// 言えず、読まずに済ませたのがこの実装の要点（サーバに尋ねて答えを得ている）。
fn outside_project_text_of(markers: &[String]) -> String {
    if markers.is_empty() {
        return "プロジェクトの範囲から外れている".to_owned();
    }

    format!(
        "プロジェクトの範囲から外れている: {} が指す範囲を見直すと参照元が揃う",
        markers.join(" / ")
    )
}

/// Stage 2 へ届かなかったことを表す文。
///
/// **尋ねていないだけなら既定では `None`** を返し、行ごと出さない。空欄やダミーで埋めると、
/// 読む側は測った結果としてそれを読む
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// `--explain` は**尋ねなかったことも 1 つのシグナルの状態として**出す
/// （そこだけ行が消えると、読む側は「シグナルを全表示」の一覧から何が抜けたのかを
/// 数え直すことになる）。
///
/// 頭を「測れない」「尋ねていない」に揃えてあるのは、呼び出し元・呼び出し先ドメイン側が
/// `…の重なりを` の後ろに続けて使うため。
fn semantics_unavailable_text_of(
    reason: SemanticsUnavailable,
    explanation: Explanation,
) -> Option<&'static str> {
    match reason {
        SemanticsUnavailable::NotAsked => match explanation {
            Explanation::AskedSignals => None,
            Explanation::AllSignals => Some("尋ねていない (Stage 1 のシグナルだけで組み立てた)"),
        },
        SemanticsUnavailable::NotACandidate => {
            Some("尋ねていない (構造が似ておらず候補ペアではない)")
        }
        SemanticsUnavailable::DocumentUnopenable => Some("測れない (サーバに開かせる形にできない)"),
        SemanticsUnavailable::WorkspaceRootUndecidable => {
            Some("測れない (ワークスペースの根を決められない)")
        }
        SemanticsUnavailable::LspUnusable => Some("測れない (LSP サーバを使えない)"),
    }
}

/// 測れた重なりと、両側のドメインごとの件数。当てた閾値は `--explain` のときだけ。
fn measured_caller_domains_text_of(
    measured: &MeasuredCallerDomains,
    threshold: Threshold,
    explanation: Explanation,
) -> String {
    format!(
        "呼び出し元ドメインの重なり {}{} ({} <-> {})",
        measured.overlap(),
        applied_threshold_text_of(threshold, explanation),
        references_per_domain_text_of(measured.callers_a()),
        references_per_domain_text_of(measured.callers_b())
    )
}

/// 片側のドメインごとの件数（`src/billing 3件 / src/inventory 5件`）。
///
/// **ディレクトリは末尾の 1 段に縮めず、そのまま出す。** 縮めると、別の親の下にある
/// 同名のディレクトリが同じ綴りになり、分布を読み違える。
fn references_per_domain_text_of(callers: &CallerDomains) -> String {
    callers
        .references_per_domain()
        .iter()
        .map(|(domain, count)| format!("{} {count}件", domain_text_of(domain)))
        .collect::<Vec<String>>()
        .join(" / ")
}

/// 測れた重なりと、両側のドメインごとの呼び出し先の件数。当てた閾値は `--explain` のときだけ。
fn measured_callee_domains_text_of(
    measured: &MeasuredCalleeDomains,
    threshold: Threshold,
    explanation: Explanation,
) -> String {
    format!(
        "呼び出し先ドメインの重なり {}{} ({} <-> {})",
        measured.overlap(),
        applied_threshold_text_of(threshold, explanation),
        callees_per_domain_text_of(measured.callees_a()),
        callees_per_domain_text_of(measured.callees_b())
    )
}

/// 片側のドメインごとの呼び出し先の件数。ディレクトリを縮めない理由は
/// [`references_per_domain_text_of`] と同じ。
fn callees_per_domain_text_of(callees: &CalleeDomains) -> String {
    callees
        .callees_per_domain()
        .iter()
        .map(|(domain, count)| format!("{} {count}件", domain_text_of(domain)))
        .collect::<Vec<String>>()
        .join(" / ")
}

/// ドメイン 1 つの綴り。宣言の名前には `(宣言)` を添える。
///
/// **添えないと、宣言の名前と同じ綴りのディレクトリを見分けられない。** 両者は
/// 別のドメインとして数えている（`semantics::domain::Domain`）ので、同じ綴りに
/// 見えると重なり 0.00 の理由が読めなくなる。
fn domain_text_of(domain: &Domain) -> String {
    match domain {
        Domain::Declared(name) => format!("{name} (宣言)"),
        Domain::Directory(directory) => directory.display().to_string(),
    }
}

/// 2 つの宣言に当たったファイルと、その 2 つの名前（`dryguard.toml` を直す先）。
fn ambiguous_domain_text_of(ambiguous: &AmbiguousDomain) -> String {
    let [first, second] = ambiguous.domains();
    format!(
        "{} が dryguard.toml の 2 つの宣言 {first} / {second} に当たる",
        ambiguous.path().display()
    )
}

/// シグナルが判定を傾けた向き。
fn lean_text_of(lean: Lean) -> &'static str {
    match lean {
        Lean::TowardExtract => "共通化する側",
        Lean::TowardDoNotExtract => "共通化しない側",
        Lean::Neither => "どちらでもない",
    }
}

/// ラベルに対して人が次に取る行動。
///
/// ラベルからそのまま決まるので、シグナルを見ない。ここでシグナルを見ると
/// 判定が 2 箇所になる（`rules/architecture.md`「判定は 1 箇所にだけ置く」）。
fn suggestion_of(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::ExtractCandidate => "共通化してよい。1 つにまとめる先を検討する。",
        Verdict::DoNotExtract => "偶発的な重複の可能性が高い。共通化せず分離を維持する。",
        Verdict::Review => "判断材料が足りない。人が見て決める。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain_declaration::DomainDeclarations;
    use crate::line_number::LineNumber;
    use std::path::{Path, PathBuf};

    use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
    use crate::classification::{
        ConfiguredThresholds, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of,
    };
    use crate::similarity::Similarity;
    use crate::syntax::import::ImportsUnavailable;
    use crate::syntax::module_distance::ModuleDistance;
    use crate::test_support::{declarations_of, location, overload_count, scan_of_fixture};
    use crate::threshold::Threshold;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// 別のディレクトリにある 2 ファイルの隔たり（2 段）。
    fn separate_directories() -> ModuleDistance {
        ModuleDistance::between(
            Path::new("src/billing/discount.ts"),
            Path::new("src/inventory/reorder.ts"),
        )
    }

    /// 別のディレクトリにある 2 箇所を、渡した閾値で判定した既定の text。
    ///
    /// 距離を固定して、構造の似かたと依存先の重なりだけを動かす。
    fn text_of_separate_directories(
        structural_similarity: StructuralSimilarity,
        import_overlap: ImportOverlap,
        threshold: Threshold,
    ) -> String {
        explained_text_of_separate_directories(
            structural_similarity,
            import_overlap,
            threshold,
            Explanation::AskedSignals,
        )
    }

    /// 同じ組を、根拠をどこまで出すかまで指定して判定した text。
    fn explained_text_of_separate_directories(
        structural_similarity: StructuralSimilarity,
        import_overlap: ImportOverlap,
        threshold: Threshold,
        explanation: Explanation,
    ) -> String {
        let signals = Signals::new(
            structural_similarity,
            import_overlap,
            separate_directories(),
        );

        text_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(
                &signals,
                ConfiguredThresholds::default().with_structural_similarity(threshold),
            ),
            explanation,
        )
    }

    /// [`text_of_accidental_duplication`] と同じ組を `--explain` 付きで出した text。
    fn explained_text_of_accidental_duplication() -> String {
        explained_text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
            Explanation::AllSignals,
        )
    }

    /// 片方がもう片方のディレクトリの下にある 2 ファイルの隔たり（1 段）。
    fn nested_directories() -> ModuleDistance {
        ModuleDistance::between(
            Path::new("src/billing/tax/rate.ts"),
            Path::new("src/billing/invoice.ts"),
        )
    }

    /// 隔たりが閾値に届かない組を `--explain` 付きで出した text。
    ///
    /// 測った段数（1）と当てた段数（2）が違う入力を選ぶ。同じ値の入力では、
    /// 閾値の代わりに測った段数をもう一度出す実装でも通ってしまう
    /// （`rules/testing.md`「既定値と違う答えになる入力を選ぶ」）。
    fn explained_text_of_nested_directories() -> String {
        let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            nested_directories(),
        );

        text_of(
            &location("src/billing/tax/rate.ts", 42),
            &location("src/billing/invoice.ts", 18),
            &classification_of(
                &signals,
                ConfiguredThresholds::default().with_structural_similarity(threshold),
            ),
            Explanation::AllSignals,
        )
    }

    /// 構造が似ていて依存先を共有していない組（`DO-NOT-EXTRACT` になる）の text。
    fn text_of_accidental_duplication() -> String {
        text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        )
    }

    #[test]
    fn test_text_of_starts_with_the_verdict_and_both_locations() {
        let text = text_of_accidental_duplication();

        assert_eq!(
            text.lines().next(),
            Some("[DO-NOT-EXTRACT] src/billing/discount.ts:42 <-> src/inventory/reorder.ts:18")
        );
    }

    #[test]
    fn test_text_of_reports_the_structural_similarity_with_the_threshold_it_was_compared_against() {
        let text = text_of_accidental_duplication();

        assert!(
            text.contains("構造類似度: 0.94 (閾値 0.5) → 共通化する側"),
            "測った値・比べた閾値・傾きが 1 行で読める: {text}"
        );
    }

    #[test]
    fn test_text_of_reports_the_threshold_it_was_given_instead_of_the_default() {
        // 既定と違う値を渡す。既定と同じ値では、渡した閾値が使われたのか
        // 既定が使われたのかが分からない
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            Threshold::from_literal(0.8),
        );

        assert!(
            text.contains("(閾値 0.8)"),
            "指定された閾値がそのまま出る: {text}"
        );
    }

    #[test]
    fn test_text_of_without_explain_omits_the_thresholds_of_the_other_signals() {
        // 対照は同じ出力の構造類似度の行。`--threshold` が動かす閾値だけは既定でも出る
        let text = text_of_accidental_duplication();

        assert!(
            text.contains("依存先の重なり 0.00 → 共通化しない側")
                && text.contains("モジュール距離 2 段 → 共通化しない側"),
            "既定では値と傾きだけが並ぶ: {text}"
        );
        assert!(
            text.contains("構造類似度: 0.94 (閾値 0.5)"),
            "`--threshold` が動かす閾値は既定でも出る: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_threshold_each_signal_was_compared_against() {
        let text = explained_text_of_accidental_duplication();

        assert!(
            text.contains("依存先の重なり 0.00 (閾値 0.5) → 共通化しない側"),
            "当てた閾値が値の隣に出る: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_given_threshold_apart_from_the_others() {
        // 既定と違う `--threshold` を渡す。既定と同じ値では、構造類似度の閾値を
        // すべてのシグナルへ流用する実装でも通ってしまう
        // （`rules/testing.md`「既定値と違う答えになる入力を選ぶ」）
        let text = explained_text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            Threshold::from_literal(0.8),
            Explanation::AllSignals,
        );

        assert!(
            text.contains("構造類似度: 0.94 (閾値 0.8)")
                && text.contains("依存先の重なり 0.00 (閾値 0.5)"),
            "`--threshold` が動かした閾値と、動かさなかった閾値が別々に出る: {text}"
        );
    }

    /// 3 つの閾値を、既定値（どれも 0.5）とも互いとも違う値にして `--explain` で出した text。
    ///
    /// 同じ値にすると、**1 つの閾値をすべてのシグナルへ流用する実装でも通る**
    /// （`harness/records/pr-227.md` 指摘 2 が、動かす手立てが無いために
    /// 塞げないと書き残していた穴）。
    fn explained_text_of_three_moved_thresholds() -> String {
        let thresholds = ConfiguredThresholds::default()
            .with_structural_similarity(Threshold::from_literal(0.8))
            .with_shared_imports(Threshold::from_literal(0.7))
            .with_shared_caller_domains(Threshold::from_literal(0.6));
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        )
        .with_semantics(
            TypeSignatureMatch::NotUnifiable,
            callers_in_separate_domains(),
        );

        text_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(&signals, thresholds),
            Explanation::AllSignals,
        )
    }

    #[test]
    fn test_explained_text_of_reports_each_moved_threshold_on_its_own_signal() {
        let text = explained_text_of_three_moved_thresholds();

        assert!(
            text.contains("構造類似度: 0.94 (閾値 0.8)")
                && text.contains("依存先の重なり 0.00 (閾値 0.7)")
                && text.contains("呼び出し元ドメインの重なり 0.00 (閾値 0.6)"),
            "動かした 3 つの閾値が、それぞれのシグナルの行に出る: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_steps_it_compared_against_not_the_measured_ones() {
        let text = explained_text_of_nested_directories();

        assert!(
            text.contains("モジュール距離 1 段 (閾値 2 段) → 共通化する側"),
            "測った段数と当てた段数が別々に出る: {text}"
        );
    }

    #[test]
    fn test_text_of_without_explain_omits_the_signals_that_were_not_asked() {
        // 対照は同じ出力に並ぶ Stage 1 の行。尋ねたシグナルが 1 つも無い入力で
        // 確かめると、実装が何をしても通る
        let text = text_of_accidental_duplication();

        assert!(
            !text.contains("型シグネチャ")
                && !text.contains("呼び出し元ドメイン")
                && !text.contains("呼び出し先ドメイン"),
            "尋ねていないシグナルは行ごと出さない: {text}"
        );
        assert!(
            text.contains("構造類似度: 0.94") && text.contains("依存先の重なり 0.00"),
            "尋ねたシグナルはそのまま出る: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_signals_that_were_not_asked() {
        let text = explained_text_of_accidental_duplication();

        assert!(
            text.contains(
                "型シグネチャ: 尋ねていない (Stage 1 のシグナルだけで組み立てた) → どちらでもない"
            ),
            "尋ねなかったことも 1 つのシグナルの状態として出る: {text}"
        );
        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを尋ねていない \
                 (Stage 1 のシグナルだけで組み立てた) → どちらでもない"
            ),
            "尋ねなかった Stage 2 のシグナルは呼び出し元も出る: {text}"
        );
        assert!(
            text.contains(
                "呼び出し先ドメインの重なりを尋ねていない \
                 (Stage 1 のシグナルだけで組み立てた) → どちらでもない"
            ),
            "尋ねなかった Stage 2 のシグナルは呼び出し先も出る: {text}"
        );
    }

    #[test]
    fn test_text_of_reports_every_signal_with_the_direction_it_leaned() {
        let text = text_of_accidental_duplication();

        assert!(
            text.contains("  理由: 依存先の重なり 0.00 → 共通化しない側\n")
                && text.contains("        モジュール距離 2 段 → 共通化しない側\n"),
            "シグナルごとに値と傾きが組で出る: {text}"
        );
    }

    #[test]
    fn test_text_of_reports_a_signal_that_leaned_against_the_verdict() {
        // 依存先を共有しているので EXTRACT-CANDIDATE になるが、ディレクトリは
        // 分かれている。判定と逆へ傾いた根拠を落とすと、読者が「なぜこの判定か」を
        // 追えなくなる
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(1.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains("モジュール距離 2 段 → 共通化しない側"),
            "候補側の判定でも、反対へ傾けた根拠が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_without_imports_reports_that_the_signal_could_not_be_measured() {
        // 対照として構造類似度は測れている。測れた値と測れなかったことが
        // 同じ出方をすると、読者が両者を区別できない
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Unavailable(ImportsUnavailable::NoDeclarations),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains(
                "依存先の重なりを測れない (依存の宣言が無いファイルがある) → どちらでもない"
            ),
            "測れなかった理由まで出る: {text}"
        );
        assert!(
            text.contains("構造類似度: 0.94"),
            "測れたシグナルはそのまま値が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_unreadable_imports_reports_a_different_reason_from_having_none() {
        // 対照に「宣言が無い」側の文を置く。理由を畳んで 1 つの文にする実装だと、
        // 利用者は**書いてあるのに dryguard が読めていない**ことに気付けない
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Unavailable(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(11),
            }),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains(
                "依存先の重なりを測れない (12 行目の依存の宣言を読み取れなかったファイルがある) → どちらでもない"
            ),
            "読み取れなかったことが理由に出る: {text}"
        );
        assert!(
            !text.contains("依存の宣言が無いファイルがある"),
            "宣言が無いときの文とは別の文になる: {text}"
        );
    }

    #[test]
    fn test_text_of_with_a_rebound_require_reports_a_different_reason_from_being_unreadable() {
        // 対照に「読み取れなかった」側の文を置く。畳むと、**書いてあるものは
        // すべて読めている**のに利用者を dryguard の穴のほうへ向けてしまう
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Unavailable(ImportsUnavailable::ReboundSpelling),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains(
                "依存先の重なりを測れない (require の綴りが読み込みを指すと言い切れないファイルがある) → どちらでもない"
            ),
            "綴りが曖昧なことが理由に出る: {text}"
        );
        assert!(
            !text.contains("読み取れなかった"),
            "読み取れなかったときの文とは別の文になる: {text}"
        );
    }

    #[test]
    fn test_text_of_without_structural_similarity_omits_the_threshold() {
        // 測れていない値に閾値を並べると、比べた結果として読めてしまう
        let text = text_of_separate_directories(
            StructuralSimilarity::NoTokens,
            ImportOverlap::Measured(measured(0.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains("構造類似度: 測れない (トークンが 1 つも無い) → どちらでもない"),
            "測れなかった理由まで出る: {text}"
        );
        assert!(!text.contains("閾値"), "比べていない閾値は出さない: {text}");
    }

    #[test]
    fn test_text_of_do_not_extract_suggests_keeping_the_code_separate() {
        let text = text_of_accidental_duplication();

        assert!(
            text.contains("  提案: 偶発的な重複の可能性が高い。共通化せず分離を維持する。"),
            "共通化しない側の提案が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_extract_candidate_suggests_extracting() {
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(1.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains("  提案: 共通化してよい。1 つにまとめる先を検討する。"),
            "候補側の提案が出る: {text}"
        );
    }

    /// 候補ペアが 1 組だけ出るフィクスチャの text。
    fn scan_text_of_fixture() -> String {
        scan_text_of(
            &scan_of_fixture("scan", ConfiguredThresholds::default()),
            Explanation::AskedSignals,
        )
    }

    #[test]
    fn test_scan_text_of_reports_a_candidate_pair_with_its_verdict_and_both_locations() {
        let text = scan_text_of_fixture();

        let verdict_line = text
            .lines()
            .find(|line| line.starts_with("[DO-NOT-EXTRACT] "))
            .unwrap_or_default();
        assert!(
            verdict_line.contains("billing/discount.ts:3")
                && verdict_line.contains("inventory/reorder.ts:3"),
            "候補ペアが compare と同じ 1 行目の形で出る: {text}"
        );
    }

    #[test]
    fn test_scan_text_of_separates_the_pairs_it_lists_with_a_blank_line() {
        // 閾値を 0.0 まで下げて、比べたペアをすべて候補にする。1 組しか出ない
        // 入力では「並べた形」になっているかを確かめられない
        let scan = scan_of_fixture(
            "scan",
            ConfiguredThresholds::default()
                .with_structural_similarity(Threshold::from_literal(0.0)),
        );

        let text = scan_text_of(&scan, Explanation::AskedSignals);

        let verdict_lines = text.lines().filter(|line| line.starts_with('[')).count();
        assert_eq!(verdict_lines, 14, "比べた 14 ペアが並ぶ: {text}");
        assert!(text.contains("\n\n["), "ペアとペアの間に空行が入る: {text}");
    }

    #[test]
    fn test_scan_text_of_reports_how_much_of_the_codebase_it_walked() {
        let text = scan_text_of_fixture();

        assert!(
            text.contains(
                "対象 8 ファイル / チャンク 6 件 / 比較 14 ペア（うち長さで確定 5 ペア）/ 候補 1 ペア"
            ),
            "走査した量と、突き合わせを省いた内訳が読める: {text}"
        );
    }

    #[test]
    fn test_scan_text_of_reports_a_file_it_could_not_read() {
        let text = scan_text_of(
            &scan_of_fixture("scan-skipped", ConfiguredThresholds::default()),
            Explanation::AskedSignals,
        );

        assert!(
            text.contains("読めなかったファイル:"),
            "飛ばしたファイルの見出しが出る: {text}"
        );
        assert!(
            text.contains("not-utf8.ts"),
            "どのファイルを飛ばしたかが出る: {text}"
        );
    }

    #[test]
    fn test_scan_text_of_reports_a_function_it_could_not_chunk() {
        let text = scan_text_of(
            &scan_of_fixture("scan-skipped", ConfiguredThresholds::default()),
            Explanation::AskedSignals,
        );

        assert!(
            text.contains("構文エラーで切り出せなかった関数:"),
            "切り出せなかった関数の見出しが出る: {text}"
        );
        assert!(
            text.contains("unterminated.ts:1"),
            "どの関数を飛ばしたかが位置で出る: {text}"
        );
    }

    #[test]
    fn test_scan_text_of_without_anything_skipped_omits_those_sections() {
        // 対照は上の 2 つ。飛ばしたものが無い走査で見出しだけが残ると、
        // 読む側は「何かを飛ばした」と読む
        let text = scan_text_of_fixture();

        assert!(
            !text.contains("読めなかったファイル:") && !text.contains("構文エラーで"),
            "飛ばしたものが無ければ見出しごと出さない: {text}"
        );
    }

    /// 呼び出し元が別のドメインに分かれている（重なり 0.00）。
    fn callers_in_separate_domains() -> CallerDomainOverlap {
        let paths_a = [PathBuf::from("/repo/src/billing/invoice.ts")];
        let paths_b = [PathBuf::from("/repo/src/inventory/stock.ts")];
        let (Some(callers_a), Some(callers_b)) = (
            CallerDomains::from_reference_paths(&paths_a, &DomainDeclarations::default())
                .ok()
                .flatten(),
            CallerDomains::from_reference_paths(&paths_b, &DomainDeclarations::default())
                .ok()
                .flatten(),
        ) else {
            panic!("テストが渡す参照元は 1 件以上");
        };

        CallerDomainOverlap::Measured(MeasuredCallerDomains::new(callers_a, callers_b))
    }

    /// 構造が似ていて依存先を共有していない組を、渡した Stage 2 のシグナルで判定した text。
    fn text_of_accidental_duplication_with_semantics(
        type_signature_match: TypeSignatureMatch,
        caller_domain_overlap: CallerDomainOverlap,
    ) -> String {
        explained_text_of_accidental_duplication_with_semantics(
            type_signature_match,
            caller_domain_overlap,
            Explanation::AskedSignals,
        )
    }

    /// 同じ組を、根拠をどこまで出すかまで指定して判定した text。
    fn explained_text_of_accidental_duplication_with_semantics(
        type_signature_match: TypeSignatureMatch,
        caller_domain_overlap: CallerDomainOverlap,
        explanation: Explanation,
    ) -> String {
        let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        )
        .with_semantics(type_signature_match, caller_domain_overlap);

        text_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(
                &signals,
                ConfiguredThresholds::default().with_structural_similarity(threshold),
            ),
            explanation,
        )
    }

    #[test]
    fn test_text_of_reports_the_type_signature_with_the_direction_it_leaned() {
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::NotUnifiable,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains("  型シグネチャ: 単一化不能 → 共通化しない側\n"),
            "測った値と傾きが 1 行で読める: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unopened_type_name_says_so_instead_of_calling_the_pair_not_unifiable() {
        // 対照は上のテスト（単一化不能と言い切る場合）。**比較に残る型名を開けていないのに
        // 「単一化不能」と出すと、確かめられなかったことが答えとして読まれる**
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UnopenedTypeName {
                reason: UnopenedReason::TypeDefinitionNotProvided,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (比較に残る型名を開けない: サーバが typeDefinition を提供していない) → どちらでもない"
            ),
            "開けなかったことが理由として出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unsettled_hover_says_so_instead_of_calling_the_pair_unifiable() {
        // 対照は 1 つ上のテスト（開けなかった型名）。**待てば変わる**側なので、
        // 直す先が違う。読み込み前の hover は推論された型に `any` を綴り、`any` どうしは
        // 重なるので、黙って採ると単一化可能に出る（偽陽性）
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::ServerStillWorking,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (サーバが作業中で hover の答えが落ち着かない) → どちらでもない"
            ),
            "落ち着かなかったことが理由として出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unsettled_declaration_hover_says_which_hover_did_not_settle() {
        // 対照は 1 つ上のテスト。**落ち着かなかった hover が別**（チャンクの位置ではなく
        // 宣言の位置）で、開く先の綴りが取れていない。1 語で出すと、どちらの往復を
        // 待てばよいのかが読めない
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UnopenedTypeName {
                reason: UnopenedReason::ServerStillWorking,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (比較に残る型名を開けない: サーバが作業中で宣言の位置の hover の答えが落ち着かない) → どちらでもない"
            ),
            "宣言の位置の hover が落ち着かなかったことが出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unreadable_type_definition_says_so_instead_of_blaming_the_server() {
        // 対照は 1 つ上のテスト（サーバが提供していない場合）。**サーバは宣言を持っており、
        // 読めないのはこちら側の穴**なので、直す先が違う
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UnopenedTypeName {
                reason: UnopenedReason::UnreadableTypeDefinition,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (比較に残る型名を開けない: typeDefinition の応答を読めない) → どちらでもない"
            ),
            "読めなかったことが理由として出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_omitted_type_annotation_points_at_the_annotation_to_write() {
        // 注釈が省かれているのは構文木から確かめてあるので、**直す先を言い切ってよい**
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UntracedTypeName {
                reason: UntracedReason::OmittedTypeAnnotation,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (型注釈が省かれている: 注釈を書くと辿れる) → どちらでもない"
            ),
            "注釈を書けば辿れることまで出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_unaligned_parameters_does_not_claim_the_annotation_is_missing() {
        // 対照は 1 つ上のテスト。どちらも「尋ねていない」だが、**引数の数が揃わないと
        // どの引数が省いたのかを言えない**。注釈を書けと出すと、もう書いてある
        // 利用者に嘘の案内を出す
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UntracedTypeName {
                reason: UntracedReason::UnalignedParameters,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (引数の数が綴りと揃わない: どの引数の型名を尋ねていないかを言えない) → どちらでもない"
            ),
            "注釈を書けとは出ない: {text}"
        );
    }

    #[test]
    fn test_text_of_with_no_traced_record_does_not_claim_the_annotation_is_missing() {
        // 対照は 1 つ上のテスト。どちらも「尋ねていない」だが、**こちらは注釈が
        // 書かれているのに集め損ねただけかもしれない**。書かれていないと言い切ると、
        // 注釈を書いてある利用者に嘘の案内を出す
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::UntracedTypeName {
                reason: UntracedReason::NoTracedRecord,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (比較に残る型名を尋ねていない: ソースに書かれていないか、dryguard が集め損ねている) → どちらでもない"
            ),
            "dryguard 側の穴かもしれないことが読める: {text}"
        );
    }

    #[test]
    fn test_text_of_with_a_miscounted_overload_set_says_how_many_are_missing() {
        // 対照は 1 つ上のテスト（開けなかった型名）。どちらも測れないが、直す先が違う。
        // **本数を出さないと、利用者はどれだけ足りないのかを見られない**
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::OverloadSetMiscounted {
                counted: overload_count(3),
                found: 1,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            text.contains(
                "型シグネチャ: 測れない (サーバは 3 本のオーバーロードを数えたが、揃えられたのは 1 本) → どちらでもない"
            ),
            "揃わなかった本数が理由として出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unusable_lsp_reports_why_the_type_signature_is_missing() {
        // 対照として構造類似度は測れている。測れた値と測れなかったことが
        // 同じ出方をすると、読者が両者を区別できない
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
        );

        assert!(
            text.contains("型シグネチャ: 測れない (LSP サーバを使えない) → どちらでもない"),
            "型シグネチャを測れなかった理由まで出る: {text}"
        );
        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない (LSP サーバを使えない) → どちらでもない"
            ),
            "呼び出し元を測れなかった理由まで出る: {text}"
        );
        assert!(
            text.contains("構造類似度: 0.94"),
            "測れたシグナルはそのまま値が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unrooted_project_says_what_to_place() {
        // 対照は上のテスト（サーバを使えない場合）。**印が無いのはサーバの都合ではなく
        // 利用者が直せる**ので、「測れない」で止めず次にすることまで出す。
        // 型シグネチャは印の有無に関わらず取れるので、落ちるのは呼び出し元だけ
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::ProjectUnrooted {
                markers: vec!["tsconfig.json".to_owned(), "jsconfig.json".to_owned()],
            },
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない \
                 (プロジェクトの印が見つからない: tsconfig.json / jsconfig.json \
                  のどれかを置くと参照元が揃う)"
            ),
            "印が無いことと、次にすることが出る: {text}"
        );
        assert!(
            text.contains("型シグネチャ: 単一化可能"),
            "型シグネチャは印の有無に関わらず出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_unrooted_project_without_markers_suggests_nothing_to_place() {
        // 対照は上のテスト。**印の名前が無いのに「置け」と言わない**
        // （そのサーバに無いファイルを勧めることになる）
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::ProjectUnrooted {
                markers: Vec::new(),
            },
        );

        assert!(
            text.contains("呼び出し元ドメインの重なりを測れない (プロジェクトの印が見つからない)"),
            "印が無いことだけを出す: {text}"
        );
        assert!(
            !text.contains("を置くと参照元が揃う"),
            "置くべきファイルの名前が無いのに勧めない: {text}"
        );
    }

    #[test]
    fn test_text_of_a_file_outside_the_project_says_to_widen_the_range_not_to_place_a_marker() {
        // 対照は「印が見つからない」テスト。**印はあるので「置け」では直らない**。
        // 印はあるが範囲外、という別の理由なのに同じ文を出すと、利用者は
        // 既に置いてあるファイルをもう一度置こうとする
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::OutsideProject {
                markers: vec!["tsconfig.json".to_owned(), "jsconfig.json".to_owned()],
            },
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない \
                 (プロジェクトの範囲から外れている: tsconfig.json / jsconfig.json \
                  が指す範囲を見直すと参照元が揃う)"
            ),
            "範囲から外れていることと、直す相手が出る: {text}"
        );
        assert!(
            !text.contains("プロジェクトの印が見つからない"),
            "印はあるので、置く話にしない: {text}"
        );
    }

    #[test]
    fn test_text_of_an_unverifiable_membership_does_not_blame_the_project_config() {
        // 対照は上のテスト（範囲外と確かめた場合）。**確かめる術が無いだけ**なので、
        // 範囲を直せとは言わない。直す先はサーバのほう
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::ProjectMembershipNotProvided,
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない (サーバがプロジェクトの所属を答えられない)"
            ),
            "確かめられなかったことが出る: {text}"
        );
        assert!(
            !text.contains("プロジェクトの範囲から外れている"),
            "確かめていないのに範囲外と言わない: {text}"
        );
    }

    #[test]
    fn test_text_of_a_file_outside_the_project_without_markers_names_no_file_to_fix() {
        // 対照は上のテスト。**印の名前が無いのに「あの印を直せ」と言わない**
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::OutsideProject {
                markers: Vec::new(),
            },
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない (プロジェクトの範囲から外れている)"
            ),
            "範囲から外れていることだけを出す: {text}"
        );
        assert!(
            !text.contains("が指す範囲を見直すと参照元が揃う"),
            "直す相手の名前が無いのに勧めない: {text}"
        );
    }

    #[test]
    fn test_text_of_with_an_undecidable_workspace_root_does_not_blame_the_lsp_server() {
        // 対照は上のテスト（サーバを使えない場合）。**尋ねる前に止まった理由を
        // 「LSP サーバを使えない」に畳むと、利用者が直す先を取り違える**
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::WorkspaceRootUndecidable,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::WorkspaceRootUndecidable,
            },
        );

        assert!(
            text.contains("型シグネチャ: 測れない (ワークスペースの根を決められない)"),
            "尋ねる前に止まった理由がそのまま出る: {text}"
        );
        assert!(
            !text.contains("LSP サーバを使えない"),
            "サーバのせいにしない: {text}"
        );
    }

    #[test]
    fn test_text_of_of_a_pair_below_the_threshold_says_it_did_not_ask() {
        // 尋ねなかったこと自体は出す。行ごと消すと、読む側は Stage 2 が
        // 効いたのか効かなかったのかを区別できない
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotACandidate,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotACandidate,
            },
        );

        assert!(
            text.contains("型シグネチャ: 尋ねていない (構造が似ておらず候補ペアではない)")
                && text.contains(
                    "呼び出し元ドメインの重なりを尋ねていない (構造が似ておらず候補ペアではない)"
                ),
            "尋ねなかった理由が両方の行に出る: {text}"
        );
    }

    #[test]
    fn test_text_of_without_asking_the_lsp_omits_the_stage2_lines() {
        // 対照は上のテスト。**尋ねていないことと測れなかったことを同じ出方にしない**。
        // 空欄やダミーで埋めると、読む側は測った結果としてそれを読む
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        assert!(
            !text.contains("型シグネチャ") && !text.contains("呼び出し元ドメイン"),
            "尋ねていない Stage 2 の行は出さない: {text}"
        );
        assert!(
            text.contains("依存先の重なり 0.00"),
            "Stage 1 の根拠はそのまま出る: {text}"
        );
    }

    #[test]
    fn test_text_of_reports_how_many_references_each_caller_domain_has() {
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_separate_domains(),
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なり 0.00 \
                 (/repo/src/billing 1件 <-> /repo/src/inventory 1件) → 共通化しない側"
            ),
            "重なりの値と、両側の分布と、傾きが 1 行で読める: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_threshold_beside_the_caller_domain_overlap() {
        // 対照は上のテスト。既定では同じ行に閾値が入らない
        let text = explained_text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_separate_domains(),
            Explanation::AllSignals,
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なり 0.00 (閾値 0.5) \
                 (/repo/src/billing 1件 <-> /repo/src/inventory 1件) → 共通化しない側"
            ),
            "当てた閾値が重なりの値と分布のあいだに出る: {text}"
        );
    }

    /// 構造が似ていて依存先を共有していない組に、呼び出し先だけを重ねて判定した text。
    fn text_of_accidental_duplication_with_callees(
        callee_domain_overlap: CalleeDomainOverlap,
        explanation: Explanation,
    ) -> String {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.94)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        )
        .with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        )
        .with_callee_domain_overlap(callee_domain_overlap);

        text_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(&signals, ConfiguredThresholds::default()),
            explanation,
        )
    }

    /// 呼び出し先が別のドメインに分かれている（重なり 0.00）。
    fn callees_in_separate_domains() -> CalleeDomainOverlap {
        let paths_a = [
            PathBuf::from("/repo/src/billing/invoice.ts"),
            PathBuf::from("/repo/src/billing/rate.ts"),
        ];
        let paths_b = [PathBuf::from("/repo/src/inventory/stock.ts")];
        let (Some(callees_a), Some(callees_b)) = (
            CalleeDomains::from_callee_paths(&paths_a, &DomainDeclarations::default())
                .ok()
                .flatten(),
            CalleeDomains::from_callee_paths(&paths_b, &DomainDeclarations::default())
                .ok()
                .flatten(),
        ) else {
            panic!("テストが渡す呼び出し先は 1 件以上");
        };

        CalleeDomainOverlap::Measured(MeasuredCalleeDomains::new(callees_a, callees_b))
    }

    #[test]
    fn test_text_of_reports_how_many_callees_each_callee_domain_has() {
        let text = text_of_accidental_duplication_with_callees(
            callees_in_separate_domains(),
            Explanation::AskedSignals,
        );

        assert!(
            text.contains(
                "呼び出し先ドメインの重なり 0.00 \
                 (/repo/src/billing 2件 <-> /repo/src/inventory 1件) → 共通化しない側"
            ),
            "重なりの値と、両側の分布と、傾きが 1 行で読める: {text}"
        );
    }

    #[test]
    fn test_explained_text_of_reports_the_threshold_beside_the_callee_domain_overlap() {
        // 対照は上のテスト。既定では同じ行に閾値が入らない
        let text = text_of_accidental_duplication_with_callees(
            callees_in_separate_domains(),
            Explanation::AllSignals,
        );

        assert!(
            text.contains(
                "呼び出し先ドメインの重なり 0.00 (閾値 0.5) \
                 (/repo/src/billing 2件 <-> /repo/src/inventory 1件) → 共通化しない側"
            ),
            "当てた閾値が重なりの値と分布のあいだに出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_only_external_callees_says_so_instead_of_calling_them_none() {
        // 言語の lib しか呼ばない関数を「呼び出し先が無い」と出すと、
        // 読む側は callHierarchy が取り損ねたと読む
        let text = text_of_accidental_duplication_with_callees(
            CalleeDomainOverlap::OnlyExternalCallees,
            Explanation::AskedSignals,
        );

        assert!(
            text.contains(
                "呼び出し先ドメインの重なりを測れない \
                 (呼び出し先がどれも依存パッケージ (node_modules) の中にある) → どちらでもない"
            ),
            "依存パッケージしか呼んでいないことが理由として出る: {text}"
        );
    }

    #[test]
    fn test_text_of_with_several_call_hierarchy_items_says_how_many_came_back() {
        let text = text_of_accidental_duplication_with_callees(
            CalleeDomainOverlap::SeveralCallHierarchyItems { count: 3 },
            Explanation::AskedSignals,
        );

        assert!(
            text.contains("呼び出し関係の起点が 3 個返り、どれか決められない"),
            "返った起点の数が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_without_call_hierarchy_says_the_server_lacks_it() {
        // callHierarchy を使えなかった判定は、残りのシグナルだけで出ている。
        // それを読む側が知れないと、判定の重みを誤る
        let text = text_of_accidental_duplication_with_callees(
            CalleeDomainOverlap::CallHierarchyNotProvided,
            Explanation::AskedSignals,
        );

        assert!(
            text.contains(
                "呼び出し先ドメインの重なりを測れない \
                 (サーバが callHierarchy を提供していない) → どちらでもない"
            ),
            "callHierarchy を使えなかったことが既定の出力にも出る: {text}"
        );
    }

    #[test]
    fn test_text_of_review_suggests_a_human_decision() {
        let text = text_of_separate_directories(
            StructuralSimilarity::Measured(measured(0.2)),
            ImportOverlap::Measured(measured(0.0)),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains("  提案: 判断材料が足りない。人が見て決める。"),
            "中間ケースの提案が出る: {text}"
        );
    }

    /// 層で分けた置き方の 2 つの参照元を、請求と在庫の宣言で数えた重なり。
    fn callers_in_separate_declared_domains() -> CallerDomainOverlap {
        let declarations = declarations_of(&[
            ("billing", &["src/**/invoice*.ts"]),
            ("inventory", &["src/**/product*.ts"]),
        ]);
        let callers = |path: &str| {
            CallerDomains::from_reference_paths(&[PathBuf::from(path)], &declarations)
                .expect("宣言は食い違わない")
                .expect("参照元は 1 件")
        };

        CallerDomainOverlap::Measured(MeasuredCallerDomains::new(
            callers("/repo/src/services/invoiceService.ts"),
            callers("/repo/src/services/productService.ts"),
        ))
    }

    #[test]
    fn test_text_of_marks_a_declared_caller_domain_as_declared() {
        // 印が無いと、宣言の名前と同じ綴りのディレクトリを見分けられない
        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::NoName,
            callers_in_separate_declared_domains(),
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なり 0.00 (billing (宣言) 1件 <-> inventory (宣言) 1件)"
            ),
            "宣言の名前で数えた分布が出る: {text}"
        );
    }

    #[test]
    fn test_text_of_names_the_file_and_the_two_declarations_a_reference_matched() {
        let ambiguous = declarations_of(&[
            ("billing", &["src/billing/**"]),
            ("reporting", &["src/**/report*.ts"]),
        ])
        .declared_domain_of(Path::new("/repo/src/billing/report.ts"))
        .expect_err("2 つの宣言に当たる");

        let text = text_of_accidental_duplication_with_semantics(
            TypeSignatureMatch::NoName,
            CallerDomainOverlap::AmbiguousDomain(ambiguous),
        );

        assert!(
            text.contains(
                "呼び出し元ドメインの重なりを測れない (参照元の /repo/src/billing/report.ts が \
                 dryguard.toml の 2 つの宣言 billing / reporting に当たる) → どちらでもない"
            ),
            "直す先（どのファイルがどの 2 つに当たったか）が出る: {text}"
        );
    }
}
