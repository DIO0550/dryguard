//! シグナルとして測った類似度。

use std::fmt;

use crate::threshold::Threshold;

/// 0.0 以上 1.0 以下の類似度。1.0 が完全一致。
///
/// [`crate::threshold::Threshold`] と同じ範囲だが別の型にする。**測った値**と
/// **比較する基準**は取り違えても素の `f64` では型エラーにならない
/// (rules/coding.md「スコアと閾値を素の `f64` で混ぜない」)。
///
/// `syntax` の中ではなくここに置くのは、Stage 2 が出す型シグネチャの類似度も
/// 同じ型で受けるため。ステージごとに別の型があると、`classification` が
/// シグナルの出どころごとに違う型を扱うことになる。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Similarity(f64);

impl Similarity {
    /// 範囲を確かめて類似度を作る。
    ///
    /// 0.0 未満・1.0 超過・NaN のときは作れないので `None` を返す。
    pub fn new(value: f64) -> Option<Self> {
        // NaN はどの比較でも false になるため、範囲の比較だけでは弾けない
        if value.is_nan() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        Some(Self(value))
    }

    /// 全体 `combined` 個のうち `shared` 個が共通しているときの類似度。
    ///
    /// `shared` は `combined` の一部なので、比は必ず 0.0-1.0 に収まる。
    /// [`Similarity::new`] と違って失敗しないのは、**起こりえない `None` の分岐を
    /// 呼び出し側に開かせない**ため（ありうる失敗だと誤解させる）。
    ///
    /// `combined` が 0 のときは 1.0。どちらにも要素が無いなら違いも無い
    /// （Jaccard 係数の慣習に合わせる）。
    pub(crate) fn from_shared_count(shared: usize, combined: usize) -> Self {
        if combined == 0 {
            return Self(1.0);
        }
        Self(shared as f64 / combined as f64)
    }

    /// 類似度そのもの。丸めない値が要るときに使う。
    pub fn value(self) -> f64 {
        self.0
    }

    /// 閾値に届いているか（閾値と同じ値なら届いている）。
    ///
    /// 比較をここに置くのは、**測った値と基準を突き合わせる操作の置き場所**が
    /// 他に無いため。呼び出し側で `similarity.value() >= threshold.value()` と
    /// 書かせると、素の `f64` に戻った時点で 2 つを型で分けた意味が消える。
    pub fn is_at_least(self, threshold: Threshold) -> bool {
        self.0 >= threshold.value()
    }
}

impl fmt::Display for Similarity {
    /// 小数第 2 位まで表示する。
    ///
    /// 割り算の結果をそのまま出すと `0.9411764705882353` になる。桁を増やしても
    /// 読む側が判断できることは増えず、入力が少し変わるたびに末尾が動いて
    /// 出力の差分が読みにくくなる。丸めない値は [`Similarity::value`] で取れる。
    ///
    /// Why not（`Threshold` に揃えて丸めない）: 閾値は利用者が指定した値を返す表示で、
    /// 打った通りに出るほうが確かめやすい。こちらは計算結果なので事情が違う。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:.2}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_within_range_keeps_the_value() {
        let similarity = Similarity::new(0.91).expect("範囲内なので作れる");

        assert_eq!(similarity.value(), 0.91);
    }

    #[test]
    fn test_similarity_at_both_bounds_is_accepted() {
        assert!(Similarity::new(0.0).is_some(), "0.0 は範囲に含む");
        assert!(Similarity::new(1.0).is_some(), "1.0 は範囲に含む");
    }

    #[test]
    fn test_similarity_below_zero_cannot_be_created() {
        assert_eq!(Similarity::new(-0.1), None);
    }

    #[test]
    fn test_similarity_above_one_cannot_be_created() {
        assert_eq!(Similarity::new(1.1), None);
    }

    #[test]
    fn test_similarity_of_nan_cannot_be_created() {
        assert_eq!(Similarity::new(f64::NAN), None);
    }

    #[test]
    fn test_similarity_from_a_part_of_the_whole_is_the_ratio() {
        let similarity = Similarity::from_shared_count(1, 4);

        assert_eq!(similarity.value(), 0.25);
    }

    #[test]
    fn test_similarity_from_nothing_shared_of_nothing_is_one() {
        // どちらにも要素が無いなら違いも無い（Jaccard 係数の慣習）
        let similarity = Similarity::from_shared_count(0, 0);

        assert_eq!(similarity.value(), 1.0);
    }

    #[test]
    fn test_similarity_above_the_threshold_reaches_it() {
        let similarity = Similarity::new(0.9).expect("作れる");
        let threshold = Threshold::new(0.85).expect("作れる");

        assert!(similarity.is_at_least(threshold));
    }

    #[test]
    fn test_similarity_below_the_threshold_does_not_reach_it() {
        let similarity = Similarity::new(0.84).expect("作れる");
        let threshold = Threshold::new(0.85).expect("作れる");

        assert!(!similarity.is_at_least(threshold));
    }

    #[test]
    fn test_similarity_equal_to_the_threshold_reaches_it() {
        // 境界。閾値ちょうどを弾くと、閾値に指定した値そのものが判定に出てこない
        let similarity = Similarity::new(0.85).expect("作れる");
        let threshold = Threshold::new(0.85).expect("作れる");

        assert!(similarity.is_at_least(threshold));
    }

    #[test]
    fn test_similarity_displays_with_two_decimals() {
        let similarity = Similarity::new(0.9411764705882353).expect("作れる");

        assert_eq!(similarity.to_string(), "0.94");
    }
}
