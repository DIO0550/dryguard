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

/// `file:` URI のスキーム。
const FILE_SCHEME: &str = "file";

/// URI の path が `/` で始まるとき、その `/` が表すディレクトリ。
///
/// ここに来る URI は authority もドライブ文字も持たないので、根はこれだけ。
const ROOT_DIRECTORY: &str = "/";

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

/// `file:` URI が指すパス。[`file_uri_of`] の逆。
///
/// 渡すのはサーバが応答で返した URI（`references` が返す参照元など）。要素ごとに
/// `%XX` を戻し、区切りでつなぐ。**綴りのまま扱わない**のは、符号化された名前が
/// そのままディレクトリ名になるため（`my%20project` という名前のディレクトリを指す）。
///
/// # Errors
///
/// `file:` 以外のスキームのとき（サーバは開いていないバッファを `untitled:` で指す）、
/// authority を持つとき（UNC）、Windows のドライブ文字を含むとき、
/// `%XX` を戻した並びが UTF-8 として読めないとき。
///
/// **Why not（UNC とドライブ文字をパスに直す）**: どちらも Windows のパスで、
/// **この環境では正しく直せたかを確かめられない**（Linux の `PathBuf` は
/// `/C:/repo` を 1 つのディレクトリ名として持つ）。間違ったパスを返すより、
/// 綴れないことを名前で返す側に寄せる（`disk_letter_of` と同じ判断。Issue #112）。
pub(super) fn path_of(uri: &Uri) -> Result<PathBuf, UriPathError> {
    let spells_a_file = uri
        .scheme()
        .is_some_and(|scheme| scheme.eq_lowercase(FILE_SCHEME));
    if !spells_a_file {
        return Err(UriPathError::NotAFileUri { uri: text_of(uri) });
    }

    let names_a_host = uri
        .authority()
        .is_some_and(|authority| !authority.as_str().is_empty());
    if names_a_host {
        return Err(UriPathError::HasAuthority { uri: text_of(uri) });
    }

    let mut path = PathBuf::from(ROOT_DIRECTORY);

    for segment in uri.path().segments() {
        // 末尾の `/` と `//` は空の要素として現れる。要素として置くと、同じファイルが
        // 2 通りの綴りで届く。
        if segment.as_str().is_empty() {
            continue;
        }

        let Ok(name) = segment.decode().into_string() else {
            return Err(UriPathError::NotUtf8 { uri: text_of(uri) });
        };
        if is_drive_letter(&name) {
            return Err(UriPathError::DriveLetter { uri: text_of(uri) });
        }

        path.push(name.as_ref());
    }

    Ok(path)
}

/// Windows のドライブ文字を表す要素か（`C:`）。
///
/// [`file_uri_of`] が前置きをこの綴りで置くので、逆向きでも同じ形で見つかる。
fn is_drive_letter(name: &str) -> bool {
    let mut bytes = name.bytes();

    let starts_with_a_letter = bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic());
    let ends_with_a_colon = bytes.next() == Some(b':');

    starts_with_a_letter && ends_with_a_colon && bytes.next().is_none()
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

/// URI をパスにできなかった理由。
///
/// [`PathUriError`] と向きが逆。**1 つにまとめない**のは、綴れないパスを渡された話と、
/// サーバが返した URI を読めない話で**直す先が違う**ため（前者はこちらの入力、
/// 後者はサーバか dryguard の穴）。
#[derive(Debug)]
pub enum UriPathError {
    /// `file:` 以外のスキームを持つ。
    NotAFileUri {
        /// 読めなかった URI。
        uri: String,
    },
    /// authority（ホスト名）を持つ。UNC はまだ扱えない。
    HasAuthority {
        /// 読めなかった URI。
        uri: String,
    },
    /// Windows のドライブ文字を含む。
    DriveLetter {
        /// 読めなかった URI。
        uri: String,
    },
    /// `%XX` を戻した並びが UTF-8 として読めない。
    NotUtf8 {
        /// 読めなかった URI。
        uri: String,
    },
}

