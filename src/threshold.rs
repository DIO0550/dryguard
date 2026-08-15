//! 判定に使う閾値。

use std::error::Error;
use std::fmt;
use std::str::FromStr;

/// 0.0 以上 1.0 以下の閾値。
///
/// 素の `f64` にしないのは、範囲外や NaN を後段が受け取らないようにするため。
/// 生成できた時点で範囲を満たしていることが保証される
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Threshold(f64);

impl Threshold {
    /// 範囲を確かめて閾値を作る。
    ///
    /// 0.0 未満・1.0 超過・NaN のときは作れないので `None` を返す。
    pub fn new(value: f64) -> Option<Self> {
        // NaN はどの比較でも false になるため、範囲の比較だけでは弾けない
        if value.is_nan() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        Some(Self(value))
    }

    /// 閾値そのもの。
    pub fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Threshold {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for Threshold {
    type Err = ThresholdParseError;

    /// 閾値を解釈する。
    ///
    /// # Errors
    ///
    /// 数値として読めない / 0.0-1.0 の範囲外のとき。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value: f64 = text
            .parse()
            .map_err(|_| ThresholdParseError::NotANumber(text.to_owned()))?;
        Self::new(value).ok_or(ThresholdParseError::OutOfRange(value))
    }
}

/// [`Threshold`] の解釈が失敗した理由。
#[derive(Debug, Clone, PartialEq)]
pub enum ThresholdParseError {
    /// 数値として読めない。保持しているのは読めなかった文字列。
    NotANumber(String),
    /// 0.0-1.0 の範囲外。保持しているのは読めたが使えなかった値。
    OutOfRange(f64),
}

impl fmt::Display for ThresholdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber(text) => write!(formatter, "閾値が数値ではありません: {text}"),
            Self::OutOfRange(value) => {
                write!(
                    formatter,
                    "閾値は 0.0 から 1.0 の範囲で指定してください: {value}"
                )
            }
        }
    }
}

impl Error for ThresholdParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threshold_within_range_keeps_the_value() {
        let threshold = Threshold::new(0.85).expect("範囲内なので作れる");

        assert_eq!(threshold.value(), 0.85);
    }

    #[test]
    fn test_threshold_at_both_bounds_is_accepted() {
        assert!(Threshold::new(0.0).is_some(), "0.0 は範囲に含む");
        assert!(Threshold::new(1.0).is_some(), "1.0 は範囲に含む");
    }

    #[test]
    fn test_threshold_below_zero_cannot_be_created() {
        assert_eq!(Threshold::new(-0.1), None);
    }

    #[test]
    fn test_threshold_above_one_cannot_be_created() {
        assert_eq!(Threshold::new(1.1), None);
    }

    #[test]
    fn test_threshold_of_nan_cannot_be_created() {
        // NaN はどの比較でも false になるので、範囲の比較だけでは通り抜ける
        assert_eq!(Threshold::new(f64::NAN), None);
    }

    #[test]
    fn test_threshold_parsed_from_decimal_text_keeps_the_value() {
        let threshold: Threshold = "0.9".parse().expect("解釈できる");

        assert_eq!(threshold.value(), 0.9);
    }

    #[test]
    fn test_threshold_parsed_from_non_numeric_text_returns_not_a_number() {
        let result = "high".parse::<Threshold>();

        assert_eq!(
            result,
            Err(ThresholdParseError::NotANumber("high".to_owned()))
        );
    }

    #[test]
    fn test_threshold_parsed_from_out_of_range_number_returns_out_of_range() {
        let result = "1.5".parse::<Threshold>();

        assert_eq!(result, Err(ThresholdParseError::OutOfRange(1.5)));
    }
}
