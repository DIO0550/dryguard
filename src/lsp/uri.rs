//! パスから LSP へ渡す `file:` URI を作る。
//!
//! **綴り方をここ 1 箇所に閉じる。** ワークスペースの根とドキュメントの両方が同じ形の
//! URI を要るので、片方だけ別の綴りになると、サーバから見て**同じファイルが別物**になる。

use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

/// `file:` URI の、根より前の部分。
const FILE_SCHEME_PREFIX: &str = "file://";

/// URI の path に符号化せずに置ける記号（RFC 3986 の pchar から `%` を除いたもの）。
///
/// **符号化しすぎない。** ここを狭めて `@` や `+` まで `%40` / `%2B` にすると、
/// サーバが返す URI と綴りが食い違い、同じファイルを別物として突き合わせることになる。
const UNENCODED_PUNCTUATION: &[u8] = b"-._~!$&'()*+,;=:@";

/// 絶対パスを `file:` URI にする。
///
/// パスの区切りごとに、符号化が要る文字だけを `%XX` に直す。区切りそのものは符号化しない。
///
/// # Errors
///
/// `path` が絶対パスでないとき、UTF-8 として読めない要素を含むとき、
/// 組み立てた文字列が URI として読めないとき。
pub(super) fn file_uri_of(path: &Path) -> Result<Uri, PathUriError> {
    if !path.is_absolute() {
        return Err(PathUriError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }

    let mut text = String::from(FILE_SCHEME_PREFIX);

    for component in path.components() {
        match component {
            // 根は `file://` に続く `/` が表す。ここで足すと `//` になる。
            Component::RootDir => continue,
            // Windows の `C:` のような前置き。`:` は符号化せずに置ける記号なので、
            // 要素と同じ扱いにすると `C%3A` になってしまう。
            Component::Prefix(prefix) => {
                let Some(prefix) = prefix.as_os_str().to_str() else {
                    return Err(not_utf8_error_of(path));
                };
                text.push('/');
                text.push_str(prefix);
            }
            Component::Normal(name) => {
                let Some(name) = name.to_str() else {
                    return Err(not_utf8_error_of(path));
                };
                text.push('/');
                text.push_str(&percent_encoded(name));
            }
            // 絶対パスに `.` / `..` は現れないが、`is_absolute` はそこまで見ない。
            Component::CurDir | Component::ParentDir => {
                return Err(PathUriError::NotAbsolute {
                    path: path.to_path_buf(),
                });
            }
        }
    }

    // 根そのもの（`/`）は要素を 1 つも持たないので、ここまでで `file://` のまま。
    if text == FILE_SCHEME_PREFIX {
        text.push('/');
    }

    Uri::from_str(&text).map_err(|_| PathUriError::Malformed { text })
}

/// パスの要素 1 つ分を、URI に置ける形にする。
fn percent_encoded(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());

    // 文字ではなくバイトで回す。ASCII の外は、UTF-8 のバイトごとに `%XX` を並べるため。
    for byte in name.bytes() {
        let placeable = byte.is_ascii_alphanumeric() || UNENCODED_PUNCTUATION.contains(&byte);
        if placeable {
            encoded.push(char::from(byte));
            continue;
        }

        encoded.push_str(&format!("%{byte:02X}"));
    }

    encoded
}

fn not_utf8_error_of(path: &Path) -> PathUriError {
    PathUriError::NotUtf8 {
        path: path.to_path_buf(),
    }
}

/// パスを URI にできなかった理由。
#[derive(Debug)]
pub enum PathUriError {
    /// 絶対パスではない。
    NotAbsolute {
        /// 渡されたパス。
        path: PathBuf,
    },
    /// UTF-8 として読めない要素を含む。
    NotUtf8 {
        /// 読めなかったパス。
        path: PathBuf,
    },
    /// 組み立てた文字列が URI として読めない。
    Malformed {
        /// 読めなかった文字列。
        text: String,
    },
}

impl fmt::Display for PathUriError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute { path } => write!(
                formatter,
                "LSP へ渡すパスが絶対パスではありません: {}",
                path.display()
            ),
            Self::NotUtf8 { path } => write!(
                formatter,
                "LSP へ渡すパスを UTF-8 として読めません: {}",
                path.display()
            ),
            Self::Malformed { text } => {
                write!(formatter, "URI として読めない文字列になりました: {text}")
            }
        }
    }
}

impl Error for PathUriError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_uri_of_an_absolute_path_keeps_the_separators() {
        let uri = file_uri_of(Path::new("/home/user/src/pad.ts")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///home/user/src/pad.ts");
    }

    #[test]
    fn test_file_uri_of_a_path_with_a_space_encodes_it() {
        // 空白を素通しすると URI として読めない
        let uri = file_uri_of(Path::new("/home/user/my src/pad.ts")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///home/user/my%20src/pad.ts");
    }

    #[test]
    fn test_file_uri_of_a_path_outside_ascii_encodes_each_utf8_byte() {
        let uri = file_uri_of(Path::new("/home/user/請求/pad.ts")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///home/user/%E8%AB%8B%E6%B1%82/pad.ts");
    }

    #[test]
    fn test_file_uri_of_a_path_with_a_percent_encodes_it() {
        // 素通しすると、読む側が既に符号化された文字列として解く
        let uri = file_uri_of(Path::new("/home/user/100%/pad.ts")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///home/user/100%25/pad.ts");
    }

    #[test]
    fn test_file_uri_of_a_path_with_placeable_punctuation_leaves_it_as_written() {
        // 符号化しすぎると、サーバが返す URI と綴りが食い違う
        let uri = file_uri_of(Path::new("/home/user/@scope/a+b/pad.ts")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///home/user/@scope/a+b/pad.ts");
    }

    #[test]
    fn test_file_uri_of_the_root_directory_keeps_a_path() {
        // 要素が 1 つも無いので、`file://`（ホストだけ）になりうる
        let uri = file_uri_of(Path::new("/")).expect("URI にできる");

        assert_eq!(uri.as_str(), "file:///");
    }

    #[test]
    fn test_file_uri_of_a_relative_path_reports_it() {
        let error = file_uri_of(Path::new("src/pad.ts")).expect_err("URI にできない");

        assert!(matches!(
            error,
            PathUriError::NotAbsolute { path } if path == Path::new("src/pad.ts")
        ));
    }
}
