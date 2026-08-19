//! 判定の結果として返すラベル。

use std::fmt;

/// このツールが返す 3 つのラベル。
///
/// 3 つで閉じているので `String` にしない。文字列で持つと、綴りの違う同じラベルが
/// 生まれ、`--fail-on` の突き合わせが黙って外れる
/// (rules/coding.md「値の語彙を型で閉じる」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 構造・依存ドメインが一致している。共通化してよい。
    ExtractCandidate,
    /// 構造は似ているが依存先のドメインが別。偶発的な重複。
    DoNotExtract,
    /// 中間ケース。人間の判断が要る。
    Review,
}

impl fmt::Display for Verdict {
    /// `docs/dryguard-plan.md` の出力イメージと同じ綴りで書く。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ExtractCandidate => "EXTRACT-CANDIDATE",
            Self::DoNotExtract => "DO-NOT-EXTRACT",
            Self::Review => "REVIEW",
        };
        formatter.write_str(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verdict_of_extract_candidate_displays_the_planned_label() {
        assert_eq!(Verdict::ExtractCandidate.to_string(), "EXTRACT-CANDIDATE");
    }

    #[test]
    fn test_verdict_of_do_not_extract_displays_the_planned_label() {
        assert_eq!(Verdict::DoNotExtract.to_string(), "DO-NOT-EXTRACT");
    }

    #[test]
    fn test_verdict_of_review_displays_the_planned_label() {
        assert_eq!(Verdict::Review.to_string(), "REVIEW");
    }
}
