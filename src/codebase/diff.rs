//! git の差分から、作業ツリーのどの行が変わったかを集める。
//!
//! **git を起動するのは [`ChangedLines::of`] だけ。** 差分の文字列を読むところは
//! プロセスを要らない関数に切ってあるので、書式の扱いはリポジトリを作らずに全部テストできる
//! （`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」と同じ形）。

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::line_number::LineNumber;
use crate::syntax::line_range::LineRange;

/// 差分を取るときに作業ツリーと比べるリビジョン（`diff base`）。
///
/// **作業ツリーと比べる形に閉じる。** 行番号が作業ツリーを指していないと、
/// チャンクを切り出すファイルと行が食い違う。だから範囲（`a..b` / `a...b`）は受けない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBase(String);

/// `--diff` に値を書かなかったときの [`DiffBase`]。
pub const DEFAULT_DIFF_BASE: &str = "HEAD";

impl DiffBase {
    /// リビジョンの綴り。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DiffBase {
    type Err = DiffBaseError;

    /// リビジョンの綴りを確かめて作る。
    ///
    /// # Errors
    ///
    /// 空のとき、`-` で始まるとき（git がオプションとして読む）、`..` を含むとき（範囲）。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err(DiffBaseError::Empty);
        }
        if text.starts_with('-') {
            return Err(DiffBaseError::LeadingDash(text.to_owned()));
        }
        if text.contains("..") {
            return Err(DiffBaseError::Range(text.to_owned()));
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Display for DiffBase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// [`DiffBase`] として受け付けなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffBaseError {
    /// 空の綴り。
    Empty,
    /// `-` で始まる。git がリビジョンではなくオプションとして読む。
    LeadingDash(String),
    /// 範囲の指定。作業ツリーと比べる形にならない。
    Range(String),
}

impl fmt::Display for DiffBaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "リビジョンが空です"),
            Self::LeadingDash(text) => {
                write!(formatter, "{text}: `-` で始まるリビジョンは受け付けません")
            }
            Self::Range(text) => write!(
                formatter,
                "{text}: 範囲は受け付けません。作業ツリーと比べるリビジョンを 1 つ指定してください"
            ),
        }
    }
}

impl Error for DiffBaseError {}

/// 作業ツリーで変わった行を、ファイルごとに持つ。
///
/// キーのパスは、[`ChangedLines::of`] に渡した根に差分の相対パスをつないだもの。
/// **`codebase::source_paths_of` が同じ根から組み立てるパスと同じ形**になるので、
/// 走査で集めたパスでそのまま引ける。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChangedLines {
    files: BTreeMap<PathBuf, FileChange>,
}

/// 1 つのファイルの変わり方。
#[derive(Debug, PartialEq, Eq)]
enum FileChange {
    /// まだ追跡していないファイル。全行が変わったものとして扱う。
    Untracked,
    /// 差分の hunk の並び。
    Hunks(Vec<Hunk>),
}

/// 差分の hunk 1 つが、作業ツリー側で指す場所。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hunk {
    /// 足した・書き換えた行の範囲。
    Lines(LineRange),
    /// 消しただけの hunk。`following` の行の直前の点を指す。
    ///
    /// 行の範囲にしないのは、**消した点は 2 つの行の間にあり、どちらの行も変わっていない**ため。
    /// どちらかの行に寄せると、隣の関数を消した差分が手前の関数を「変わった」にする。
    Deletion { following: LineNumber },
}

impl Hunk {
    /// その行範囲のチャンクが、この hunk で変わったか。
    ///
    /// 消した点は、チャンクの中（`following` の手前の行もチャンクに入る）にあるときだけ数える。
    fn touches(self, lines: LineRange) -> bool {
        match self {
            Self::Lines(changed) => changed.overlaps(lines),
            Self::Deletion { following } => lines.contains(following) && lines.start() < following,
        }
    }
}

