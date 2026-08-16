//! ソース上の位置。CLI が `file:line` の形で受け取る。

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::line_number::{LineNumber, LineNumberParseError};

/// ソース上の位置。
///
/// 行番号を [`LineNumber`] で持つので、0 行目という位置は組み立てられない。
/// 後段が「0 かもしれない」を考えなくて済む
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    path: PathBuf,
    line: LineNumber,
}

impl Location {
    /// 位置を組み立てる。
    ///
    /// `path` は対象ファイルのパス、`line` はその行番号。
    pub fn new(path: PathBuf, line: LineNumber) -> Self {
        Self { path, line }
    }

    /// 対象ファイルのパス。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 行番号。
    pub fn line(&self) -> LineNumber {
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
    /// `:` が無い / パスが空 / `:` の右を行番号として読めないとき。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // 右から分割する。左からだと Windows のドライブレター (`C:\a.rs:12`) を
        // 区切りと取り違える。
        let (path, line) = text
            .rsplit_once(':')
            .ok_or(LocationParseError::MissingSeparator)?;

        if path.is_empty() {
            return Err(LocationParseError::EmptyPath);
        }

        let line = line.parse().map_err(LocationParseError::Line)?;

        Ok(Self::new(PathBuf::from(path), line))
    }
}

/// [`Location`] の解釈が失敗した理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationParseError {
    /// `file:line` の `:` が無い。
    MissingSeparator,
    /// `:` の左が空。
    EmptyPath,
    /// `:` の右を行番号として読めない。
    ///
    /// 理由の内訳を自前で持たず [`LineNumberParseError`] をそのまま抱えるのは、
    /// 同じ分類を 2 箇所に置くと片方だけ古くなるため。
    Line(LineNumberParseError),
}

impl fmt::Display for LocationParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSeparator => {
                write!(formatter, "位置は file:line の形で指定してください")
            }
            Self::EmptyPath => write!(formatter, "ファイルパスが空です"),
            Self::Line(cause) => write!(formatter, "{cause}"),
        }
    }
}

impl Error for LocationParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MissingSeparator | Self::EmptyPath => None,
            Self::Line(cause) => Some(cause),
        }
    }
}

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
            Err(LocationParseError::Line(LineNumberParseError::NotANumber(
                "abc".to_owned()
            )))
        );
    }

    #[test]
    fn test_location_with_zero_line_returns_zero() {
        let result = "src/a.ts:0".parse::<Location>();

        assert_eq!(
            result,
            Err(LocationParseError::Line(LineNumberParseError::Zero))
        );
    }

    #[test]
    fn test_location_displays_as_file_and_line() {
        let location = Location::new(PathBuf::from("src/a.ts"), line(7));

        assert_eq!(location.to_string(), "src/a.ts:7");
    }
}
