//! LSP サーバに開かせるソースファイル 1 つ分。

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lsp_types::{TextDocumentItem, Uri};

use super::uri::{self, PathUriError};
use crate::syntax::tree::Grammar;

/// 開いたドキュメントの版。
///
/// **増えない。** dryguard はファイルを編集せず `didChange` を送らないので、
/// 開いた時点の中身のまま閉じるまで変わらない。
const INITIAL_VERSION: i32 = 1;

/// サーバに開かせるソースファイル。
///
/// 生成時に URI と languageId まで決めるので、**作れた時点で `didOpen` として送れる**
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDocument {
    uri: Uri,
    language_id: LanguageId,
    text: String,
}

impl SourceDocument {
    /// 読んだファイルのパスと、その中身から組み立てる。
    ///
    /// `text` を渡してもらうのは、ファイルを読むのが `codebase` / `location` の責務のため
    /// (rules/coding.md「I/O を持ってよい場所」)。ここが読むのはパスの解決だけで、
    /// **サーバへ渡す URI は絶対パスでなければならない**。
    ///
    /// # Errors
    ///
    /// パスを辿れないとき、読める拡張子でないとき、URI にできないとき。
    pub fn new(path: &Path, text: String) -> Result<Self, DocumentError> {
        let Some(grammar) = Grammar::of_path(path) else {
            return Err(DocumentError::UnreadableExtension {
                path: path.to_path_buf(),
            });
        };

        let resolved = fs::canonicalize(path).map_err(|cause| DocumentError::PathUnresolvable {
            path: path.to_path_buf(),
            cause,
        })?;

        Ok(Self {
            uri: uri::file_uri_of(&resolved).map_err(DocumentError::Uri)?,
            language_id: LanguageId::from_grammar(grammar),
            text,
        })
    }

    /// このドキュメントの URI。
    pub(super) fn uri(&self) -> &Uri {
        &self.uri
    }

    /// `didOpen` で送る形。
    pub(super) fn to_text_document_item(&self) -> TextDocumentItem {
        TextDocumentItem {
            uri: self.uri.clone(),
            language_id: self.language_id.as_str().to_owned(),
            version: INITIAL_VERSION,
            text: self.text.clone(),
        }
    }
}

/// サーバに伝える言語の名前。LSP が綴りを決めている語彙。
///
/// **拡張子からは決めない。** 読める拡張子の一覧は [`Grammar`] が 1 箇所で持っており、
/// ここにもう 1 つ置くと拡張子を足したときに片方だけが古くなる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanguageId {
    /// `.ts`
    TypeScript,
    /// `.tsx`
    TypeScriptReact,
}

impl LanguageId {
    fn from_grammar(grammar: Grammar) -> Self {
        match grammar {
            Grammar::TypeScript => Self::TypeScript,
            Grammar::Tsx => Self::TypeScriptReact,
        }
    }

    /// LSP が定める綴り。
    fn as_str(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
        }
    }
}

/// 開かせるドキュメントを組み立てられなかった理由。
#[derive(Debug)]
pub enum DocumentError {
    /// 読める拡張子ではない。
    UnreadableExtension {
        /// 渡されたパス。
        path: PathBuf,
    },
    /// パスを辿れない。
    PathUnresolvable {
        /// 辿れなかったパス。
        path: PathBuf,
        /// 辿れなかった理由。
        cause: io::Error,
    },
    /// パスを URI にできない。
    Uri(PathUriError),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableExtension { path } => write!(
                formatter,
                "LSP サーバに開かせられる拡張子ではありません: {}",
                path.display()
            ),
            Self::PathUnresolvable { path, cause } => {
                write!(formatter, "パスを辿れません ({}): {cause}", path.display())
            }
            Self::Uri(cause) => write!(formatter, "{cause}"),
        }
    }
}

impl Error for DocumentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnreadableExtension { .. } => None,
            Self::PathUnresolvable { cause, .. } => Some(cause),
            Self::Uri(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repository_path;

    #[test]
    fn test_new_from_a_typescript_file_names_the_language_as_typescript() {
        let path = repository_path("tests/fixtures/billing/discount.ts");

        let document = SourceDocument::new(&path, "export const a = 1;".to_owned())
            .expect("ドキュメントにできる");

        assert_eq!(document.to_text_document_item().language_id, "typescript");
    }

    #[test]
    fn test_new_from_a_tsx_file_names_the_language_as_typescriptreact() {
        // `.tsx` を typescript として渡すと、サーバが JSX を構文エラーとして読む
        let path = repository_path("tests/fixtures/scan/src/report/Badge.tsx");

        let document = SourceDocument::new(&path, "export const a = 1;".to_owned())
            .expect("ドキュメントにできる");

        assert_eq!(
            document.to_text_document_item().language_id,
            "typescriptreact"
        );
    }

    #[test]
    fn test_new_keeps_the_text_it_was_given() {
        // 開いた中身がディスク上のものとずれると、位置を指す問い合わせが別の場所を指す
        let path = repository_path("tests/fixtures/billing/discount.ts");
        let text = "export function pad(value: string): string {\n  return value;\n}\n";

        let document = SourceDocument::new(&path, text.to_owned()).expect("ドキュメントにできる");

        assert_eq!(document.to_text_document_item().text, text);
    }

    #[test]
    fn test_new_from_a_relative_path_still_produces_an_absolute_uri() {
        // 相対パスのまま URI にすると、サーバはこちらのカレントディレクトリを知らない
        let path = Path::new("tests/fixtures/billing/discount.ts");

        let document = SourceDocument::new(path, "export const a = 1;".to_owned())
            .expect("ドキュメントにできる");

        let uri = document.uri().as_str().to_owned();
        assert!(
            uri.starts_with("file:///") && uri.ends_with("/tests/fixtures/billing/discount.ts"),
            "絶対パスの URI になる: {uri}"
        );
    }

    #[test]
    fn test_new_from_a_file_that_is_not_source_reports_the_extension() {
        let path = repository_path("tests/fixtures/scan/src/notes.md");

        let error = SourceDocument::new(&path, String::new()).expect_err("開かせられない");

        assert!(matches!(
            error,
            DocumentError::UnreadableExtension { path: reported } if reported == path
        ));
    }

    #[test]
    fn test_new_from_a_missing_file_reports_which_one() {
        let missing = repository_path("tests/fixtures/billing/dryguard-no-such-file.ts");

        let error = SourceDocument::new(&missing, String::new()).expect_err("辿れない");

        assert!(matches!(
            error,
            DocumentError::PathUnresolvable { path, .. } if path == missing
        ));
    }

    #[test]
    fn test_to_text_document_item_starts_at_the_first_version() {
        let path = repository_path("tests/fixtures/billing/discount.ts");

        let document = SourceDocument::new(&path, String::new()).expect("ドキュメントにできる");

        assert_eq!(document.to_text_document_item().version, 1);
    }
}
