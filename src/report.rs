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
use crate::classification::signal::{ImportOverlap, StructuralSimilarity};
use crate::classification::verdict::Verdict;
use crate::location::Location;
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
/// Stage 2（LSP）が要る行——型シグネチャ・呼び出し元の分布——は出さない。
/// Phase 0 では測っていないので、空欄やダミーで埋めずに行ごと出さない
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
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
        }
    }

    lines.extend(reason_lines_of(&reason_texts));
    lines.push(format!(
        "{INDENT}提案: {}",
        suggestion_of(classification.verdict())
    ));

    lines.join("\n")
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
    use crate::similarity::Similarity;
    use crate::syntax::module_distance::ModuleDistance;
    use crate::test_support::line;
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
