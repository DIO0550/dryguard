//! サーバが自分の状態（取り込んだプロジェクト・索引）を書くディレクトリを、起動 1 回ごとに用意する。
//!
//! **作るのも消すのも起動側（[`super::Client`]）。** [`super::ServerCommand`] は I/O を持たない
//! 値なので、どのオプションで渡すかだけを持つ。

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

/// 同じプロセスの中で、次に作るディレクトリの名前に付ける番号。
static NEXT_SUFFIX: AtomicU64 = AtomicU64::new(0);

/// 名前が既にあったときに、番号を進めて作り直す回数の上限。
///
/// 既にあるのは、同じ pid の前のプロセスが消さずに落ちたときだけ。
const CREATE_ATTEMPTS: usize = 16;

/// 起動 1 回のためだけに作った、サーバの状態を書くディレクトリ。`Drop` で中身ごと消す。
///
/// **起動ごとに別の場所にする。** 共有すると、並べて起こしたサーバ同士が同じ場所を
/// 取り合い、名前の同じ別のプロジェクトの状態が混ざる。
#[derive(Debug)]
pub(super) struct StateDirectory {
    path: PathBuf,
}

impl StateDirectory {
    /// 一時ディレクトリの下に、まだ無い名前で作る。
    ///
    /// # Errors
    ///
    /// 作れなかったとき。番号を進めても既にある名前しか当たらなかったときは、
    /// 最後の `AlreadyExists`。
    pub(super) fn create() -> io::Result<Self> {
        let parent = env::temp_dir();
        let mut last_cause = io::Error::from(io::ErrorKind::AlreadyExists);
        for _ in 0..CREATE_ATTEMPTS {
            let suffix = NEXT_SUFFIX.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("dryguard-lsp-{}-{suffix}", process::id()));
            // `create_dir` は既にあれば失敗するので、他の誰かのディレクトリを奪わない。
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => last_cause = cause,
                Err(cause) => return Err(cause),
            }
        }
        Err(last_cause)
    }

    /// 作ったディレクトリ。
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StateDirectory {
    /// 失敗しても報告先が無いので捨てる。
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_makes_a_directory_that_drop_removes() {
        let directory = StateDirectory::create().expect("一時ディレクトリの下に作れる");
        let path = directory.path().to_path_buf();
        fs::write(path.join("index"), "jdtls が書く中身の代わり").expect("中に書ける");
        assert!(path.is_dir());

        drop(directory);

        assert!(!path.exists(), "中身ごと消える");
    }

    #[test]
    fn test_create_twice_makes_different_directories() {
        let first = StateDirectory::create().expect("作れる");
        let second = StateDirectory::create().expect("作れる");

        assert_ne!(first.path(), second.path());
    }
}
