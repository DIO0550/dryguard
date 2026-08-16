//! ソース上の行番号。

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;

/// 1 始まりの行番号。
///
/// 素の [`NonZeroUsize`] にしないのは、それでは「0 でない」しか言えないため。
/// **0 始まりのインデックスと 1 始まりの行番号の変換**は使う側の至る所で要り、
/// 置き場所が決まっていないと同じ変換がその場ごとに書かれる
/// (rules/coding.md「標準の型が『その値の意味』まで言えないなら newtype にする」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineNumber(NonZeroUsize);

impl LineNumber {
    /// 1 以上を確かめて行番号を作る。
    ///
    /// 0 行目という位置は存在しないので、0 のときは作れず `None` を返す。
    pub fn new(value: usize) -> Option<Self> {
        NonZeroUsize::new(value).map(Self)
    }

    /// 0 始まりの行インデックスから作る。
    ///
    /// インデックス 0 が 1 行目。ずらす先が 1 以上になるので失敗しない。
    pub fn from_index(index: usize) -> Self {
        Self(NonZeroUsize::MIN.saturating_add(index))
    }

    /// 0 始まりの行インデックス。行の並びを添字で引くときに使う。
    pub fn to_index(self) -> usize {
        self.0.get() - 1
    }

    /// 1 始まりの行番号そのもの。
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl fmt::Display for LineNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for LineNumber {
    type Err = LineNumberParseError;

    /// 行番号を解釈する。
    ///
    /// # Errors
    ///
    /// 数値として読めない / 0 のとき。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value: usize = text
            .parse()
            .map_err(|_| LineNumberParseError::NotANumber(text.to_owned()))?;

        Self::new(value).ok_or(LineNumberParseError::Zero)
    }
}

/// [`LineNumber`] の解釈が失敗した理由。
///
/// 「数値でない」と「0 行目」を分けているのは、利用者が直す先が違うため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineNumberParseError {
    /// 数値として読めない。保持しているのは読めなかった文字列。
    NotANumber(String),
    /// 0 が指定された。行は 1 始まり。
    Zero,
}

impl fmt::Display for LineNumberParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber(text) => {
                write!(formatter, "行番号が数値ではありません: {text}")
            }
            Self::Zero => write!(formatter, "行番号は 1 以上を指定してください"),
        }
    }
}

impl Error for LineNumberParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_number_of_a_positive_value_keeps_it() {
        let line = LineNumber::new(42).expect("1 以上なので作れる");

        assert_eq!(line.get(), 42);
    }

    #[test]
    fn test_line_number_of_one_is_created() {
        assert!(LineNumber::new(1).is_some(), "1 行目は存在する");
    }

    #[test]
    fn test_line_number_of_zero_cannot_be_created() {
        assert_eq!(LineNumber::new(0), None);
    }

    #[test]
    fn test_line_number_from_the_first_index_is_the_first_line() {
        assert_eq!(LineNumber::from_index(0).get(), 1);
    }

    #[test]
    fn test_line_number_from_an_index_counts_from_one() {
        assert_eq!(LineNumber::from_index(41).get(), 42);
    }

    #[test]
    fn test_line_number_to_index_counts_from_zero() {
        let line = LineNumber::new(42).expect("作れる");

        assert_eq!(line.to_index(), 41);
    }

    #[test]
    fn test_line_number_of_the_first_line_to_index_is_zero() {
        let line = LineNumber::new(1).expect("作れる");

        assert_eq!(line.to_index(), 0);
    }

    #[test]
    fn test_line_number_parsed_from_digits_keeps_the_value() {
        let line: LineNumber = "42".parse().expect("解釈できる");

        assert_eq!(line.get(), 42);
    }

    #[test]
    fn test_line_number_parsed_from_non_numeric_text_returns_not_a_number() {
        let result = "abc".parse::<LineNumber>();

        assert_eq!(
            result,
            Err(LineNumberParseError::NotANumber("abc".to_owned()))
        );
    }

    #[test]
    fn test_line_number_parsed_from_zero_returns_zero() {
        // 「数値として読めない」と「0 行目」は利用者が直す先が違う
        let result = "0".parse::<LineNumber>();

        assert_eq!(result, Err(LineNumberParseError::Zero));
    }

    #[test]
    fn test_line_number_displays_as_the_number() {
        let line = LineNumber::new(42).expect("作れる");

        assert_eq!(line.to_string(), "42");
    }

    #[test]
    fn test_line_number_compares_by_the_number() {
        let earlier = LineNumber::new(10).expect("作れる");
        let later = LineNumber::new(20).expect("作れる");

        assert!(earlier < later);
    }
}