impl ChangedLines {
    /// `root` の作業ツリーを `base` と比べて、変わった行を集める。
    ///
    /// `root` の外の変更は含めない（`git diff --relative`）。まだ追跡していないファイル
    /// （`.gitignore` で外したものを除く）は全行が変わったものとして含める。
    /// **`git diff <base>` は新しく作ったファイルを出さない**ので、含めないと足したばかりの
    /// 関数が対象から漏れる。
    ///
    /// # Errors
    ///
    /// git を起動できない、`root` が git の作業ツリーでない、`base` を解決できない、
    /// git が失敗した、差分を読み解けないとき（[`DiffError`]）。
    pub fn of(root: &Path, base: &DiffBase) -> Result<Self, DiffError> {
        let inside = git_output_of(root, &["rev-parse", "--is-inside-work-tree"])?;
        if !inside.succeeded || inside.stdout.trim_ascii() != b"true" {
            return Err(DiffError::NotAWorkTree {
                root: root.to_path_buf(),
            });
        }

        let commit = format!("{base}^{{commit}}");
        let resolved = git_output_of(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &commit,
            ],
        )?;
        if !resolved.succeeded {
            return Err(DiffError::UnknownBase { base: base.clone() });
        }

        // 利用者の git の設定で出力の形が変わらないよう、色・外部 diff・接頭辞・パスの
        // 引用を引数で固定する
        let diff = git_output_of(
            root,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=0",
                "--relative",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                base.as_str(),
                "--",
            ],
        )?
        .succeeded_stdout()?;
        let untracked = git_output_of(root, &["ls-files", "-z", "--others", "--exclude-standard"])?
            .succeeded_stdout()?;

        let mut changed = Self::from_diff(&diff)?;
        changed.add_untracked(&untracked)?;
        Ok(changed.rooted_at(root))
    }

    /// `git diff --unified=0` の出力から、変わった行を読む。
    ///
    /// キーのパスは差分に書かれた相対パスのまま。
    ///
    /// # Errors
    ///
    /// hunk のヘッダーを読めない、hunk の行数が足りない、パスが引用符で囲まれている・
    /// UTF-8 でないとき。**読めない形を飛ばさない** — 飛ばすと、変更が見落とされたことが
    /// 出力に現れない。
    fn from_diff(diff: &[u8]) -> Result<Self, DiffParseError> {
        let mut files: BTreeMap<PathBuf, FileChange> = BTreeMap::new();
        let mut current: Option<PathBuf> = None;
        let mut lines = diff.split(|byte| *byte == b'\n');

        while let Some(line) = lines.next() {
            if line.starts_with(b"diff ") {
                current = None;
                continue;
            }
            if let Some(path) = line.strip_prefix(b"+++ ") {
                current = new_side_path_of(path)?;
                continue;
            }
            // `@@@`（結合した差分）も見出しとして読み、読めなければ止める。飛ばすと hunk を落とす
            if !line.starts_with(b"@@") {
                continue;
            }

            let header = HunkHeader::from_line(line)?;
            skip_hunk_body(&mut lines, header)?;
            // 消したファイル（`+++ /dev/null`）の hunk は、作業ツリーに指す先が無い
            let Some(path) = &current else {
                continue;
            };
            let hunks = files
                .entry(path.clone())
                .or_insert_with(|| FileChange::Hunks(Vec::new()));
            if let FileChange::Hunks(hunks) = hunks {
                hunks.push(header.new_side());
            }
        }

        Ok(Self { files })
    }

    /// `git ls-files -z --others` の出力のファイルを、全行が変わったものとして足す。
    ///
    /// # Errors
    ///
    /// パスが UTF-8 でないとき。
    fn add_untracked(&mut self, listed: &[u8]) -> Result<(), DiffParseError> {
        for path in listed
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = path_of(path)?;
            self.files.insert(path, FileChange::Untracked);
        }
        Ok(())
    }

    /// キーのパスを `root` からのパスにつなぎ直す。
    fn rooted_at(self, root: &Path) -> Self {
        Self {
            files: self
                .files
                .into_iter()
                .map(|(path, change)| (root.join(path), change))
                .collect(),
        }
    }

    /// そのファイルのその行範囲が変わったか。
    ///
    /// `path` は [`ChangedLines::of`] に渡した根から組み立てたパス。
    pub fn touches(&self, path: &Path, lines: LineRange) -> bool {
        match self.files.get(path) {
            None => false,
            Some(FileChange::Untracked) => true,
            Some(FileChange::Hunks(hunks)) => hunks.iter().any(|hunk| hunk.touches(lines)),
        }
    }

    /// 変わったファイルが 1 つも無いか。
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// `+++ ` の後ろから、作業ツリー側のパス。消したファイル（`/dev/null`）なら `None`。
///
/// 末尾のタブは落とす。**パスに空白を含むと git が付ける**（GNU patch との互換のため）。
fn new_side_path_of(spelled: &[u8]) -> Result<Option<PathBuf>, DiffParseError> {
    let spelled = spelled.strip_suffix(b"\t").unwrap_or(spelled);
    if spelled == b"/dev/null" {
        return Ok(None);
    }
    // 引用符で囲まれるのは `"`・`\`・制御文字を含むパス。エラーで止めると、対象外の
    // ファイル 1 つで check 全体が止まるので、エスケープを戻して読む
    let unquoted = match spelled.strip_prefix(b"\"") {
        Some(quoted) => unquoted_of(quoted)?,
        None => spelled.to_vec(),
    };
    let Some(path) = unquoted.strip_prefix(b"b/") else {
        return Err(DiffParseError::MalformedPath(
            String::from_utf8_lossy(spelled).into_owned(),
        ));
    };
    path_of(path).map(Some)
}

