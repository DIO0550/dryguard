//! ソースファイルの中の 1 点。

use lsp_types::Position;

use crate::line_number::LineNumber;

/// ファイルの中の 1 点。行と、行頭からの列。
///
/// **列は UTF-16 のコード単位で 0 から数える。** LSP の `Position` がそう定めており、
/// 位置を指す問い合わせ（hover など）はこの数え方でしか受け付けない。
///
/// Why（数え方をこの型に閉じる）: バイト位置から数え直すには、その位置が載っている行の
/// 文字列が要る。行の文字列を持っているのはソースを読んだ層だけなので、そこで数え終えて
/// この型にしてから持ち回る。バイト位置のまま渡すと、**どちらの数え方の列なのかが
/// 型から消える**（どちらも `usize`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    line: LineNumber,
    character: usize,
}

impl SourcePosition {
    /// その位置がある行と、行頭からその位置までの文字列から作る。
    ///
    /// `preceding` は行頭からこの位置の直前までのテキスト。列はその UTF-16 の
    /// コード単位の長さになる。
    ///
    /// **列を直接受け取る入口を置かない。** 置くと、バイトで数えた列がそのまま
    /// 渡せてしまい、非 ASCII を含む行で別の位置を指す
    /// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
    pub fn from_preceding_text(line: LineNumber, preceding: &str) -> Self {
        Self {
            line,
            character: preceding.encode_utf16().count(),
        }
    }

    /// その位置がある行。
    pub fn line(self) -> LineNumber {
        self.line
    }

    /// 行頭からの列。UTF-16 のコード単位で 0 始まり。
    pub fn character(self) -> usize {
        self.character
    }

    /// LSP が受け取る形の位置。
    ///
    /// 行を 0 始まりに直す。列は既にこの型が UTF-16 のコード単位で数えているので、
    /// そのまま渡す。どちらも `u32` なのは LSP がそう定めているため。
    ///
    /// **Why（この型が持つ）**: 0 始まりへの直しと UTF-16 の数え方は、どちらも
    /// **この位置が何を意味するか**の話。呼び出し側に組み立てさせると、
    /// 数え方を知らない場所で `Position` が作られる余地が残る。
    pub fn to_lsp_position(self) -> Position {
        Position {
            line: self.line.to_index() as u32,
            character: self.character as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::line;

    #[test]
    fn test_source_position_at_the_start_of_a_line_is_the_first_column() {
        let position = SourcePosition::from_preceding_text(line(3), "");

        assert_eq!(position.line(), line(3));
        assert_eq!(position.character(), 0);
    }

    #[test]
    fn test_source_position_counts_ascii_text_by_its_length() {
        let position = SourcePosition::from_preceding_text(line(1), "export function ");

        assert_eq!(position.character(), 16);
    }

    #[test]
    fn test_source_position_counts_a_non_ascii_prefix_in_utf16_code_units() {
        // 「請求」は UTF-8 では 6 バイト、UTF-16 では 2 コード単位。バイトで数えていると
        // 4 だけ後ろの位置を指し、サーバは別の識別子を見る
        let position = SourcePosition::from_preceding_text(line(1), "const 請求 = ");

        assert_eq!(position.character(), 11);
    }

    #[test]
    fn test_source_position_counts_a_surrogate_pair_as_two_code_units() {
        // 基本多言語面の外にある文字は UTF-16 で 2 コード単位になる。
        // Rust の char 数で数えていると 1 つ手前を指す
        let position = SourcePosition::from_preceding_text(line(1), "// 🦀 ");

        assert_eq!(position.character(), 6);
    }
}
