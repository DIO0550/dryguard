//! ソース上の位置。CLI が `file:line` の形で受け取る。

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// ソース上の位置。
///
/// 行番号を [`NonZeroUsize`] で持つのは、0 行目という位置が存在しないため。ここで弾いておくと、
/// 後段が「0 かもしれない」を考えなくて済む
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    path: PathBuf,
    line: NonZeroUsize,
}

impl Location {
    /// 位置を組み立てる。
    ///
    /// `path` は対象ファイルのパス、`line` は 1 始まりの行番号。
    pub fn new(path: PathBuf, line: NonZeroUsize) -> Self {
        Self { path, line }
    }

    /// 対象ファイルのパス。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 1 始まりの行番号。
    pub fn line(&self) -> NonZeroUsize {
        self.line
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

impl FromStr for Location {
    type Err = LocationParseError;

    /// `file:line` を解釈する。
    ///
    /// # Errors
    ///
    /// `:` が無い / パスが空 / 行番号が数値でない / 行番号が 0 のとき。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // 右から分割する。左からだと Windows のドライブレター (`C:\a.rs:12`) を
        // 区切りと取り違える。
        let (path, line) = text
            .rsplit_once(':')
            .ok_or(LocationParseError::MissingSeparator)?;

        if path.is_empty() {
            return Err(LocationParseError::EmptyPath);
        }

        let number: usize = line
            .parse()
            .map_err(|_| LocationParseError::NotANumber(line.to_owned()))?;
        let line = NonZeroUsize::new(number).ok_or(LocationParseError::ZeroLine)?;

        Ok(Self::new(PathBuf::from(path), line))
    }
}

/// [`Location`] の解釈が失敗した理由。
///
/// 「数値でない」と「0 行目」を分けているのは、利用者が直す先が違うため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationParseError {
    /// `file:line` の `:` が無い。
    MissingSeparator,
    /// `:` の左が空。
    EmptyPath,
    /// `:` の右が数値として読めない。保持しているのは読めなかった文字列。
    NotANumber(String),
    /// 行番号が 0。行は 1 始まり。
    ZeroLine,
}

impl fmt::Display for LocationParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSeparator => {
                write!(formatter, "位置は file:line の形で指定してください")
            }
            Self::EmptyPath => write!(formatter, "ファイルパスが空です"),
            Self::NotANumber(text) => {
                write!(formatter, "行番号が数値ではありません: {text}")
            }
            Self::ZeroLine => write!(formatter, "行番号は 1 以上を指定してください"),
        }
    }
}

impl Error for LocationParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::line;

    #[test]
    fn test_location_with_path_and_line_parses_both() {
        let location: Location = "src/billing/discount.ts:42".parse().expect("解釈できる");

        assert_eq!(location.path(), Path::new("src/billing/discount.ts"));
        assert_eq!(location.line(), line(42));
    }

    #[test]
    fn test_location_with_windows_drive_letter_keeps_drive_in_path() {
        // 左から分割していると `C` がパス、`\a.rs:12` が行番号になって落ちる
        let location: Location = r"C:\src\a.rs:12".parse().expect("解釈できる");

        assert_eq!(location.path(), Path::new(r"C:\src\a.rs"));
        assert_eq!(location.line(), line(12));
    }

    #[test]
    fn test_location_without_separator_returns_missing_separator() {
        let result = "src/a.ts".parse::<Location>();

        assert_eq!(result, Err(LocationParseError::MissingSeparator));
    }

    #[test]
    fn test_location_with_empty_path_returns_empty_path() {
        let result = ":42".parse::<Location>();

        assert_eq!(result, Err(LocationParseError::EmptyPath));
    }

    #[test]
    fn test_location_with_non_numeric_line_returns_not_a_number() {
        let result = "src/a.ts:abc".parse::<Location>();

        assert_eq!(
            result,
            Err(LocationParseError::NotANumber("abc".to_owned()))
        );
    }

    #[test]
    fn test_location_with_zero_line_returns_zero_line() {
        let result = "src/a.ts:0".parse::<Location>();

        assert_eq!(result, Err(LocationParseError::ZeroLine));
    }

    #[test]
    fn test_location_displays_as_file_and_line() {
        let location = Location::new(PathBuf::from("src/a.ts"), line(7));

        assert_eq!(location.to_string(), "src/a.ts:7");
    }
}