/// git が C の文字列の書式で引用したパスを、元のバイト列に戻す。
///
/// `quoted` は開きの `"` を除いた後ろ。閉じの `"` で終わる。
fn unquoted_of(quoted: &[u8]) -> Result<Vec<u8>, DiffParseError> {
    let malformed =
        || DiffParseError::MalformedQuotedPath(String::from_utf8_lossy(quoted).into_owned());
    let inner = quoted.strip_suffix(b"\"").ok_or_else(malformed)?;
    let mut bytes = Vec::with_capacity(inner.len());
    let mut rest = inner.iter().copied();

    while let Some(byte) = rest.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let escaped = match rest.next().ok_or_else(malformed)? {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'"' => b'"',
            b'\\' => b'\\',
            // core.quotePath を切っても、制御文字は 3 桁の 8 進数で書かれる
            first @ b'0'..=b'3' => {
                let digits = [
                    first,
                    rest.next().ok_or_else(malformed)?,
                    rest.next().ok_or_else(malformed)?,
                ];
                let text = std::str::from_utf8(&digits).map_err(|_| malformed())?;
                u8::from_str_radix(text, 8).map_err(|_| malformed())?
            }
            _ => return Err(malformed()),
        };
        bytes.push(escaped);
    }
    Ok(bytes)
}

