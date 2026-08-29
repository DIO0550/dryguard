//! パスを LSP へ渡せる形（絶対パスと `file:` URI）にする。
//!
//! **綴り方をここ 1 箇所に閉じる。** ワークスペースの根とドキュメントの両方が同じ形の
//! URI を要るので、片方だけ別の綴りになると、サーバから見て**同じファイルが別物**になる。

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::str::FromStr;

use lsp_types::Uri;

/// `file:` URI の、根より前の部分。
const FILE_SCHEME_PREFIX: &str = "file://";

/// URI の path に符号化せずに置ける記号（RFC 3986 の pchar から `%` を除いたもの）。
///
/// **符号化しすぎない。** ここを狭めて `@` や `+` まで `%40` / `%2B` にすると、
/// サーバが返す URI と綴りが食い違い、同じファイルを別物として突き合わせることになる。
const UNENCODED_PUNCTUATION: &[u8] = b"-._~!$&'()*+,;=:@";

/// 呼ばれた位置を起点に絶対パスへ直し、`.` と `..` を畳む。
///
/// **リンクは辿らない。** Stage 1 は importer に書かれたパスから指定子を畳んで依存先を出す
/// （`syntax::import`）ので、こちらだけリンクの向こう側へ寄せると、**同じファイルを
/// 2 つのステージが別のディレクトリから見る**ことになり、依存先の突き合わせがずれる。
///
/// **Why not（`fs::canonicalize`）**: 実在するファイルしか受け取れなくなるうえ、
/// リンクを辿るので上の食い違いが起きる。
///
/// # Errors
///
/// 空のパスを渡されたとき、カレントディレクトリを読めないとき。
pub(super) fn absolute_path_of(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut folded = PathBuf::new();

    for component in absolute.components() {
        match component {
            // `.` は位置を変えない。
            Component::CurDir => {}
            // 遡る先が無ければそのまま。根より上は無い。
            Component::ParentDir => {
                folded.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                folded.push(component);
            }
        }
    }

    Ok(folded)
}

