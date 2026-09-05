//! 判定と根拠を、人が読む text にする層。
//!
//! **文を組み立てるのはここだけ。** `reason` は「シグナルの値と、それが傾けた向き」の
//! 組であって文ではない（`rules/naming.md`「このツールの語彙を固定する」）。
//! 判定側が文を持つと、判定に効いた値と向きが文字列に埋もれて後段が読めなくなる。
//!
//! 判定はしない。ラベルと根拠を受け取って並べるだけで、シグナルからラベルを決めるのは
//! `classification` にしか無い（`rules/architecture.md`「判定は 1 箇所にだけ置く」）。

use crate::classification::Classification;
use crate::classification::reason::{Lean, Reason};
use crate::classification::signal::{
    CallerDomainOverlap, ImportOverlap, MeasuredCallerDomains, SemanticsUnavailable,
    StructuralSimilarity, TypeSignatureMatch,
};
use crate::classification::verdict::Verdict;
use crate::location::Location;
use crate::pipeline::{Scan, SkippedFile};
use crate::semantics::caller_domain::CallerDomains;
use crate::semantics::resolved_type::UnopenedReason;
use crate::syntax::module_distance::ModuleDistance;
use crate::threshold::Threshold;

/// 見出しの行に付ける字下げ。
const INDENT: &str = "  ";

/// `理由:` の 2 行目以降に付ける字下げ。
///
/// 見出し（`  理由: `）と同じ表示幅にして、根拠が縦に揃うようにしている。
const REASON_CONTINUATION_INDENT: &str = "        ";