/// git が出したパスのバイト列を、パスにする。
///
/// unix ではバイト列のまま持つ。**UTF-8 でない名前のファイル 1 つで check 全体を止めない**
/// （走査はそうしたファイルも `OsStr` のまま集めるので、ここで断ると取りこぼす）。
#[cfg(unix)]
fn path_of(bytes: &[u8]) -> Result<PathBuf, DiffParseError> {
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// git が出したパスのバイト列を、パスにする。
///
/// unix 以外ではパスをバイト列から作れないので、UTF-8 でなければ断る。
#[cfg(not(unix))]
fn path_of(bytes: &[u8]) -> Result<PathBuf, DiffParseError> {
    let path = std::str::from_utf8(bytes)
        .map_err(|_| DiffParseError::NonUtf8Path(String::from_utf8_lossy(bytes).into_owned()))?;
    Ok(PathBuf::from(path))
}

/// `@@ -a,b +c,d @@` の数。
#[derive(Debug, Clone, Copy)]
struct HunkHeader {
    old_count: usize,
    new_start: usize,
    new_count: usize,
}

impl HunkHeader {
    /// ヘッダーの行を読む。数を省いた側は 1 行。
    fn from_line(line: &[u8]) -> Result<Self, DiffParseError> {
        let malformed =
            || DiffParseError::MalformedHunkHeader(String::from_utf8_lossy(line).into_owned());
        let text = std::str::from_utf8(line).map_err(|_| malformed())?;
        let mut parts = text.split(' ');
        let (Some("@@"), Some(old), Some(new), Some("@@")) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed());
        };
        let (_, old_count) =
            range_of(old.strip_prefix('-').ok_or_else(malformed)?).ok_or_else(malformed)?;
        let (new_start, new_count) =
            range_of(new.strip_prefix('+').ok_or_else(malformed)?).ok_or_else(malformed)?;

        Ok(Self {
            old_count,
            new_start,
            new_count,
        })
    }

    /// 作業ツリー側で、この hunk が指す場所。
    ///
    /// 消しただけの hunk は `+c,0` と書かれ、c は**消した点の手前の行**を指す。
    fn new_side(self) -> Hunk {
        if self.new_count == 0 {
            return Hunk::Deletion {
                following: LineNumber::from_index(self.new_start),
            };
        }
        let start = LineNumber::from_index(self.new_start.saturating_sub(1));
        Hunk::Lines(LineRange::starting_at(start, self.new_count - 1))
    }
}

/// `c,d` / `c` を `(c, d)` に。`d` を省いたら 1。
fn range_of(spelled: &str) -> Option<(usize, usize)> {
    let Some((start, count)) = spelled.split_once(',') else {
        return Some((spelled.parse().ok()?, 1));
    };
    Some((start.parse().ok()?, count.parse().ok()?))
}

/// hunk の本文を、ヘッダーが数えた行数だけ読み飛ばす。
///
/// **数えずに `+++ ` を探すと、足した行の中身が `++ b/x` のとき次のファイルの見出しと
/// 取り違える。** `\ No newline at end of file` は行数に入らない。
fn skip_hunk_body<'a>(
    lines: &mut impl Iterator<Item = &'a [u8]>,
    header: HunkHeader,
) -> Result<(), DiffParseError> {
    let mut removed = header.old_count;
    let mut added = header.new_count;

    while removed + added > 0 {
        let Some(line) = lines.next() else {
            return Err(DiffParseError::TruncatedHunk);
        };
        match line.first() {
            Some(b'-') if removed > 0 => removed -= 1,
            Some(b'+') if added > 0 => added -= 1,
            Some(b'\\') => {}
            _ => return Err(DiffParseError::TruncatedHunk),
        }
    }
    Ok(())
}

/// 差分を読み解けなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffParseError {
    /// 引用符で囲まれたパスの、エスケープを戻せない。
    MalformedQuotedPath(String),
    /// `+++ ` の後ろが `b/` で始まらない。
    MalformedPath(String),
    /// パスが UTF-8 でない。
    NonUtf8Path(String),
    /// hunk のヘッダーを読めない。
    MalformedHunkHeader(String),
    /// hunk の本文が、ヘッダーの数えた行数に足りない。
    TruncatedHunk,
}

impl fmt::Display for DiffParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedQuotedPath(path) => {
                write!(formatter, "\"{path}: 引用符の付いたパスを戻せません")
            }
            Self::MalformedPath(path) => write!(formatter, "{path}: 差分のパスを読めません"),
            Self::NonUtf8Path(path) => write!(formatter, "{path}: UTF-8 でないパスは読めません"),
            Self::MalformedHunkHeader(line) => {
                write!(formatter, "{line}: hunk のヘッダーを読めません")
            }
            Self::TruncatedHunk => write!(formatter, "hunk の本文が途中で切れています"),
        }
    }
}

impl Error for DiffParseError {}

