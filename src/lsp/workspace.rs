//! LSP サーバに渡すワークスペースの根。
//!
//! **根は開くファイルから決める。** 候補ペアのファイルだけを開く形（`docs/dryguard-plan.md`
//! 「Stage 2: 意味情報収集 (LSP)」）なので、根もその範囲を出ない位置に置く。

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
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
    /// `paths` は候補ペアが含まれるファイル。**リンクは辿らずに**絶対パスへ直してから
    /// 比べる（理由は [`uri::absolute_path_of`]）。開かせるドキュメントと同じ綴りに
    /// なっていないと、根の外のファイルを開かせることになる。
    ///
    /// **ファイルシステムを見ない。** 綴りだけで決めるので、置いていないパスを渡しても
    /// 同じ答えが返る。プロジェクトの印まで見て広げるのは [`WorkspaceRoot::enclosing_project`]。
    ///
    /// # Errors
    ///
    /// `paths` が空のとき、絶対パスにできないパスがあるとき、共通の祖先が無いとき、
    /// URI にできないとき。
    pub fn enclosing(paths: &[PathBuf]) -> Result<Self, WorkspaceError> {
        let directories = file_directories_of(paths)?;

        Self::at(&common_ancestor_or_error_of(&directories)?)
    }

    /// 開くファイル群のプロジェクトをすべて含む、最も近いディレクトリ。
    ///
    /// `paths` は候補ペアが含まれるファイル、`markers` はそのサーバがプロジェクトの根と
    /// 見なすファイルの名前（[`super::ServerCommand::project_markers`]）。
    /// **ファイル 1 つずつについて**、そのディレクトリから上へ辿って最も近い印を探す。
    /// 根は、印を持つファイルは印のディレクトリ・持たないファイルは自分のディレクトリを
    /// 出し合った共通の祖先。**印の有無はファイルごとに** [`ProjectRoot`] が持つ。
    ///
    /// **Why（サーバと同じ探し方をする）**: tsserver は開いたファイルから上へ
    /// `tsconfig.json` を探す。探し方を揃えると、こちらが渡す根とサーバが組み立てる
    /// プロジェクトが一致する。ずれると `textDocument/references` が
    /// **呼び出し元を取りこぼしたまま答える**（呼び出し元は呼び出し先を import する側に
    /// あるので、inferred project には入らない）。
    ///
    /// **Why（共通の祖先からではなくファイルごとに探す）**: 候補ペアが兄弟プロジェクトに
    /// またがると（`packages/a/tsconfig.json` と `packages/b/tsconfig.json` があり、
    /// その上には無い形）、印は共通の祖先より**下**にある。祖先から上だけを見ると
    /// どちらの印も見えず、**サーバは根の下の印を見つけて答えられるのに**
    /// こちらが取れないものと判断してしまう。
    ///
    /// **Why not（上へ辿る段数に上限を置く）**: 走査の根を上限にすると、
    /// `scan` の根が印より下だったときに取りこぼしがそのまま残り、しかも
    /// 上限になるものが無い `compare` と根の決め方が変わる。
    ///
    /// # Errors
    ///
    /// [`WorkspaceRoot::enclosing`] と同じ。印が見つからないのは失敗ではなく、
    /// [`ProjectRoot::is_marked`] が `false` を返す形になる。
    pub(crate) fn enclosing_project(
        paths: &[PathBuf],
        markers: &[String],
    ) -> Result<ProjectRoot, WorkspaceError> {
        let directories = file_directories_of(paths)?;

        // **印を持たないサーバでは、この検査そのものが当たらない。** 根で自分の範囲を
        // 決めるサーバは、印が無くても参照元を揃えて返せる。空の一覧を「揃わないことを
        // 確かめた」と読むと、確かめていない側の答えを落とすことになる。
        if markers.is_empty() {
            return Ok(ProjectRoot {
                root: Self::at(&common_ancestor_or_error_of(&directories)?)?,
                marked: paths.iter().cloned().collect(),
            });
        }

        // 印を持つファイルは印のディレクトリを、持たないファイルは自分のディレクトリを
        // 出す。印のほうを使わないと、根が印より下に来てサーバから印が見えなくなる。
        let mut covered = Vec::with_capacity(directories.len());
        let mut marked = BTreeSet::new();
        for (path, directory) in paths.iter().zip(&directories) {
            let Some(found) = nearest_marked_directory_of(directory, markers) else {
                covered.push(directory.clone());
                continue;
            };

            covered.push(found.to_path_buf());
            marked.insert(path.clone());
        }

        Ok(ProjectRoot {
            root: Self::at(&common_ancestor_or_error_of(&covered)?)?,
            marked,
        })
    }

    /// そのディレクトリを根にする。
    ///
    /// # Errors
    ///
    /// URI にできないとき。
    fn at(directory: &Path) -> Result<Self, WorkspaceError> {
        Ok(Self {
            uri: uri::file_uri_of(directory).map_err(WorkspaceError::Uri)?,
        })
    }

    /// `rootUri` として送る URI。
    pub(super) fn uri(&self) -> &Uri {
        &self.uri
    }
}