impl fmt::Display for UriPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFileUri { uri } => {
                write!(formatter, "ファイルを指す URI ではありません: {uri}")
            }
            Self::HasAuthority { uri } => write!(
                formatter,
                "ホスト名を持つ URI はまだパスにできません（UNC）: {uri}"
            ),
            Self::DriveLetter { uri } => write!(
                formatter,
                "Windows のドライブ文字を含む URI はまだパスにできません: {uri}"
            ),
            Self::NotUtf8 { uri } => {
                write!(formatter, "URI が指すパスを UTF-8 として読めません: {uri}")
            }
        }
    }
}

impl Error for UriPathError {}

/// 読めなかった URI を、失敗に残す綴りにする。
fn text_of(uri: &Uri) -> String {
    uri.as_str().to_owned()
}

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

    /// サーバが返す形の URI。
    fn uri(text: &str) -> Uri {
        Uri::from_str(text).expect("テストが渡す文字列は URI として読める")
    }

    #[test]
    fn test_path_of_a_file_uri_is_the_path_it_spells() {
        let path = path_of(&uri("file:///home/user/src/pad.ts")).expect("パスに戻せる");

        assert_eq!(path, Path::new("/home/user/src/pad.ts"));
    }

    #[test]
    fn test_path_of_a_uri_with_an_encoded_space_decodes_it() {
        // 対照は上のテスト。符号化された要素を含む URI を、綴りのまま受け取ると
        // `my%20project` という名前のディレクトリを指す
        let path = path_of(&uri("file:///home/user/my%20project/pad.ts")).expect("パスに戻せる");

        assert_eq!(path, Path::new("/home/user/my project/pad.ts"));
    }

    #[test]
    fn test_path_of_a_uri_outside_ascii_decodes_each_utf8_byte() {
        let path =
            path_of(&uri("file:///home/user/%E6%97%A5%E6%9C%AC/pad.ts")).expect("パスに戻せる");

        assert_eq!(path, Path::new("/home/user/日本/pad.ts"));
    }

    #[test]
    fn test_path_of_a_uri_a_path_was_turned_into_returns_that_path() {
        // 往復させる。片方だけの綴り方を変えると、同じファイルが別物として突き合わされる
        let original = Path::new("/home/user/my project/日本/pad.ts");

        let path = path_of(&file_uri_of(original).expect("URI にできる")).expect("パスに戻せる");

        assert_eq!(path, original);
    }

    #[test]
    fn test_path_of_a_uri_with_a_trailing_separator_does_not_keep_it() {
        // 末尾の `/` は空の要素として現れる。要素として置くと、同じディレクトリが
        // 2 通りの綴りで届く
        let path = path_of(&uri("file:///home/user/src/")).expect("パスに戻せる");

        assert_eq!(path, Path::new("/home/user/src"));
    }

    #[test]
    fn test_path_of_a_uri_with_another_scheme_reports_it() {
        // サーバは開いていないバッファを `untitled:` で指すことがある。
        // ファイルのパスとして読むと、実在しないファイルを指す
        let error = path_of(&uri("untitled:Untitled-1")).expect_err("パスに戻せない");

        assert!(matches!(error, UriPathError::NotAFileUri { .. }));
    }

    #[test]
    fn test_path_of_a_uri_naming_a_host_reports_it() {
        // `file://server/share/pad.ts` は UNC。authority を落とすと、
        // ネットワーク共有上のファイルをローカルの `/share/pad.ts` として指す
        let error = path_of(&uri("file://server/share/pad.ts")).expect_err("パスに戻せない");

        assert!(matches!(error, UriPathError::HasAuthority { .. }));
    }

    #[test]
    fn test_path_of_a_uri_with_a_drive_letter_reports_it() {
        // `file:///C:/repo/pad.ts` をそのまま組み立てると `/C:/repo/pad.ts` になり、
        // Windows でも Linux でも存在しないファイルを指す。**間違ったパスを返すより
        // 綴れないことを名前で返す**（Windows の扱いは Issue #112）
        let error = path_of(&uri("file:///C:/repo/pad.ts")).expect_err("パスに戻せない");

        assert!(matches!(error, UriPathError::DriveLetter { .. }));
    }

    #[test]
    fn test_path_of_a_uri_encoding_bytes_that_are_not_utf8_reports_it() {
        // 対照は %E6%97%A5 のテスト。そちらは UTF-8 として読める並びで、こちらは読めない
        let error = path_of(&uri("file:///home/user/%FF.ts")).expect_err("パスに戻せない");

        assert!(matches!(error, UriPathError::NotUtf8 { .. }));
    }
}