/// 絶対パスを `file:` URI にする。
///
/// パスの区切りごとに、符号化が要る文字だけを `%XX` に直す。区切りそのものは符号化しない。
/// 渡すのは [`absolute_path_of`] が返したパス。
///
/// # Errors
///
/// `path` が絶対パスでないとき、`.` / `..` を畳んでいないとき、`file:` URI で表せない
/// 前置きを持つとき、UTF-8 として読めない要素を含むとき、組み立てた文字列が
/// URI として読めないとき。
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
            // Windows の `C:` のような前置き。**綴りをそのまま置かない**
            // （`\\?\C:` の形で渡されると `\` と `?` が URI に混じる）。
            Component::Prefix(prefix) => {
                let Some(disk) = disk_letter_of(prefix.kind()) else {
                    return Err(PathUriError::UnsupportedPrefix {
                        path: path.to_path_buf(),
                    });
                };
                text.push('/');
                text.push(char::from(disk));
                // `:` は符号化せずに置ける記号。要素と同じ扱いにすると `C%3A` になる。
                text.push(':');
            }
            Component::Normal(name) => {
                let Some(name) = name.to_str() else {
                    return Err(not_utf8_error_of(path));
                };
                text.push('/');
                text.push_str(&percent_encoded(name));
            }
            // 畳んでいないパスをそのまま URI にすると、同じファイルが 2 通りの綴りで届く。
            Component::CurDir | Component::ParentDir => {
                return Err(PathUriError::NotFolded {
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

/// Windows のドライブ文字。`file:` URI で表せない前置き（UNC・デバイス名前空間）は `None`。
///
/// **前置きの綴りではなくドライブ文字を取り出す。** `\\?\C:` のような verbatim 形式を
/// そのまま置くと、URI に `\` と `?` が混じって読めなくなる。
///
/// **Why not（UNC も URI にする）**: `file://server/share/..` の形になり、
/// 根とドキュメントで authority を揃える手当てが要る。**確かめる手段がここには無い**
/// （Linux では `Path::components` が `Prefix` を返さない）ので、
/// 綴れないことを名前で返す側に寄せる。
fn disk_letter_of(prefix: Prefix<'_>) -> Option<u8> {
    match prefix {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter),
        Prefix::Verbatim(_)
        | Prefix::VerbatimUNC(_, _)
        | Prefix::UNC(_, _)
        | Prefix::DeviceNS(_) => None,
    }
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
    /// `.` / `..` を畳んでいない。
    NotFolded {
        /// 渡されたパス。
        path: PathBuf,
    },
    /// `file:` URI で表せない前置き（UNC・デバイス名前空間）を持つ。
    UnsupportedPrefix {
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
            Self::NotFolded { path } => write!(
                formatter,
                "LSP へ渡すパスに `.` / `..` が残っています: {}",
                path.display()
            ),
            Self::UnsupportedPrefix { path } => write!(
                formatter,
                "この形式のパスは file: URI にできません（UNC・デバイス名前空間）: {}",
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
    use std::ffi::OsStr;

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

    #[test]
    fn test_file_uri_of_a_path_that_still_climbs_reports_it() {
        // 畳まずに URI にすると、同じファイルが 2 通りの綴りでサーバに届く
        let error = file_uri_of(Path::new("/home/user/src/../pad.ts")).expect_err("URI にできない");

        assert!(matches!(error, PathUriError::NotFolded { .. }));
    }

    #[test]
    fn test_disk_letter_of_a_verbatim_prefix_is_the_drive_letter() {
        // `canonicalize` や利用者が渡す `\\?\C:` をそのまま置くと、`\` と `?` が URI に混じる。
        // Linux では `Path::components` が `Prefix` を返さないので、file_uri_of 経由では
        // この分岐に届かない。ここだけ前置きの種類を直接渡して確かめる
        assert_eq!(disk_letter_of(Prefix::VerbatimDisk(b'C')), Some(b'C'));
        assert_eq!(disk_letter_of(Prefix::Disk(b'D')), Some(b'D'));
    }

    #[test]
    fn test_disk_letter_of_a_unc_prefix_is_absent() {
        // ドライブ文字を持たない前置きは、URI に綴れないものとして返す
        let unc = Prefix::UNC(OsStr::new("server"), OsStr::new("share"));

        assert_eq!(disk_letter_of(unc), None);
    }

    #[test]
    fn test_absolute_path_of_a_relative_path_starts_at_the_current_directory() {
        let current = std::env::current_dir().expect("カレントディレクトリを読める");

        let absolute = absolute_path_of(Path::new("src/pad.ts")).expect("絶対パスにできる");

        assert_eq!(absolute, current.join("src/pad.ts"));
    }

    #[test]
    fn test_absolute_path_of_folds_the_components_that_climb() {
        // 畳まないと、同じファイルが `src/../src/pad.ts` と `src/pad.ts` の 2 通りになる
        let absolute = absolute_path_of(Path::new("/home/user/src/../lib/./pad.ts"))
            .expect("絶対パスにできる");

        assert_eq!(absolute, Path::new("/home/user/lib/pad.ts"));
    }

    #[test]
    fn test_absolute_path_of_a_path_that_climbs_past_the_root_stops_at_the_root() {
        let absolute = absolute_path_of(Path::new("/../../pad.ts")).expect("絶対パスにできる");

        assert_eq!(absolute, Path::new("/pad.ts"));
    }

    #[test]
    fn test_absolute_path_of_a_path_that_does_not_exist_still_succeeds() {
        // 実在を要求する（= canonicalize する）と、リンクの向こう側を指すことになり、
        // Stage 1 が importer の位置から解決するのと食い違う
        let absolute = absolute_path_of(Path::new("/home/user/dryguard-no-such-file.ts"))
            .expect("絶対パスにできる");

        assert_eq!(absolute, Path::new("/home/user/dryguard-no-such-file.ts"));
    }

    #[test]
    fn test_absolute_path_of_an_empty_path_reports_it() {
        let error = absolute_path_of(Path::new("")).expect_err("絶対パスにできない");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