/// プロジェクトの印から決めた根と、**ファイルごとの**印の有無。
///
/// **印の有無をファイルごとに持つ。** 根は 1 つしか渡せないが、印の下にあるかは
/// ファイルごとに違う。まとめて 1 つの真偽にすると、`scan` で印の外のファイルが
/// 1 つ混じっただけで、**両側とも印の下にあるペアまで参照元を落とす**
/// （`rules/architecture.md`「どこまでを「取れなかった」に数えるか」）。
///
/// 印の下に無いファイルでは、根をどれだけ広げても参照元は揃わない
/// （サーバが開いたファイルとその import 先だけでプロジェクトを組み立てるため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRoot {
    root: WorkspaceRoot,
    marked: BTreeSet<PathBuf>,
}

impl ProjectRoot {
    /// サーバに見せる根。印の有無によらず、渡すものは根 1 つ。
    pub(crate) fn workspace_root(&self) -> &WorkspaceRoot {
        &self.root
    }

    /// そのファイルが印の下にあるか。
    ///
    /// `path` は [`WorkspaceRoot::enclosing_project`] に渡したのと**同じ綴り**。
    /// 渡していない綴りは印の下に無い扱いになる（**漏れは参照元を落とす側へ倒す**。
    /// `rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。
    pub(crate) fn is_marked(&self, path: &Path) -> bool {
        self.marked.contains(path)
    }
}

/// 開くファイルが 1 つずつ属するディレクトリ。
///
/// # Errors
///
/// `paths` が空のとき、絶対パスにできないパスがあるとき。
fn file_directories_of(paths: &[PathBuf]) -> Result<Vec<PathBuf>, WorkspaceError> {
    if paths.is_empty() {
        return Err(WorkspaceError::NoPaths);
    }

    let mut directories = Vec::with_capacity(paths.len());
    for path in paths {
        let resolved =
            uri::absolute_path_of(path).map_err(|cause| WorkspaceError::PathNotAbsolute {
                path: path.clone(),
                cause,
            })?;

        // 根そのものを渡された場合だけ親が無い。そのときは根自身が答え。
        let directory = resolved
            .parent()
            .map_or_else(|| resolved.clone(), Path::to_path_buf);
        directories.push(directory);
    }

    Ok(directories)
}

/// すべてを含む最も近い共通の祖先。
///
/// # Errors
///
/// 共通の祖先が無いとき。
fn common_ancestor_or_error_of(directories: &[PathBuf]) -> Result<PathBuf, WorkspaceError> {
    common_ancestor_of(directories).ok_or_else(|| WorkspaceError::NoCommonAncestor {
        directories: directories.to_vec(),
    })
}

