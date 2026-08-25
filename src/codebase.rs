//! スキャン対象のコードベース。ディレクトリを歩いて対象ファイルを集めるところまでを持つ。
//!
//! **`syntax` に I/O を持たせないための入口が、`scan` ではここ。** `location` が
//! 「自分が指すファイルを読む」までを持つのと同じ形で、ここは「対象ファイルのパスを
//! 集める」までを持ち、読んだ結果を渡す先は純粋なまま保つ
//! (rules/coding.md「I/O を持ってよい場所」)。

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::syntax::tree::Grammar;

/// 走査から外すディレクトリの名前。
///
/// Phase 1 ではハードコードにする。`dryguard.toml` への外出しは Phase 3 で、
/// 実際に調整したくなった項目だけを切り出す（閾値と同じ方針）。
const EXCLUDED_DIRECTORY_NAMES: [&str; 5] = ["node_modules", "dist", "build", "target", ".git"];

/// そのディレクトリ以下の TypeScript ファイル（`.ts` / `.tsx`）を、パス順に集める。
///
/// `root` は走査を始めるディレクトリ。`node_modules` などの生成物・依存の置き場
/// （`EXCLUDED_DIRECTORY_NAMES`）は中へ降りない。
///
/// 対象かどうかは [`Grammar::of_path`] が読める拡張子かで決める。**拡張子の一覧を
/// ここに持たない**（持つと、読める拡張子を増やしたときに片方だけが古くなる）。
///
/// シンボリックリンクは辿らない。**辿ると循環したツリーで走査が終わらない**うえ、
/// 同じファイルを別のパスで 2 回数えることになる。
///
/// Why not（`path.is_dir()` で判定する）: リンクを辿った先の型を返すので、
/// リンクのディレクトリが `true` になる。`entry.file_type()` はリンク自身の型を返す。
///
/// # Errors
///
/// `root` がディレクトリでない / 途中のディレクトリを読めないとき。
pub fn typescript_paths_of(root: &Path) -> Result<Vec<PathBuf>, CodebaseError> {
    if !root.is_dir() {
        return Err(CodebaseError::RootNotADirectory {
            root: root.to_path_buf(),
        });
    }

    // パス順に並べるために木の形のまま集める。呼ぶ側が並べ直すと、
    // 「出力の並びは列挙順」がここの歩き方に左右される
    let mut paths = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(directory) = pending.pop() {
        for entry in entries_of(&directory)? {
            let path = entry.path();
            let file_type =
                entry
                    .file_type()
                    .map_err(|cause| CodebaseError::DirectoryUnreadable {
                        directory: directory.clone(),
                        cause,
                    })?;

            if file_type.is_dir() {
                if is_excluded(&path) {
                    continue;
                }
                pending.push(path);
                continue;
            }

            let is_readable_source = file_type.is_file() && Grammar::of_path(&path).is_some();
            if is_readable_source {
                paths.insert(path);
            }
        }
    }

    Ok(paths.into_iter().collect())
}

/// 対象ファイルを丸ごと読む。
///
/// `path` は [`typescript_paths_of`] が集めたファイル。
///
/// **`syntax` へ渡す前の読み込みをここに閉じる。** 呼び出し側で `fs` を直接叩くと、
/// I/O の置き場所が `codebase` と `location` の 2 つに閉じている形が崩れる
/// (rules/coding.md「I/O を持ってよい場所」)。
///
/// # Errors
///
/// ファイルが開けない / 読めない / UTF-8 として解釈できないとき。
pub fn source_of(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// そのディレクトリの中身。読めなければ、どのディレクトリで失敗したかを持つエラー。
fn entries_of(directory: &Path) -> Result<Vec<fs::DirEntry>, CodebaseError> {
    let unreadable = |cause: io::Error| CodebaseError::DirectoryUnreadable {
        directory: directory.to_path_buf(),
        cause,
    };

    fs::read_dir(directory)
        .map_err(unreadable)?
        .collect::<Result<Vec<fs::DirEntry>, io::Error>>()
        .map_err(unreadable)
}

/// そのディレクトリが走査の対象外か。
fn is_excluded(directory: &Path) -> bool {
    directory.file_name().is_some_and(|name| {
        EXCLUDED_DIRECTORY_NAMES
            .iter()
            .any(|excluded| name == *excluded)
    })
}

/// コードベースを走査できなかった理由。
///
/// 2 つに分けているのは利用者が直す先が違うため。根の指定は引数の間違いで、
/// 途中のディレクトリが読めないのは権限などの環境側の問題。
#[derive(Debug)]
pub enum CodebaseError {
    /// 走査の根がディレクトリではない（存在しない場合も含む）。
    RootNotADirectory { root: PathBuf },
    /// 途中のディレクトリを読めなかった。
    DirectoryUnreadable {
        directory: PathBuf,
        cause: io::Error,
    },
}

impl fmt::Display for CodebaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotADirectory { root } => {
                write!(formatter, "{} はディレクトリではありません", root.display())
            }
            Self::DirectoryUnreadable { directory, cause } => {
                write!(formatter, "{} を読めません: {cause}", directory.display())
            }
        }
    }
}

