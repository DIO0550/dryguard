//! LSP サーバに渡すワークスペースの根。
//!
//! **根は開くファイルから決める。** 候補ペアのファイルだけを開く形（`docs/dryguard-plan.md`
//! 「Stage 2: 意味情報収集 (LSP)」）なので、根もその範囲を出ない位置に置く。

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lsp_types::Uri;

use super::uri::{self, PathUriError};

/// `initialize` でサーバに渡すワークスペースの根。
///
/// 生成時に絶対パスへ直して URI にするので、**作れた時点で `rootUri` として送れる**
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    uri: Uri,
}

impl WorkspaceRoot {
    /// 開くファイル群をすべて含む、最も近い共通の祖先ディレクトリ。
    ///
    /// `paths` は候補ペアが含まれるファイル。シンボリックリンクを辿った後で比べるので、
    /// 別の綴りで同じファイルを指していても同じ根になる。
    ///
    /// **Why not（`tsconfig.json` を上へ探す）**: 探索が TS 固有になり、Phase 4 の
    /// rust-analyzer に持ち越せない。tsserver は開いたファイルから自分で tsconfig を辿る。
    ///
    /// # Errors
    ///
    /// `paths` が空のとき、辿れないパスがあるとき、共通の祖先が無いとき、
    /// URI にできないとき。
    pub fn enclosing(paths: &[PathBuf]) -> Result<Self, WorkspaceError> {
        if paths.is_empty() {
            return Err(WorkspaceError::NoPaths);
        }

        let mut directories = Vec::with_capacity(paths.len());
        for path in paths {
            let resolved =
                fs::canonicalize(path).map_err(|cause| WorkspaceError::PathUnresolvable {
                    path: path.clone(),
                    cause,
                })?;

            // 根そのものを渡された場合だけ親が無い。そのときは根自身が答え。
            let directory = resolved
                .parent()
                .map_or_else(|| resolved.clone(), Path::to_path_buf);
            directories.push(directory);
        }

        let Some(ancestor) = common_ancestor_of(&directories) else {
            return Err(WorkspaceError::NoCommonAncestor { directories });
        };

        Ok(Self {
            uri: uri::file_uri_of(&ancestor).map_err(WorkspaceError::Uri)?,
        })
    }

    /// `rootUri` として送る URI。
    pub(super) fn uri(&self) -> &Uri {
        &self.uri
    }
}

/// すべてのディレクトリに共通する、最も深い祖先。共通部分が無ければ `None`。
///
/// 文字列ではなく要素で比べる。`/a/bc` と `/a/b` は先頭が一致するが、共通の祖先は `/a`。
fn common_ancestor_of(directories: &[PathBuf]) -> Option<PathBuf> {
    let (first, rest) = directories.split_first()?;
    let mut shared: Vec<_> = first.components().collect();

    for directory in rest {
        let matched = directory
            .components()
            .zip(shared.iter())
            .take_while(|(component, kept)| component == *kept)
            .count();
        shared.truncate(matched);
    }

    if shared.is_empty() {
        return None;
    }

    Some(shared.iter().collect())
}

/// ワークスペースの根を決められなかった理由。
#[derive(Debug)]
pub enum WorkspaceError {
    /// 開くファイルが 1 つも無い。
    NoPaths,
    /// パスを辿れない。
    PathUnresolvable {
        /// 辿れなかったパス。
        path: PathBuf,
        /// 辿れなかった理由。
        cause: io::Error,
    },
    /// 共通の祖先ディレクトリが無い。
    NoCommonAncestor {
        /// 祖先を共有していないディレクトリ。
        directories: Vec<PathBuf>,
    },
    /// 根を URI にできない。
    Uri(PathUriError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPaths => write!(
                formatter,
                "LSP に渡すワークスペースの根を決められません: 開くファイルがありません"
            ),
            Self::PathUnresolvable { path, cause } => {
                write!(formatter, "パスを辿れません ({}): {cause}", path.display())
            }
            Self::NoCommonAncestor { directories } => write!(
                formatter,
                "共通の祖先ディレクトリがありません: {}",
                directories
                    .iter()
                    .map(|directory| directory.display().to_string())
                    .collect::<Vec<String>>()
                    .join(" / ")
            ),
            Self::Uri(cause) => write!(formatter, "{cause}"),
        }
    }
}

impl Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoPaths | Self::NoCommonAncestor { .. } => None,
            Self::PathUnresolvable { cause, .. } => Some(cause),
            Self::Uri(cause) => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repository_path;

    /// 根として期待するディレクトリの URI。綴りは `uri` の担当なので、そちらで組み立てる。
    fn expected_uri_of(relative: &str) -> String {
        let directory = fs::canonicalize(repository_path(relative)).expect("実在するディレクトリ");

        uri::file_uri_of(&directory)
            .expect("URI にできる")
            .as_str()
            .to_owned()
    }

    #[test]
    fn test_enclosing_files_in_the_same_directory_is_that_directory() {
        let paths = vec![
            repository_path("src/lsp/uri.rs"),
            repository_path("src/lsp/workspace.rs"),
        ];

        let root = WorkspaceRoot::enclosing(&paths).expect("根を決められる");

        assert_eq!(root.uri().as_str(), expected_uri_of("src/lsp"));
    }

    #[test]
    fn test_enclosing_files_at_different_depths_is_the_shared_ancestor() {
        // 深いほうを答えにしていると、浅いほうのファイルが根の外に出る
        let paths = vec![
            repository_path("src/lib.rs"),
            repository_path("src/lsp/uri.rs"),
        ];

        let root = WorkspaceRoot::enclosing(&paths).expect("根を決められる");

        assert_eq!(root.uri().as_str(), expected_uri_of("src"));
    }

    #[test]
    fn test_enclosing_a_single_file_is_its_directory() {
        let paths = vec![repository_path("src/lsp/uri.rs")];

        let root = WorkspaceRoot::enclosing(&paths).expect("根を決められる");

        assert_eq!(root.uri().as_str(), expected_uri_of("src/lsp"));
    }

    #[test]
    fn test_enclosing_directories_that_share_only_a_name_prefix_stops_at_their_parent() {
        // 文字列の共通接頭辞で比べていると、`scan-skipped` を含まない
        // `tests/fixtures/scan` を根にしてしまう
        let paths = vec![
            repository_path("tests/fixtures/scan/src/shared/adder.ts"),
            repository_path("tests/fixtures/scan-skipped/src/sound.ts"),
        ];

        let root = WorkspaceRoot::enclosing(&paths).expect("根を決められる");

        assert_eq!(root.uri().as_str(), expected_uri_of("tests/fixtures"));
    }

    #[test]
    fn test_enclosing_no_paths_reports_it() {
        let error = WorkspaceRoot::enclosing(&[]).expect_err("根を決められない");

        assert!(matches!(error, WorkspaceError::NoPaths));
    }

    #[test]
    fn test_enclosing_a_missing_path_reports_which_one() {
        let missing = repository_path("src/dryguard-no-such-file.rs");
        let paths = vec![repository_path("src/lib.rs"), missing.clone()];

        let error = WorkspaceRoot::enclosing(&paths).expect_err("辿れない");

        assert!(matches!(
            error,
            WorkspaceError::PathUnresolvable { path, .. } if path == missing
        ));
    }
}