/// `from` から上へ辿って、印を持つ最も近いディレクトリ。どこにも無ければ `None`。
///
/// **最も近い 1 つで止める。** 上にもう 1 つ印があっても、近いほうがそのファイルの
/// 属するプロジェクトを表す（`src/tsconfig.json` を持つ木の上に、リポジトリ全体の
/// `tsconfig.json` があるような形）。
fn nearest_marked_directory_of<'a>(from: &'a Path, markers: &[String]) -> Option<&'a Path> {
    from.ancestors().find(|directory| {
        markers
            .iter()
            .any(|marker| directory.join(marker).is_file())
    })
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
    /// パスを絶対パスにできない。
    PathNotAbsolute {
        /// 直せなかったパス。
        path: PathBuf,
        /// 直せなかった理由。
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
            Self::PathNotAbsolute { path, cause } => write!(
                formatter,
                "パスを絶対パスにできません ({}): {cause}",
                path.display()
            ),
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
            Self::PathNotAbsolute { cause, .. } => Some(cause),
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
        let directory =
            uri::absolute_path_of(&repository_path(relative)).expect("絶対パスにできる");

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
    fn test_enclosing_does_not_look_at_the_file_system() {
        // 実在を確かめる（= リンクを辿る）と、開かせるドキュメント側の綴りとずれる
        let paths = vec![
            repository_path("src/lib.rs"),
            repository_path("src/dryguard-no-such-file.rs"),
        ];

        let root = WorkspaceRoot::enclosing(&paths).expect("根を決められる");

        assert_eq!(root.uri().as_str(), expected_uri_of("src"));
    }

    /// そのディレクトリを指す根。**開くファイルではなくディレクトリから組み立てる**ので、
    /// 「印のあるディレクトリまで上がったか」を、入力と別の道筋で確かめられる。
    fn marked_root_of(relative_directory: &str) -> WorkspaceRoot {
        WorkspaceRoot::enclosing(&[repository_path(relative_directory).join("anything.ts")])
            .expect("根を決められる")
    }

    /// このリポジトリに置いていないファイルの名前。印が見つからない側の入力に使う。
    ///
    /// **実在する名前を使わない。** `tsconfig.json` を渡すと、リポジトリより上の階層に
    /// 誰かが置いた同名のファイルを拾って、環境によって答えが変わる。
    const A_MARKER_NOWHERE: &str = "dryguard-no-such-marker.json";

    /// 候補ペアが `tests/fixtures/references/src/billing/` に揃っている入力。
    ///
    /// 共通の祖先は `billing` で、**印はその 1 段上**（`src/tsconfig.json`）にある。
    fn paths_under_a_marked_ancestor() -> Vec<PathBuf> {
        vec![
            repository_path("tests/fixtures/references/src/billing/discount.ts"),
            repository_path("tests/fixtures/references/src/billing/scale.ts"),
        ]
    }

    #[test]
    fn test_enclosing_project_climbs_to_the_directory_holding_the_marker() {
        // 共通の祖先（billing）のままだと、サーバは開いたファイルとその import 先しか
        // プロジェクトに入れず、呼び出し元が返らない
        let root = WorkspaceRoot::enclosing_project(
            &paths_under_a_marked_ancestor(),
            &["tsconfig.json".to_owned()],
        )
        .expect("根を決められる");

        assert_eq!(
            root.workspace_root(),
            &marked_root_of("tests/fixtures/references/src")
        );
        assert!(
            paths_under_a_marked_ancestor()
                .iter()
                .all(|path| root.is_marked(path)),
            "どちらのファイルも印の下にある"
        );
    }

    #[test]
    fn test_enclosing_project_without_any_marker_stays_at_the_common_ancestor() {
        // 対照は上のテスト。同じ入力で印の名前だけを変える。**印が無いことを
        // 失敗にしない**（Stage 1 のシグナルだけで判定は続く）
        let root = WorkspaceRoot::enclosing_project(
            &paths_under_a_marked_ancestor(),
            &[A_MARKER_NOWHERE.to_owned()],
        )
        .expect("根を決められる");

        let paths = paths_under_a_marked_ancestor();
        assert_eq!(
            root.workspace_root(),
            &WorkspaceRoot::enclosing(&paths).expect("根を決められる")
        );
        assert!(
            paths.iter().all(|path| !root.is_marked(path)),
            "どちらのファイルも印の下に無い"
        );
    }

    #[test]
    fn test_enclosing_project_with_the_marker_at_the_common_ancestor_stops_there() {
        // 共通の祖先そのものが印を持つ形。1 段上へ行ってしまうと、
        // 開かせないファイルまで含む位置をサーバに見せることになる
        let paths = vec![
            repository_path("tests/fixtures/references/src/billing/discount.ts"),
            repository_path("tests/fixtures/references/src/report/monthly.ts"),
        ];

        let root = WorkspaceRoot::enclosing_project(&paths, &["tsconfig.json".to_owned()])
            .expect("根を決められる");

        assert_eq!(
            root.workspace_root(),
            &WorkspaceRoot::enclosing(&paths).expect("根を決められる")
        );
        assert!(paths.iter().all(|path| root.is_marked(path)));
    }

    #[test]
    fn test_enclosing_project_with_markers_at_two_depths_takes_the_nearest() {
        // `tsconfig.json` は `tests/fixtures/references/src` に、`Cargo.toml` は
        // リポジトリの根にある。遠いほうを採ると、リポジトリ全体がプロジェクトになる
        let root = WorkspaceRoot::enclosing_project(
            &paths_under_a_marked_ancestor(),
            &["Cargo.toml".to_owned(), "tsconfig.json".to_owned()],
        )
        .expect("根を決められる");

        assert_eq!(
            root.workspace_root(),
            &marked_root_of("tests/fixtures/references/src")
        );
    }

    #[test]
    fn test_enclosing_project_across_sibling_projects_covers_both_markers() {
        // 印が共通の祖先より**下**に 2 つある形（`sibling-projects/` に印は無い）。
        // 祖先から上だけを見ると印が 1 つも見えないが、**サーバは根の下の印を
        // 見つけて答えられる**ので、取れないものと判断してはいけない
        let paths = vec![
            repository_path("tests/fixtures/sibling-projects/billing/src/discount.ts"),
            repository_path("tests/fixtures/sibling-projects/inventory/src/reorder.ts"),
        ];

        let root = WorkspaceRoot::enclosing_project(&paths, &["tsconfig.json".to_owned()])
            .expect("根を決められる");

        assert_eq!(
            root.workspace_root(),
            &marked_root_of("tests/fixtures/sibling-projects")
        );
        assert!(
            paths.iter().all(|path| root.is_marked(path)),
            "どちらのファイルも自分のプロジェクトの下にある"
        );
    }

    #[test]
    fn test_enclosing_project_marks_only_the_files_under_a_project() {
        // 対照は上のテスト。片方だけ印を持たない形。**印の外の 1 つに引きずられて
        // 印の下のファイルまで落とさない**（`scan` では印の外のファイルが 1 つ
        // 混じっただけで、両側とも印の下にあるペアの参照元まで失われる）
        let inside = repository_path("tests/fixtures/sibling-projects/billing/src/discount.ts");
        let outside = repository_path("tests/fixtures/billing/discount.ts");
        let paths = vec![inside.clone(), outside.clone()];

        let root = WorkspaceRoot::enclosing_project(&paths, &["tsconfig.json".to_owned()])
            .expect("根を決められる");

        assert!(root.is_marked(&inside), "印の下のファイルは印の下と答える");
        assert!(
            !root.is_marked(&outside),
            "印の外のファイルは印の外と答える"
        );
    }

    #[test]
    fn test_enclosing_project_without_markers_treats_every_file_as_rooted() {
        // 印で範囲を決めないサーバ。**空の一覧は「当てはまらない」であって
        // 「揃わないことを確かめた」ではない**ので、参照元を落とす側へ倒さない
        let paths = paths_under_a_marked_ancestor();

        let root = WorkspaceRoot::enclosing_project(&paths, &[]).expect("根を決められる");

        assert_eq!(
            root.workspace_root(),
            &WorkspaceRoot::enclosing(&paths).expect("根を決められる")
        );
        assert!(
            paths.iter().all(|path| root.is_marked(path)),
            "印を見ないサーバでは、どのファイルも参照元を尋ねる対象になる"
        );
    }

    #[test]
    fn test_enclosing_project_with_no_paths_reports_it() {
        let error = WorkspaceRoot::enclosing_project(&[], &["tsconfig.json".to_owned()])
            .expect_err("根を決められない");

        assert!(matches!(error, WorkspaceError::NoPaths));
    }

    #[test]
    fn test_enclosing_an_empty_path_reports_which_one() {
        let empty = PathBuf::new();
        let paths = vec![repository_path("src/lib.rs"), empty.clone()];

        let error = WorkspaceRoot::enclosing(&paths).expect_err("根を決められない");

        assert!(matches!(
            error,
            WorkspaceError::PathNotAbsolute { path, .. } if path == empty
        ));
    }
}