impl Error for CodebaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootNotADirectory { .. } => None,
            Self::DirectoryUnreadable { cause, .. } => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tests/fixtures/` 配下のディレクトリ。
    ///
    /// カレントディレクトリではなくマニフェストの位置から組み立てる
    /// （テストの実行位置に依存させない）。
    fn fixture(relative_path: &str) -> PathBuf {
        PathBuf::from(format!(
            "{}/tests/fixtures/{relative_path}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// 対象ディレクトリからの相対パス。並びを読みやすくするために揃える。
    fn relative_paths_of(root: &Path, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn test_typescript_paths_of_a_directory_returns_every_typescript_file_in_path_order() {
        let root = fixture("scan");

        let paths = typescript_paths_of(&root).expect("フィクスチャのディレクトリはある");

        assert_eq!(
            relative_paths_of(&root, &paths),
            vec![
                "src/billing/discount.ts",
                "src/billing/invoice.ts",
                "src/inventory/reorder.ts",
                "src/inventory/stock.ts",
                "src/report/Badge.tsx",
                "src/shared/adder.ts",
            ]
        );
    }

    #[test]
    fn test_typescript_paths_of_a_directory_leaves_out_the_excluded_directories() {
        // 対照は同じフィクスチャの src/。除外が効かないと node_modules と dist の
        // 関数まで比較の対象になる
        let root = fixture("scan");

        let paths = typescript_paths_of(&root).expect("フィクスチャのディレクトリはある");

        let relative = relative_paths_of(&root, &paths);
        assert!(
            !relative
                .iter()
                .any(|path| path.starts_with("node_modules/") || path.starts_with("dist/")),
            "除外したディレクトリのファイルが混ざっている: {relative:?}"
        );
        assert!(
            relative.iter().any(|path| path.starts_with("src/")),
            "除外の対象でないディレクトリは残る: {relative:?}"
        );
    }

    #[test]
    fn test_typescript_paths_of_a_directory_does_not_walk_into_a_symlinked_directory() {
        // 対照はリンク先の実体（src/billing）。実体のファイルは出るが、リンク経由の
        // パスでは出ない。`entry.file_type()` はリンク自身の型を返すのでこうなるが、
        // `path.is_dir()` に書き換えるとリンクを辿り、循環したツリーで走査が終わらなくなる
        let root = fixture("scan");

        let paths = typescript_paths_of(&root).expect("フィクスチャのディレクトリはある");

        let relative = relative_paths_of(&root, &paths);
        assert!(
            !relative
                .iter()
                .any(|path| path.starts_with("src/linked-billing/")),
            "リンク経由のパスで同じファイルを 2 回数えている: {relative:?}"
        );
        assert!(
            relative.contains(&"src/billing/discount.ts".to_owned()),
            "リンク先の実体は走査に出る: {relative:?}"
        );
    }

    #[test]
    fn test_typescript_paths_of_a_directory_returns_tsx_as_well_as_ts() {
        // `.tsx` を落とすと、JSX を書くリポジトリでは対象が丸ごと抜ける
        let root = fixture("scan");

        let paths = typescript_paths_of(&root).expect("フィクスチャのディレクトリはある");

        let relative = relative_paths_of(&root, &paths);
        assert!(
            relative.contains(&"src/report/Badge.tsx".to_owned()),
            "`.tsx` が対象に入っていない: {relative:?}"
        );
    }

    #[test]
    fn test_typescript_paths_of_a_directory_leaves_out_files_with_another_extension() {
        // 対照は上の 2 つ。フィクスチャの src/notes.md は同じディレクトリにあるが、
        // 読める拡張子ではないので対象にならない
        let root = fixture("scan");

        let paths = typescript_paths_of(&root).expect("フィクスチャのディレクトリはある");

        let relative = relative_paths_of(&root, &paths);
        assert!(
            !relative.iter().any(|path| path.ends_with(".md")),
            "読めない拡張子が混ざっている: {relative:?}"
        );
    }

    #[test]
    fn test_typescript_paths_of_a_missing_directory_reports_the_root_it_was_given() {
        let root = fixture("scan/missing");

        let result = typescript_paths_of(&root);

        let Err(CodebaseError::RootNotADirectory { root: reported }) = result else {
            panic!("ディレクトリでない根は RootNotADirectory になる");
        };
        assert_eq!(reported, root, "受け取った根をそのまま持つ");
    }

    #[test]
    fn test_typescript_paths_of_a_file_reports_that_it_is_not_a_directory() {
        // 対照は上のテスト。存在しないのではなく、あるがディレクトリではない
        let root = fixture("scan/src/billing/discount.ts");

        let result = typescript_paths_of(&root);

        assert!(
            matches!(result, Err(CodebaseError::RootNotADirectory { .. })),
            "ファイルを根に渡しても走査は始まらない"
        );
    }
}
