//! git の index（ステージ済みの版）から、走査対象のファイルとその中身を読む
//! （`check --diff --staged`）。
//!
//! **チャンクを切り出す版と、差分の行番号が指す版を揃えるためにある。** `git diff --cached` の
//! 行番号は index の中身を指すので、部分的にステージした（`git add -p`）ファイルを
//! 作業ツリーから切り出すと行が食い違う。
//!
//! git を起動するのは [`IndexSources::of`] だけ。`ls-files` と `cat-file` の出力を読むところは
//! プロセスを要らない関数に切ってある（`codebase::diff` と同じ形）。

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use super::SourceLanguage;
use super::diff::{DiffError, git_output_of, path_of};
use super::is_excluded_name;
use crate::syntax::tree::Grammar;

/// 通常のファイルの mode。シンボリックリンク（`120000`）とサブモジュール（`160000`）は読まない。
///
/// 作業ツリーの走査もシンボリックリンクを辿らない（`codebase::source_paths_of`）。
const REGULAR_FILE_MODES: [&str; 2] = ["100644", "100755"];

/// 衝突していないエントリの stage。1〜3 は未解決のマージ衝突の各版。
const MERGED_STAGE: &str = "0";

/// 読み解けなかった出力のうち、エラーに持たせる長さの上限（バイト）。
const UNREADABLE_EXCERPT_LENGTH: usize = 80;

/// index にある走査対象のファイルと、その中身。
///
/// キーのパスは [`IndexSources::of`] に渡した根に index の相対パスをつないだもの。
/// **`ChangedLines` のキーと同じ形**になるので、変わった行をそのまま引ける。
#[derive(Debug, Default)]
pub struct IndexSources {
    files: BTreeMap<PathBuf, Vec<u8>>,
}

/// `git ls-files -s` の 1 行のうち、読むと決めたもの。
#[derive(Debug, PartialEq, Eq)]
struct IndexEntry {
    /// 根からの相対パス。
    path: PathBuf,
    /// 中身のオブジェクト名。
    object: String,
}

impl IndexSources {
    /// `root` の下の、`language` の対象ファイルを index から読む。
    ///
    /// 集める範囲は作業ツリーの走査（`codebase::source_paths_of`）と揃える。対象は拡張子で決め、
    /// 走査から外すディレクトリの下は読まない。**未追跡のファイルは index に無いので入らない**。
    ///
    /// 中身は `git cat-file --batch` 1 回で読む。ファイルごとに `git show :<path>` を起こすと、
    /// 走査対象のファイル数だけプロセスが立つ。
    ///
    /// # Errors
    ///
    /// git を起動できない・git が失敗した・出力を読み解けないとき、`root` の下に未解決の
    /// マージ衝突があるとき（[`DiffError::UnmergedPath`]）。
    pub fn of(root: &Path, language: SourceLanguage) -> Result<Self, DiffError> {
        let listed = git_output_of(root, &["ls-files", "-s", "-z"])?.succeeded_stdout()?;
        let entries = index_entries_of(&listed, language)?;
        if entries.is_empty() {
            return Ok(Self::default());
        }

        let mut requested = Vec::new();
        for entry in &entries {
            requested.extend_from_slice(entry.object.as_bytes());
            requested.push(b'\n');
        }
        let output = cat_file_output_of(root, requested)?;
        let blobs = blobs_of(&output, entries.len())?;

        Ok(Self {
            files: entries
                .into_iter()
                .zip(blobs)
                .map(|(entry, blob)| (root.join(entry.path), blob))
                .collect(),
        })
    }

    /// 読んだファイルのパス。パス順。
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    /// そのファイルの index の版を、文字列として返す。
    ///
    /// # Errors
    ///
    /// index から読んだファイルでないとき（`NotFound`）、UTF-8 として解釈できないとき
    /// （`InvalidData`）。**作業ツリーを読む `codebase::source_of` と同じ種類で返す**ので、
    /// 呼び出し側は読めなかったファイルを同じ形で飛ばせる。
    pub fn source_of(&self, path: &Path) -> io::Result<String> {
        let blob = self.files.get(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{} は index から読んだファイルではありません",
                    path.display()
                ),
            )
        })?;
        String::from_utf8(blob.clone())
            .map_err(|cause| io::Error::new(io::ErrorKind::InvalidData, cause))
    }
}

