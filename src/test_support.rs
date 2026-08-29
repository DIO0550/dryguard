//! テストだけが使う値の組み立て。
//!
//! 置いてよいのは、**テストであることに依存しているもの**に限る
//! (rules/testing.md「テスト用ヘルパーの置き場所」)。ここにあるのは
//! 「テストが渡す値は前提を満たしている」という表明を含むので、実装側には置けない。
//!
//! 汎用の操作（値を別の表現へ直すなど）は実装側の責務なので、ここに書かない。

use std::path::{Path, PathBuf};

use crate::line_number::LineNumber;

/// このリポジトリの中のパス。
///
/// 実在するファイルを指す必要があるテスト（パスを辿る・URI にするなど）が使う。
/// カレントディレクトリではなくクレートの位置から組み立てるので、
/// どこから `cargo test` を呼んでも同じ場所を指す。
pub(crate) fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// 1 始まりの行番号。
///
/// # Panics
///
/// `number` が 0 のとき。テストが 0 行目を渡すのは、テスト自体の書き間違い。
pub(crate) fn line(number: usize) -> LineNumber {
    LineNumber::new(number).expect("テストが渡す行番号は 1 以上")
}
