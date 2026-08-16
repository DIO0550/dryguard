//! ソース上の行範囲。

use std::fmt;
use std::num::NonZeroUsize;

/// 1 始まりの行範囲。両端を含む。
///
/// 開始行と終了行を素の 2 つの値で持ち回すと「終了行が開始行より手前」という
/// 存在しない範囲が作れてしまうため、生成時に確かめて閉じる
/// (rules/coding.md「不正な状態を型で表現できなくする」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    start: NonZeroUsize,
    end: NonZeroUsize,
}

impl LineRange {
    /// 前後を確かめて行範囲を作る。
    ///
    /// `end` が `start` より手前のときは作れないので `None` を返す。
    /// 1 行だけの範囲は `start` と `end` が同じ値になる。
    pub fn new(start: NonZeroUsize, end: NonZeroUsize) -> Option<Self> {
        if end < start {
            return None;
        }
        Some(Self { start, end })
    }

    /// 開始行と、そこから続く行数で範囲を作る。
    ///
    /// `additional_lines` は開始行の**次の行から**数える。0 なら 1 行だけの範囲。
    ///
    /// 前後が入れ替わりようがないので、[`LineRange::new`] と違って失敗しない。
    /// 「開始位置と長さ」の形で範囲が決まる呼び出し側が `Option` を開く必要をなくす
    /// （起こりえない `None` の分岐は、読む側にありうる失敗だと誤解させる）。
    pub fn starting_at(start: NonZeroUsize, additional_lines: usize) -> Self {
        Self {
            start,
            end: start.saturating_add(additional_lines),
        }
    }

    /// 開始行。
    pub fn start(self) -> NonZeroUsize {
        self.start
    }

    /// 終了行。
    pub fn end(self) -> NonZeroUsize {
        self.end
    }

    /// その行が範囲に入っているか。両端は含む。
    pub fn contains(self, line: NonZeroUsize) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}-{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::line;

    #[test]
    fn test_line_range_with_start_before_end_is_created() {
        let range = LineRange::new(line(10), line(20)).expect("start < end なので作れる");

        assert_eq!(range.start(), line(10));
        assert_eq!(range.end(), line(20));
    }

    #[test]
    fn test_line_range_of_a_single_line_is_created() {
        assert!(
            LineRange::new(line(7), line(7)).is_some(),
            "1 行だけの範囲は start == end で表す"
        );
    }

    #[test]
    fn test_line_range_with_end_before_start_cannot_be_created() {
        assert_eq!(LineRange::new(line(20), line(10)), None);
    }

    #[test]
    fn test_line_range_starting_at_a_line_without_more_lines_ends_at_the_same_line() {
        let range = LineRange::starting_at(line(7), 0);

        assert_eq!(range.start(), line(7));
        assert_eq!(range.end(), line(7), "続く行が 0 なら 1 行だけの範囲");
    }

    #[test]
    fn test_line_range_starting_at_a_line_counts_the_rest_from_the_next_line() {
        let range = LineRange::starting_at(line(3), 3);

        assert_eq!(range.end(), line(6));
    }

    #[test]
    fn test_line_range_contains_a_line_between_both_ends() {
        let range = LineRange::new(line(10), line(20)).expect("作れる");

        assert!(range.contains(line(15)));
    }

    #[test]
    fn test_line_range_contains_both_ends() {
        let range = LineRange::new(line(10), line(20)).expect("作れる");

        assert!(range.contains(line(10)), "開始行は範囲に含む");
        assert!(range.contains(line(20)), "終了行は範囲に含む");
    }

    #[test]
    fn test_line_range_does_not_contain_a_line_outside_both_ends() {
        let range = LineRange::new(line(10), line(20)).expect("作れる");

        assert!(!range.contains(line(9)));
        assert!(!range.contains(line(21)));
    }

    #[test]
    fn test_line_range_displays_as_start_and_end() {
        let range = LineRange::new(line(10), line(20)).expect("作れる");

        assert_eq!(range.to_string(), "10-20");
    }
}
