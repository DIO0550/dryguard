//! TypeScript のソースを 1 文字ずつ読むときの、文字の種類。
//!
//! ここにあるのは `char` 一般の性質ではなく**その言語の字句としての性質**
//! （`$` が識別子に使える / バッククォートが文字列を開く）。Phase 4 で Rust 対応が
//! 入ったとき、言語ごとに違うものを分けて置けるようにここへ集める。
//!
//! Why not（`char` の拡張トレイト）: `current.is_quote()` と書けるが、言語ごとに
//! 違うはずのものが `char` 全体の性質に見える（`impl char` 自体は書けない）。

/// 文字列・テンプレートリテラルを開く引用符か。
pub(crate) fn is_quote(character: char) -> bool {
    matches!(character, '\'' | '"' | '`')
}

/// 識別子の 1 文字目になれる文字か。
pub(crate) fn is_word_start(character: char) -> bool {
    character.is_alphabetic() || matches!(character, '_' | '$')
}

/// 識別子の 2 文字目以降になれる文字か。
///
/// 1 文字目と違って数字を含む。
pub(crate) fn is_word_part(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$')
}