/// `git ls-files -s -z` の出力から、読むエントリを選ぶ。
///
/// # Errors
///
/// stage が 0 でないエントリがあるとき（[`DiffError::UnmergedPath`]）。**対象の言語でない
/// ファイルの衝突でも止める** — 衝突が残っているあいだはコミットできず、index は
/// コミットされる版を表していない。行を読み解けないときは [`DiffError::UnreadableIndex`]。
fn index_entries_of(listed: &[u8], language: SourceLanguage) -> Result<Vec<IndexEntry>, DiffError> {
    let mut entries = Vec::new();

    for record in listed
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let unreadable =
            || DiffError::UnreadableIndex(String::from_utf8_lossy(record).into_owned());
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(unreadable)?;
        let (header, path) = (&record[..tab], &record[tab + 1..]);
        let header = std::str::from_utf8(header).map_err(|_| unreadable())?;
        let mut fields = header.split(' ');
        let (Some(mode), Some(object), Some(stage), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(unreadable());
        };
        let path = path_of(path)?;

        if stage != MERGED_STAGE {
            return Err(DiffError::UnmergedPath { path });
        }
        let is_regular_file = REGULAR_FILE_MODES.contains(&mode);
        let is_readable_source =
            Grammar::of_path(&path).is_some_and(|grammar| language.accepts(grammar));
        if !is_regular_file || !is_readable_source || is_under_excluded_directory(&path) {
            continue;
        }

        entries.push(IndexEntry {
            path,
            object: object.to_owned(),
        });
    }

    Ok(entries)
}

/// 根からの相対パスが、走査から外すディレクトリの下にあるか。
///
/// 見るのは親のディレクトリだけ。作業ツリーの走査もディレクトリの名前で外し、
/// ファイルの名前では外さない。
fn is_under_excluded_directory(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent.iter().any(is_excluded_name))
}