/// 変わった行を集められなかった理由。
///
/// **直す先ごとに分ける**（`rules/coding.md`「エラー型は原因ごとにバリアントを分ける」）。
#[derive(Debug)]
pub enum DiffError {
    /// git を起動できなかった（入っていない・実行できない）。
    GitUnavailable(io::Error),
    /// 根が git の作業ツリーの中にない。
    NotAWorkTree { root: PathBuf },
    /// リビジョンをコミットへ解決できなかった。
    UnknownBase { base: DiffBase },
    /// git が失敗した。保持しているのは git の標準エラー。
    GitFailed(String),
    /// 差分を読み解けなかった。
    Unparsable(DiffParseError),
}

impl fmt::Display for DiffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitUnavailable(cause) => write!(formatter, "git を起動できません: {cause}"),
            Self::NotAWorkTree { root } => write!(
                formatter,
                "{} は git の作業ツリーの中にありません",
                root.display()
            ),
            Self::UnknownBase { base } => {
                write!(formatter, "{base}: コミットとして解決できません")
            }
            Self::GitFailed(stderr) => write!(formatter, "git が失敗しました: {stderr}"),
            Self::Unparsable(cause) => write!(formatter, "差分を読み解けません: {cause}"),
        }
    }
}

impl Error for DiffError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::GitUnavailable(cause) => Some(cause),
            Self::Unparsable(cause) => Some(cause),
            Self::NotAWorkTree { .. } | Self::UnknownBase { .. } | Self::GitFailed(_) => None,
        }
    }
}

impl From<DiffParseError> for DiffError {
    fn from(cause: DiffParseError) -> Self {
        Self::Unparsable(cause)
    }
}

/// git を 1 回起動した結果。
struct GitOutput {
    succeeded: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl GitOutput {
    /// 成功していれば標準出力、失敗していれば標準エラーを持つ [`DiffError::GitFailed`]。
    fn succeeded_stdout(self) -> Result<Vec<u8>, DiffError> {
        if !self.succeeded {
            return Err(DiffError::GitFailed(
                String::from_utf8_lossy(&self.stderr).trim().to_owned(),
            ));
        }
        Ok(self.stdout)
    }
}

fn git_output_of(root: &Path, arguments: &[&str]) -> Result<GitOutput, DiffError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(DiffError::GitUnavailable)?;