/// 判定と根拠を、計画の出力イメージの形にする。
///
/// `structural_similarity_threshold` は判定に使った閾値で、構造類似度の行に併記する。
/// 併記しないと、`--threshold` の指定がどこに効いたのかが出力から読めない。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
///
/// Stage 2（LSP）が要る行——型シグネチャ・呼び出し元の分布——は、**尋ねたときだけ**出す。
/// 尋ねていないものを空欄やダミーで埋めずに行ごと出さない
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// **尋ねて取れなかったときは理由まで出す**（環境が悪いのか材料が無いのかで、
/// 読者が次にすることが違う）。
pub fn text_of(
    location_a: &Location,
    location_b: &Location,
    classification: &Classification,
    structural_similarity_threshold: Threshold,
) -> String {
    let mut lines = vec![format!(
        "[{}] {location_a} <-> {location_b}",
        classification.verdict()
    )];
    let mut reason_texts = Vec::new();

    for reason in classification.reasons() {
        match reason {
            Reason::StructuralSimilarity { signal, lean } => lines.push(format!(
                "{INDENT}構造類似度: {} → {}",
                structural_similarity_text_of(*signal, structural_similarity_threshold),
                lean_text_of(*lean)
            )),
            Reason::ImportOverlap { signal, lean } => reason_texts.push(format!(
                "{} → {}",
                import_overlap_text_of(*signal),
                lean_text_of(*lean)
            )),
            Reason::ModuleDistance { signal, lean } => reason_texts.push(format!(
                "{} → {}",
                module_distance_text_of(*signal),
                lean_text_of(*lean)
            )),
            Reason::TypeSignatureMatch { signal, lean } => {
                lines.extend(type_signature_text_of(*signal).map(|signal_text| {
                    format!(
                        "{INDENT}型シグネチャ: {signal_text} → {}",
                        lean_text_of(*lean)
                    )
                }));
            }
            Reason::CallerDomainOverlap { signal, lean } => {
                reason_texts.extend(
                    caller_domain_overlap_text_of(signal)
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
/// `structural_similarity_threshold` は候補を絞るのと判定に使った閾値で、
/// ペアごとの行に併記する。
///
/// 候補ペアは [`text_of`] と同じ形で並べる。**`compare` と `scan` で同じペアの
/// 見え方が変わると、片方で見た結果をもう片方で確かめられない。**
///
/// 飛ばしたファイルと切り出せなかった関数は、あるときだけ節ごと出す。
/// 空の見出しを残すと、読む側は「何かを飛ばした」と読む。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
pub fn scan_text_of(scan: &Scan, structural_similarity_threshold: Threshold) -> String {
    let mut blocks: Vec<String> = scan
        .candidate_pairs()
        .iter()
        .map(|pair| {
            text_of(
                pair.location_a(),
                pair.location_b(),
                pair.classification(),
                structural_similarity_threshold,
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
fn import_overlap_text_of(signal: ImportOverlap) -> String {
    match signal {
        ImportOverlap::Measured(overlap) => format!("依存先の重なり {overlap}"),
        ImportOverlap::NoImports => {
            "依存先の重なりを測れない (import が無いファイルがある)".to_owned()
        }
    }
}

/// モジュール距離の値。段数は必ず取れるので、測れなかった形にはならない。
fn module_distance_text_of(distance: ModuleDistance) -> String {
    format!("モジュール距離 {} 段", distance.steps())
}

/// 型シグネチャの単一化の可否。測れていなければ、その理由。
///
/// LSP に尋ねていないときだけ `None` を返し、**行ごと出さない**。空欄やダミーで
/// 埋めると、読む側は測った結果としてそれを読む
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// **測れなかったのとは別**なので、そちらは理由まで出す。
fn type_signature_text_of(signal: TypeSignatureMatch) -> Option<&'static str> {
    match signal {
        TypeSignatureMatch::Unifiable => Some("単一化可能"),
        TypeSignatureMatch::NotUnifiable => Some("単一化不能"),
        TypeSignatureMatch::Unavailable { reason } => semantics_unavailable_text_of(reason),
        TypeSignatureMatch::NoName => Some("測れない (チャンクが名前を持たない)"),
        TypeSignatureMatch::NoTypeThere => Some("測れない (サーバがその位置に型を持たない)"),
        TypeSignatureMatch::UnreadableHover => Some("測れない (hover の応答を読めない)"),
        TypeSignatureMatch::UnreadableSignature => Some("測れない (返った綴りを読み解けない)"),
        TypeSignatureMatch::HoverNotProvided => Some("測れない (サーバが hover を提供していない)"),
        TypeSignatureMatch::UnopenedTypeName { reason } => Some(unopened_text_of(reason)),
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
    }
}

/// 呼び出し元ドメインの重なりの値と分布。測れていなければ、その理由。
///
/// 尋ねていないときに `None` を返すのは [`type_signature_text_of`] と同じ理由。
fn caller_domain_overlap_text_of(signal: &CallerDomainOverlap) -> Option<String> {
    let unmeasured = match signal {
        CallerDomainOverlap::Measured(measured) => {
            return Some(measured_caller_domains_text_of(measured));
        }
        CallerDomainOverlap::Unavailable { reason } => {
            let unavailable = semantics_unavailable_text_of(*reason)?;

            return Some(format!("呼び出し元ドメインの重なりを{unavailable}"));
        }
        CallerDomainOverlap::NoName => "チャンクが名前を持たない",
        CallerDomainOverlap::NoReferences => "参照元が 1 件も返らない",
        CallerDomainOverlap::UnreadableReferences => "読めない URI が混じっている",
        CallerDomainOverlap::ServerStillWorking => "サーバが作業中で答えが落ち着かない",
        CallerDomainOverlap::ReferencesNotProvided => "サーバが references を提供していない",
    };

    Some(format!(
        "呼び出し元ドメインの重なりを測れない ({unmeasured})"
    ))
}

/// Stage 2 へ届かなかったことを表す文。
///
/// **尋ねていないだけなら `None`** を返し、行ごと出さない。空欄やダミーで埋めると、
/// 読む側は測った結果としてそれを読む
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
///
/// 頭を「測れない」「尋ねていない」に揃えてあるのは、呼び出し元ドメイン側が
/// `…の重なりを` の後ろに続けて使うため。
fn semantics_unavailable_text_of(reason: SemanticsUnavailable) -> Option<&'static str> {
    match reason {
        SemanticsUnavailable::NotAsked => None,
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

/// 測れた重なりと、両側のドメインごとの件数。
fn measured_caller_domains_text_of(measured: &MeasuredCallerDomains) -> String {
    format!(
        "呼び出し元ドメインの重なり {} ({} <-> {})",
        measured.overlap(),
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
        .map(|(domain, count)| format!("{} {count}件", domain.directory().display()))
        .collect::<Vec<String>>()
        .join(" / ")
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
    use std::path::{Path, PathBuf};

    use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
    use crate::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
    use crate::location::Location;
    use crate::pipeline::{Scan, scan_of};
    use crate::similarity::Similarity;
    use crate::syntax::module_distance::ModuleDistance;
    use crate::test_support::{line, missing_server};
    use crate::threshold::Threshold;

    fn location(path: &str, number: usize) -> Location {
        Location::new(PathBuf::from(path), line(number))
    }

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

    /// 別のディレクトリにある 2 箇所を、渡した閾値で判定した text。
    ///
    /// 距離を固定して、構造の似かたと依存先の重なりだけを動かす。判定に渡す閾値と
    /// 表示に渡す閾値を揃えているのは、`compare` が同じ値を両方へ渡すため。
    fn text_of_separate_directories(
        structural_similarity: StructuralSimilarity,
        import_overlap: ImportOverlap,
        threshold: Threshold,
    ) -> String {
        let signals = Signals::new(
            structural_similarity,
            import_overlap,
            separate_directories(),
        );

        text_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(&signals, threshold),
            threshold,
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
            ImportOverlap::NoImports,
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        );

        assert!(
            text.contains(
                "依存先の重なりを測れない (import が無いファイルがある) → どちらでもない"
            ),
            "測れなかった理由まで出る: {text}"
        );
        assert!(
            text.contains("構造類似度: 0.94"),
            "測れたシグナルはそのまま値が出る: {text}"
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

    /// `tests/fixtures/` 配下のディレクトリを走査した結果。
    ///
    /// カレントディレクトリではなくマニフェストの位置から組み立てる
    /// （テストの実行位置に依存させない）。
    ///
    /// **起動できないサーバを渡す。** 実サーバを要する形にすると、サーバの入っていない
    /// 開発機で出力が変わる（`rules/testing.md`「LSP を要するテストは、飛ばしたことが
    /// 分かる形にする」）。
    fn scan_of_fixture(relative_path: &str, threshold: Threshold) -> Scan {
        let root = PathBuf::from(format!(
            "{}/tests/fixtures/{relative_path}",
            env!("CARGO_MANIFEST_DIR")
        ));

        scan_of(&root, threshold, &missing_server())
            .expect("フィクスチャのディレクトリは走査できる")
    }

    /// 候補ペアが 1 組だけ出るフィクスチャの text。
    fn scan_text_of_fixture() -> String {
        let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;

        scan_text_of(&scan_of_fixture("scan", threshold), threshold)
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
        let threshold = Threshold::from_literal(0.0);
        let scan = scan_of_fixture("scan", threshold);

        let text = scan_text_of(&scan, threshold);

        let verdict_lines = text.lines().filter(|line| line.starts_with('[')).count();
        assert_eq!(verdict_lines, 9, "比べた 9 ペアが並ぶ: {text}");
        assert!(text.contains("\n\n["), "ペアとペアの間に空行が入る: {text}");
    }

    #[test]
    fn test_scan_text_of_reports_how_much_of_the_codebase_it_walked() {
        let text = scan_text_of_fixture();

        assert!(
            text.contains(
                "対象 6 ファイル / チャンク 5 件 / 比較 9 ペア（うち長さで確定 3 ペア）/ 候補 1 ペア"
            ),
            "走査した量と、突き合わせを省いた内訳が読める: {text}"
        );
    }

    #[test]
    fn test_scan_text_of_reports_a_file_it_could_not_read() {
        let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;

        let text = scan_text_of(&scan_of_fixture("scan-skipped", threshold), threshold);

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
        let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;

        let text = scan_text_of(&scan_of_fixture("scan-skipped", threshold), threshold);

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
            CallerDomains::from_reference_paths(&paths_a),
            CallerDomains::from_reference_paths(&paths_b),
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
            &classification_of(&signals, threshold),
            threshold,
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
}