/// `git cat-file --batch` に `requested`（オブジェクト名を 1 行ずつ）を渡し、標準出力を返す。
///
/// # Errors
///
/// git を起動できない・失敗したとき（[`DiffError::GitUnavailable`] / [`DiffError::GitFailed`]）、
/// 起動した git との標準入出力のやり取りに失敗したとき（[`DiffError::GitPipeFailed`]）。
fn cat_file_output_of(root: &Path, requested: Vec<u8>) -> Result<Vec<u8>, DiffError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(DiffError::GitUnavailable)?;

    // 書き込みは別のスレッドで行う。同じスレッドで書き切ってから読むと、標準出力のパイプが
    // 埋まった git が止まり、こちらの書き込みも止まる
    let stdin = child.stdin.take();
    let writer = thread::spawn(move || match stdin {
        Some(mut stdin) => stdin.write_all(&requested),
        None => Ok(()),
    });
    let output = child.wait_with_output().map_err(DiffError::GitPipeFailed)?;
    let written = writer
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("標準入力へ書くスレッドが落ちました")));

    if !output.status.success() {
        return Err(DiffError::GitFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    written.map_err(DiffError::GitPipeFailed)?;
    Ok(output.stdout)
}

/// `git cat-file --batch` の出力から、各オブジェクトの中身を頼んだ順に読む。
///
/// # Errors
///
/// 見出し（`<名前> blob <大きさ>`）を読めない・blob でない・見つからない（`<名前> missing`）・
/// 中身が途中で切れている・`expected` 件と数が揃わないとき（[`DiffError::UnreadableIndex`]）。
fn blobs_of(output: &[u8], expected: usize) -> Result<Vec<Vec<u8>>, DiffError> {
    let mut blobs = Vec::with_capacity(expected);
    let mut rest = output;

    while !rest.is_empty() {
        let unreadable = |spelled: &[u8]| {
            DiffError::UnreadableIndex(String::from_utf8_lossy(spelled).into_owned())
        };
        // 見出しの改行が無ければ、残りは中身かもしれない。全部をエラーに持たせない
        let newline = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| unreadable(&rest[..rest.len().min(UNREADABLE_EXCERPT_LENGTH)]))?;
        let header = &rest[..newline];
        let text = std::str::from_utf8(header).map_err(|_| unreadable(header))?;
        let mut fields = text.split(' ');
        let (Some(_), Some("blob"), Some(size), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(unreadable(header));
        };
        let size: usize = size.parse().map_err(|_| unreadable(header))?;

        let body = &rest[newline + 1..];
        // 中身の後ろに区切りの改行が 1 つ付く
        let (Some(blob), Some(b'\n')) = (body.get(..size), body.get(size)) else {
            return Err(unreadable(header));
        };
        blobs.push(blob.to_vec());
        rest = &body[size + 1..];
    }

    if blobs.len() != expected {
        return Err(DiffError::UnreadableIndex(format!(
            "{expected} 件を頼んで {} 件が返りました",
            blobs.len()
        )));
    }
    Ok(blobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &str = "587be6b4c3f93f93c489c0111bba5596147a26cb";

    fn listed(records: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    fn paths_of(entries: &[IndexEntry]) -> Vec<&Path> {
        entries.iter().map(|entry| entry.path.as_path()).collect()
    }

    #[test]
    fn test_index_entries_of_keeps_sources_of_the_language_with_their_objects() {
        let listed = listed(&[
            &format!("100644 {OBJECT} 0\tsrc/a.ts"),
            &format!("100644 {OBJECT} 0\tsrc/b.rs"),
            &format!("100644 {OBJECT} 0\tREADME.md"),
        ]);

        let entries =
            index_entries_of(&listed, SourceLanguage::TypeScript).expect("読める一覧を渡す");

        assert_eq!(
            entries,
            vec![IndexEntry {
                path: PathBuf::from("src/a.ts"),
                object: OBJECT.to_owned(),
            }]
        );
    }

    #[test]
    fn test_index_entries_of_leaves_out_excluded_directories_and_non_regular_files() {
        // 作業ツリーの走査と同じ範囲にする。実行可能ファイルは通常のファイルとして読む
        let listed = listed(&[
            &format!("100644 {OBJECT} 0\tnode_modules/dep/index.ts"),
            &format!("100644 {OBJECT} 0\tsrc/target/generated.rs"),
            &format!("100644 {OBJECT} 0\tpkg/target/built.rs"),
            &format!("120000 {OBJECT} 0\tsrc/link.rs"),
            &format!("160000 {OBJECT} 0\tvendor/sub.rs"),
            &format!("100755 {OBJECT} 0\tsrc/run.rs"),
            &format!("100644 {OBJECT} 0\tsrc/target.rs"),
        ]);

        let entries = index_entries_of(&listed, SourceLanguage::Auto).expect("読める一覧を渡す");

        assert_eq!(
            paths_of(&entries),
            vec![Path::new("src/run.rs"), Path::new("src/target.rs")],
            "除外ディレクトリの下・リンク・サブモジュールは読まず、同じ名前のファイルは読む"
        );
    }

    #[test]
    fn test_index_entries_of_an_unmerged_entry_is_rejected() {
        let listed = listed(&[
            &format!("100644 {OBJECT} 0\tsrc/a.ts"),
            &format!("100644 {OBJECT} 2\tdocs/notes.md"),
        ]);

        let result = index_entries_of(&listed, SourceLanguage::TypeScript);

        assert!(
            matches!(&result, Err(DiffError::UnmergedPath { path }) if path == Path::new("docs/notes.md")),
            "対象の言語でないファイルの衝突でも止める: {result:?}"
        );
    }

    #[test]
    fn test_index_entries_of_a_record_without_a_tab_is_rejected() {
        let listed = listed(&[&format!("100644 {OBJECT} 0 src/a.ts")]);

        let result = index_entries_of(&listed, SourceLanguage::TypeScript);

        assert!(matches!(result, Err(DiffError::UnreadableIndex(_))));
    }

    #[test]
    fn test_index_sources_paths_are_in_the_same_order_as_the_work_tree_walk() {
        // git は index をバイト順に並べる（`a-b/y.ts` が `a/x.ts` より前）。作業ツリーの走査は
        // パスの要素順なので、そのまま並べると `--staged` の有無で候補ペアの並びが変わる
        let sources = IndexSources {
            files: [
                (PathBuf::from("./a-b/y.ts"), Vec::new()),
                (PathBuf::from("./a/x.ts"), Vec::new()),
            ]
            .into_iter()
            .collect(),
        };

        assert_eq!(
            sources.paths(),
            vec![PathBuf::from("./a/x.ts"), PathBuf::from("./a-b/y.ts")]
        );
    }

    #[test]
    fn test_blobs_of_reads_each_body_by_its_size_in_order() {
        // 中身に改行や見出しに似た行を含んでも、大きさで切る
        let header_like = format!("{OBJECT} blob 1\n");
        let output = format!(
            "{OBJECT} blob 2\nx\n\n{OBJECT} blob {}\n{header_like}\n",
            header_like.len()
        );

        let blobs = blobs_of(output.as_bytes(), 2).expect("読める出力を渡す");

        assert_eq!(blobs, vec![b"x\n".to_vec(), header_like.into_bytes()]);
    }

    #[test]
    fn test_blobs_of_a_missing_object_is_rejected() {
        let output = format!("{OBJECT} missing\n");

        let result = blobs_of(output.as_bytes(), 1);

        assert!(matches!(result, Err(DiffError::UnreadableIndex(_))));
    }

    #[test]
    fn test_blobs_of_a_truncated_body_is_rejected() {
        let output = format!("{OBJECT} blob 10\nshort\n");

        let result = blobs_of(output.as_bytes(), 1);

        assert!(matches!(result, Err(DiffError::UnreadableIndex(_))));
    }

    #[test]
    fn test_blobs_of_fewer_objects_than_requested_is_rejected() {
        let output = format!("{OBJECT} blob 2\nx\n\n");

        let result = blobs_of(output.as_bytes(), 2);

        assert!(matches!(result, Err(DiffError::UnreadableIndex(_))));
    }

    #[test]
    fn test_index_sources_source_of_non_utf8_content_is_invalid_data() {
        let path = PathBuf::from("src/a.ts");
        let sources = IndexSources {
            files: BTreeMap::from([(path.clone(), vec![0xff, 0xfe])]),
        };

        let result = sources.source_of(&path);

        assert!(
            matches!(&result, Err(error) if error.kind() == io::ErrorKind::InvalidData),
            "作業ツリーを読むときと同じ種類で返す: {result:?}"
        );
    }
}