    Ok(GitOutput {
        succeeded: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::line;

    fn lines(start: usize, end: usize) -> LineRange {
        LineRange::new(line(start), line(end)).expect("テストが渡す範囲は前後が揃っている")
    }

    fn changed_lines_of(diff: &str) -> ChangedLines {
        ChangedLines::from_diff(diff.as_bytes()).expect("テストが渡す差分は読める")
    }

    const MODIFIED: &str = "\
diff --git a/src/a.ts b/src/a.ts
index 1111111..2222222 100644
--- a/src/a.ts
+++ b/src/a.ts
@@ -12,2 +12,3 @@ function a() {
-  old();
-  older();
+  new();
+  newer();
+  newest();
";

    #[test]
    fn test_changed_lines_of_a_modified_hunk_touches_a_chunk_overlapping_the_new_lines() {
        let changed = changed_lines_of(MODIFIED);

        assert!(changed.touches(Path::new("src/a.ts"), lines(10, 12)));
        assert!(changed.touches(Path::new("src/a.ts"), lines(14, 20)));
    }

    #[test]
    fn test_changed_lines_of_a_modified_hunk_does_not_touch_a_chunk_outside_the_new_lines() {
        // 対照は上のテスト。足した行は 12-14 で、15 から始まるチャンクには届かない
        let changed = changed_lines_of(MODIFIED);

        assert!(!changed.touches(Path::new("src/a.ts"), lines(15, 20)));
        assert!(!changed.touches(Path::new("src/a.ts"), lines(1, 11)));
    }

    #[test]
    fn test_changed_lines_of_a_modified_hunk_does_not_touch_another_file() {
        let changed = changed_lines_of(MODIFIED);

        assert!(!changed.touches(Path::new("src/b.ts"), lines(1, 100)));
    }

    #[test]
    fn test_changed_lines_of_a_count_omitted_header_reads_it_as_one_line() {
        let changed = changed_lines_of(
            "\
+++ b/a.rs
@@ -3 +3 @@
-x
+y
",
        );

        assert!(changed.touches(Path::new("a.rs"), lines(3, 3)));
        assert!(!changed.touches(Path::new("a.rs"), lines(4, 9)));
    }

    const DELETION: &str = "\
+++ b/a.rs
@@ -6,2 +5,0 @@ fn a() {
-    gone();
-    gone_too();
";

    #[test]
    fn test_changed_lines_of_a_deletion_touches_a_chunk_spanning_the_deleted_point() {
        // 5 行目と 6 行目の間で消えた。両方を含むチャンクの中身が変わった
        let changed = changed_lines_of(DELETION);

        assert!(changed.touches(Path::new("a.rs"), lines(1, 6)));
    }

    #[test]
    fn test_changed_lines_of_a_deletion_does_not_touch_a_chunk_on_either_side_of_the_point() {
        // 対照は上のテスト。消した点の手前で終わるチャンクも、直後から始まるチャンクも
        // 中身は変わっていない
        let changed = changed_lines_of(DELETION);

        assert!(!changed.touches(Path::new("a.rs"), lines(1, 5)));
        assert!(!changed.touches(Path::new("a.rs"), lines(6, 9)));
    }

    #[test]
    fn test_changed_lines_of_a_deleted_file_has_nothing_to_touch() {
        let changed = changed_lines_of(
            "\
diff --git a/gone.ts b/gone.ts
deleted file mode 100644
--- a/gone.ts
+++ /dev/null
@@ -1,2 +0,0 @@
-a
-b
",
        );

        assert!(
            changed.is_empty(),
            "消したファイルは作業ツリーに無い: {changed:?}"
        );
    }

    #[test]
    fn test_changed_lines_of_a_renamed_file_is_keyed_by_the_new_path() {
        let changed = changed_lines_of(
            "\
diff --git a/old.ts b/new.ts
similarity index 90%
rename from old.ts
rename to new.ts
--- a/old.ts
+++ b/new.ts
@@ -2 +2 @@
-a
+b
",
        );

        assert!(changed.touches(Path::new("new.ts"), lines(1, 3)));
        assert!(!changed.touches(Path::new("old.ts"), lines(1, 3)));
    }

    #[test]
    fn test_changed_lines_of_a_path_with_a_space_drops_the_trailing_tab() {
        let changed = changed_lines_of("+++ b/sub/x y.ts\t\n@@ -2 +2 @@\n-b\n+B\n");

        assert!(changed.touches(Path::new("sub/x y.ts"), lines(2, 2)));
    }

    #[test]
    fn test_changed_lines_of_an_added_line_spelled_like_a_header_is_read_as_content() {
        // 足した行の中身が `++ b/other.ts` なので、本文の行は `+++ b/other.ts` になる。
        // 行数で読まずに見出しを探すと、後ろの hunk が other.ts に付く
        let changed = changed_lines_of(
            "\
+++ b/a.ts
@@ -1,0 +2,1 @@
+++ b/other.ts
@@ -9 +10 @@
-x
+y
",
        );

        assert!(changed.touches(Path::new("a.ts"), lines(10, 10)));
        assert!(!changed.touches(Path::new("other.ts"), lines(1, 100)));
    }

    #[test]
    fn test_changed_lines_of_a_hunk_without_newline_at_end_skips_the_marker() {
        let changed = changed_lines_of(
            "\
+++ b/a.ts
@@ -3 +3 @@
-x
\\ No newline at end of file
+y
\\ No newline at end of file
+++ b/b.ts
@@ -1 +1 @@
-p
+q
",
        );

        assert!(changed.touches(Path::new("a.ts"), lines(3, 3)));
        assert!(changed.touches(Path::new("b.ts"), lines(1, 1)));
    }

    #[test]
    fn test_changed_lines_of_a_quoted_path_reads_it_with_the_escapes_undone() {
        // `q"z.ts` は git が `"b/q\"z.ts"` と引用する。読まずに止めると、対象外の
        // ファイル 1 つで check 全体が止まる
        let changed = changed_lines_of("+++ \"b/q\\\"z\\tw.ts\"\n@@ -1 +1 @@\n-a\n+b\n");

        assert!(changed.touches(Path::new("q\"z\tw.ts"), lines(1, 1)));
    }

    #[test]
    fn test_changed_lines_of_a_quoted_path_with_an_octal_escape_reads_the_byte() {
        let changed = changed_lines_of("+++ \"b/a\\001b.ts\"\n@@ -1 +1 @@\n-a\n+b\n");

        assert!(changed.touches(Path::new("a\u{1}b.ts"), lines(1, 1)));
    }

    #[test]
    fn test_changed_lines_of_an_unclosed_quoted_path_is_rejected() {
        let result = ChangedLines::from_diff(b"+++ \"b/q.ts\n@@ -1 +1 @@\n-a\n+b\n");

        assert!(
            matches!(result, Err(DiffParseError::MalformedQuotedPath(_))),
            "閉じていない引用を推測で読まない: {result:?}"
        );
    }

    #[test]
    fn test_changed_lines_of_a_truncated_hunk_is_rejected() {
        let result = ChangedLines::from_diff(b"+++ b/a.ts\n@@ -1,2 +1,2 @@\n-a\n+b\n");

        assert_eq!(result, Err(DiffParseError::TruncatedHunk));
    }

    #[test]
    fn test_changed_lines_of_a_malformed_hunk_header_is_rejected() {
        let result = ChangedLines::from_diff(b"+++ b/a.ts\n@@@ -1 -1 +1 @@@\n");

        assert!(matches!(
            result,
            Err(DiffParseError::MalformedHunkHeader(_))
        ));
    }

    #[test]
    fn test_changed_lines_with_untracked_files_touches_every_line_of_them() {
        let mut changed = changed_lines_of(MODIFIED);

        changed
            .add_untracked(b"src/new.ts\0src/other new.ts\0")
            .expect("UTF-8 のパスは読める");

        assert!(changed.touches(Path::new("src/new.ts"), lines(1, 1)));
        assert!(changed.touches(Path::new("src/other new.ts"), lines(400, 500)));
    }

    #[cfg(unix)]
    #[test]
    fn test_changed_lines_of_a_non_utf8_path_keeps_its_bytes() {
        // Latin-1 の名前の画像 1 つで check 全体を止めない
        use std::os::unix::ffi::OsStrExt;

        let mut changed = changed_lines_of("+++ b/caf\u{e9}.ts\n@@ -1 +1 @@\n-a\n+b\n");
        changed
            .add_untracked(b"img/caf\xe9.png\0")
            .expect("unix ではバイト列のままパスにできる");

        let latin1 = Path::new(std::ffi::OsStr::from_bytes(b"img/caf\xe9.png"));
        assert!(changed.touches(latin1, lines(1, 1)));
    }

    #[test]
    fn test_changed_lines_rooted_at_a_directory_is_keyed_by_the_joined_path() {
        let changed = changed_lines_of(MODIFIED).rooted_at(Path::new("."));

        assert!(changed.touches(&Path::new(".").join("src/a.ts"), lines(12, 12)));
    }

    #[test]
    fn test_diff_base_of_a_revision_keeps_its_spelling() {
        let base: DiffBase = "main~2".parse().expect("リビジョンの綴りは受け付ける");

        assert_eq!(base.as_str(), "main~2");
    }

    #[test]
    fn test_diff_base_of_a_leading_dash_is_rejected() {
        let result = "--output=x".parse::<DiffBase>();

        assert_eq!(
            result,
            Err(DiffBaseError::LeadingDash("--output=x".to_owned()))
        );
    }

    #[test]
    fn test_diff_base_of_a_range_is_rejected() {
        let result = "main...HEAD".parse::<DiffBase>();

        assert_eq!(result, Err(DiffBaseError::Range("main...HEAD".to_owned())));
    }

    #[test]
    fn test_diff_base_of_an_empty_spelling_is_rejected() {
        assert_eq!("".parse::<DiffBase>(), Err(DiffBaseError::Empty));
    }
}
