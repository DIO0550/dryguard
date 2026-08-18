//! 比較の単位（チャンク）と、ソースからの切り出し。
//!
//! Phase 0 の素朴実装。tree-sitter は使わず、関数の始まりに見える行とブレースの対応で
//! 範囲を決める（`docs/dryguard-plan.md`「Phase 0: 貫通させる (LSPなし)」）。
//!
//! 切り出しを `extract_*` と呼ばないのは、このツールの語彙では `extract` が
//! `EXTRACT-CANDIDATE`（共通化してよい）の意味で埋まっているため
//! (rules/naming.md「このツールの語彙を固定する」)。

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::line_number::LineNumber;
use crate::location::Location;
use crate::syntax::import::ImportSet;
use crate::syntax::line_range::LineRange;
use crate::syntax::source_character::{is_multiline_quote, is_quote, is_word_part};

/// 比較の単位。関数・メソッド 1 つ分のソースと、それがどこにあったか。
///
/// 依存先の集合だけはチャンクの範囲ではなく**ファイル全体**から採る。
/// import は関数の外に書かれるので、範囲を関数に合わせると必ず空になる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    path: PathBuf,
    lines: LineRange,
    source: String,
    imports: Option<ImportSet>,
}

impl Chunk {
    /// 指定位置を含む関数を、ソースから切り出す。
    ///
    /// `location` は切り出したい位置、`source` はそのファイルの中身。
    /// 入れ子になっている場合は、指定位置を含む**もっとも内側**の関数を返す。
    ///
    /// # Errors
    ///
    /// 指定行がソースの行数を超えている / 指定行を含む関数が無い /
    /// 関数の始まりは見つかったが閉じるブレースが無いとき。
    pub fn find_enclosing(location: &Location, source: &str) -> Result<Self, ChunkingError> {
        let lines: Vec<&str> = source.lines().collect();

        if location.line().get() > lines.len() {
            return Err(ChunkingError::LineBeyondSource {
                total_lines: lines.len(),
            });
        }

        let target_index = location.line().to_index();

        // 上に向かって関数の始まりを探す。最初に当たった始まりの範囲が指定行を含まない
        // ことがある（内側の関数が指定行より手前で閉じている場合）ので、含むまで遡り続ける。
        for header_index in (0..=target_index).rev() {
            if !is_function_header(lines[header_index]) {
                continue;
            }

            let Some(closing_index) = find_closing_index(&lines, header_index) else {
                // 内側が閉じていないなら、それを囲む関数も閉じていない
                return Err(ChunkingError::UnterminatedFunction {
                    start: LineNumber::from_index(header_index),
                });
            };

            if closing_index < target_index {
                continue;
            }

            return Ok(Self::new(
                location.path().to_path_buf(),
                LineRange::starting_at(
                    LineNumber::from_index(header_index),
                    closing_index - header_index,
                ),
                lines[header_index..=closing_index].join("\n"),
                ImportSet::from_source(source, location.path()),
            ));
        }

        Err(ChunkingError::NoEnclosingFunction)
    }

    /// 切り出した結果を組み立てる。
    ///
    /// モジュールの外から呼べないのは、`lines` と `source` が食い違ったチャンクを
    /// 作れないようにするため。組み立てるのは [`Chunk::find_enclosing`] だけで、
    /// そこでは `source` を `lines` の範囲から切り出している
    /// (rules/coding.md「不正な状態を型で表現できなくする」)。
    fn new(path: PathBuf, lines: LineRange, source: String, imports: Option<ImportSet>) -> Self {
        Self {
            path,
            lines,
            source,
            imports,
        }
    }

    /// 切り出し元のファイルパス。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 切り出した行範囲。
    pub fn lines(&self) -> LineRange {
        self.lines
    }

    /// 切り出したソース。行の区切りは改行 1 文字。
    pub fn source(&self) -> &str {
        &self.source
    }

    /// このチャンクがあるファイルの、依存先の集合。
    ///
    /// import が 1 つも無いファイルでは `None`。空の集合を返さないのは、
    /// 後段が「依存先が食い違っている」と「材料が無い」を区別できるようにするため
    /// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    pub fn imports(&self) -> Option<&ImportSet> {
        self.imports.as_ref()
    }
}

/// チャンクを切り出せなかった理由。
///
/// 「切り出せなかった」を既定値で埋めずに構造へ出す
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
/// 3 つに分けているのは、利用者が直す先がそれぞれ違うため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkingError {
    /// 指定行がソースの行数を超えている。保持しているのはソースの行数。
    LineBeyondSource { total_lines: usize },
    /// 指定行を含む関数が見つからない。
    NoEnclosingFunction,
    /// 関数の始まりは見つかったが、閉じるブレースが無い。保持しているのは始まりの行。
    UnterminatedFunction { start: LineNumber },
}

impl fmt::Display for ChunkingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineBeyondSource { total_lines } => {
                write!(formatter, "ソースは {total_lines} 行しかありません")
            }
            Self::NoEnclosingFunction => {
                write!(formatter, "指定行を含む関数が見つかりません")
            }
            Self::UnterminatedFunction { start } => {
                write!(formatter, "{start} 行目から始まる関数が閉じていません")
            }
        }
    }
}

impl Error for ChunkingError {}

/// 行頭に置かれると関数の始まりに見えてしまう制御構文のキーワード。
const CONTROL_KEYWORDS: [&str; 8] = ["if", "for", "while", "switch", "catch", "do", "try", "else"];

/// その行が関数・メソッドの始まりに見えるか。
///
/// `{` で終わり、丸カッコの対を持ち、制御構文で始まらない行を関数の始まりとみなす。
/// `class Foo {` はカッコが無いので外れ、`if (x) {` は行頭の語で外れる。
///
/// 複数行にまたがるシグネチャ（`): void {` だけの行）は拾えない。ここを詰めるより
/// tree-sitter へ移すほうが確実なので、Phase 0 では切り出せなかったこととして扱う。
fn is_function_header(line: &str) -> bool {
    let trimmed = line.trim();

    // `} else if (x) {` のように、閉じてから続く行を関数の始まりと取らない
    if trimmed.starts_with('}') || !trimmed.ends_with('{') {
        return false;
    }

    let has_parameter_list = match (trimmed.find('('), trimmed.rfind(')')) {
        (Some(open), Some(close)) => open < close,
        _ => false,
    };
    if !has_parameter_list {
        return false;
    }

    !CONTROL_KEYWORDS.contains(&leading_word(trimmed))
}

/// 行頭から続く、識別子として読める部分。
fn leading_word(trimmed: &str) -> &str {
    let end = trimmed
        .find(|character: char| !is_word_part(character))
        .unwrap_or(trimmed.len());
    &trimmed[..end]
}

/// 走査中に今どこにいるか。ブレースを数えてよいのは [`ScanState::Code`] のときだけ。
#[derive(Debug, Clone, Copy)]
enum ScanState {
    Code,
    BlockComment,
    /// 文字列・テンプレートリテラルの中。保持しているのは閉じる引用符。
    Text(char),
}

/// `start` の行から始まる本体が閉じる行のインデックス。閉じないまま末尾に達したら `None`。
///
/// 文字列リテラルとコメントの中は数えない。`const close = "}";` の 1 行で深さがずれると、
/// 範囲が**黙って**別の場所で切れる。切り出せなかったことは構造に出せるが、
/// 間違った範囲で成功したことは後段からも読者からも見えない。
fn find_closing_index(lines: &[&str], start: usize) -> Option<usize> {
    let mut state = ScanState::Code;
    let mut depth: usize = 0;
    let mut opened = false;

    for (index, line) in lines.iter().enumerate().skip(start) {
        let characters: Vec<char> = line.chars().collect();
        let mut position = 0;
        let mut text_continues_by_backslash = false;

        while position < characters.len() {
            let current = characters[position];
            let next = characters.get(position + 1).copied();

            match state {
                ScanState::Code => match (current, next) {
                    ('/', Some('/')) => break,
                    ('/', Some('*')) => {
                        state = ScanState::BlockComment;
                        position += 2;
                        continue;
                    }
                    // マッチガードにしているのは、パターンでは is_quote を呼べないため。
                    // 上 2 つの 2 文字先読みをパターンのまま残せる位置に置く
                    (quote, _) if is_quote(quote) => state = ScanState::Text(quote),
                    ('{', _) => {
                        depth += 1;
                        opened = true;
                    }
                    ('}', _) => {
                        depth = depth.saturating_sub(1);
                        if opened && depth == 0 {
                            return Some(index);
                        }
                    }
                    _ => {}
                },
                ScanState::BlockComment => {
                    if current == '*' && next == Some('/') {
                        state = ScanState::Code;
                        position += 2;
                        continue;
                    }
                }
                ScanState::Text(quote) => {
                    if current == '\\' {
                        // 行末の `\` は行継続で、文字列は次の行へ続く。
                        // `\\` で終わる行は継続しないので、次の文字の有無で見分ける
                        text_continues_by_backslash = next.is_none();
                        position += 2;
                        continue;
                    }
                    if current == quote {
                        state = ScanState::Code;
                    }
                }
            }

            position += 1;
        }

        // 行をまたぐ文字列は、テンプレートリテラルと行継続（行末の `\`）の 2 つだけ。
        // それ以外の閉じ忘れをまたがせると、そこから先のブレースを丸ごと読み飛ばす
        let text_crosses_the_line_end = matches!(state, ScanState::Text(quote) if is_multiline_quote(quote))
            || text_continues_by_backslash;
        if matches!(state, ScanState::Text(_)) && !text_crosses_the_line_end {
            state = ScanState::Code;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::line;
    use std::path::Path;

    fn range(start: usize, end: usize) -> LineRange {
        LineRange::new(line(start), line(end)).expect("テストが渡す範囲は start <= end")
    }

    fn chunk_at(source: &str, location: &str) -> Result<Chunk, ChunkingError> {
        let location: Location = location.parse().expect("テストが渡す位置は解釈できる");
        Chunk::find_enclosing(&location, source)
    }

    const FUNCTION_AFTER_A_CONSTANT: &str = r#"const rate = 0.1;

export function applyDiscount(price: number): number {
  const discounted = price * (1 - rate);
  return Math.max(discounted, 0);
}
"#;

    const NESTED_FUNCTIONS: &str = r#"function outer() {
  function inner() {
    return 1;
  }
  return inner;
}
"#;

    const CLOSING_BRACE_IN_A_STRING: &str = r#"function render(name: string): string {
  const close = "}";
  return name + close;
}
"#;

    const CLOSING_BRACE_IN_A_TEMPLATE_LITERAL: &str = r#"function render(name: string): string {
  const close = `}`;
  return name + close;
}
"#;

    const CLOSING_BRACE_IN_A_MULTILINE_TEMPLATE_LITERAL: &str = r#"function render(name: string): string {
  const template = `
}`;
  return name + template;
}
"#;

    const CLOSING_BRACE_IN_A_CONTINUED_STRING: &str = r#"function render(name: string): string {
  const close = "a\
}";
  return name + close;
}
"#;

    const CLOSING_BRACE_IN_A_LINE_COMMENT: &str = r#"function render(name: string): string {
  // 閉じブレース } はコメントの中
  return name;
}
"#;

    const CLOSING_BRACE_IN_A_BLOCK_COMMENT: &str = r#"function render(name: string): string {
  /* } */
  return name;
}
"#;

    const CONSTANT_OUTSIDE_A_FUNCTION: &str = r#"import { Invoice } from "./invoice";

function total(invoice: Invoice): number {
  return invoice.amount;
}

export const rate = 0.1;
"#;

    const UNTERMINATED_FUNCTION: &str = r#"function broken() {
  return 1;
"#;

    const ARROW_FUNCTION: &str = r#"export const applyDiscount = (price: number): number => {
  return price * 0.9;
};
"#;

    const IF_BLOCK_IN_A_FUNCTION: &str = r#"function classify(score: number): string {
  if (score > 0.9) {
    return "high";
  }
  return "low";
}
"#;

    const METHOD_IN_A_CLASS: &str = r#"export class Invoice {
  total(): number {
    return this.amount;
  }
}
"#;

    #[test]
    fn test_chunk_of_a_line_inside_a_function_covers_the_whole_function() {
        let chunk = chunk_at(FUNCTION_AFTER_A_CONSTANT, "a.ts:4").expect("切り出せる");

        assert_eq!(chunk.lines(), range(3, 6));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_function_keeps_the_source_of_that_range() {
        let chunk = chunk_at(FUNCTION_AFTER_A_CONSTANT, "a.ts:4").expect("切り出せる");

        assert_eq!(
            chunk.source(),
            "export function applyDiscount(price: number): number {\n  \
             const discounted = price * (1 - rate);\n  \
             return Math.max(discounted, 0);\n}"
        );
    }

    #[test]
    fn test_chunk_of_the_header_line_covers_the_whole_function() {
        let chunk = chunk_at(FUNCTION_AFTER_A_CONSTANT, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(3, 6));
    }

    #[test]
    fn test_chunk_of_the_closing_line_covers_the_whole_function() {
        let chunk = chunk_at(FUNCTION_AFTER_A_CONSTANT, "a.ts:6").expect("切り出せる");

        assert_eq!(chunk.lines(), range(3, 6));
    }

    #[test]
    fn test_chunk_keeps_the_path_of_the_location_it_was_asked_for() {
        let chunk =
            chunk_at(FUNCTION_AFTER_A_CONSTANT, "src/billing/discount.ts:4").expect("切り出せる");

        assert_eq!(chunk.path(), Path::new("src/billing/discount.ts"));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_nested_function_covers_the_inner_one() {
        let chunk = chunk_at(NESTED_FUNCTIONS, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(2, 4));
    }

    #[test]
    fn test_chunk_of_a_line_after_a_nested_function_covers_the_outer_one() {
        // 上に向かって最初に当たるヘッダは内側の関数だが、その範囲は 5 行目を含まない。
        // 直前のヘッダで打ち切ると、外側の関数を取り逃がす
        let chunk = chunk_at(NESTED_FUNCTIONS, "a.ts:5").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 6));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_string_does_not_end_at_that_line() {
        let chunk = chunk_at(CLOSING_BRACE_IN_A_STRING, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 4));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_template_literal_does_not_end_at_that_line() {
        // 引用符はシングル・ダブル・バッククォートの 3 つ。バッククォートが抜けると
        // テンプレートリテラルの中の `}` を本体の終わりと数える
        let chunk = chunk_at(CLOSING_BRACE_IN_A_TEMPLATE_LITERAL, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 4));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_multiline_template_literal_does_not_end_at_that_line() {
        // 行をまたげる引用符はバッククォートだけ。行末で文字列を打ち切ると
        // 次の行の `}` を本体の終わりと数える
        let chunk =
            chunk_at(CLOSING_BRACE_IN_A_MULTILINE_TEMPLATE_LITERAL, "a.ts:4").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 5));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_continued_string_does_not_end_at_that_line() {
        // 行末の `\` は行継続で、文字列は次の行へ続く。そこで文字列を打ち切ると
        // 次の行の `}` を本体の終わりと数えてしまう
        let chunk = chunk_at(CLOSING_BRACE_IN_A_CONTINUED_STRING, "a.ts:4").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 5));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_line_comment_does_not_end_at_that_line() {
        let chunk = chunk_at(CLOSING_BRACE_IN_A_LINE_COMMENT, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 4));
    }

    #[test]
    fn test_chunk_with_a_closing_brace_in_a_block_comment_does_not_end_at_that_line() {
        let chunk = chunk_at(CLOSING_BRACE_IN_A_BLOCK_COMMENT, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 4));
    }

    #[test]
    fn test_chunk_of_an_arrow_function_covers_the_whole_function() {
        let chunk = chunk_at(ARROW_FUNCTION, "a.ts:2").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 3));
    }

    #[test]
    fn test_chunk_of_a_line_inside_an_if_block_covers_the_enclosing_function() {
        // `if (score > 0.9) {` もブレースで終わる行なので、除外しないとここが
        // 関数の始まりに見える
        let chunk = chunk_at(IF_BLOCK_IN_A_FUNCTION, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 6));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_method_covers_the_method_not_the_class() {
        let chunk = chunk_at(METHOD_IN_A_CLASS, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(2, 4));
    }

    #[test]
    fn test_chunk_of_a_line_outside_every_function_returns_no_enclosing_function() {
        // 同じソースに関数を 1 つ置いてある。「上に関数がある」だけで切り出すと通ってしまう
        let result = chunk_at(CONSTANT_OUTSIDE_A_FUNCTION, "a.ts:7");

        assert_eq!(result, Err(ChunkingError::NoEnclosingFunction));
    }

    #[test]
    fn test_chunk_of_a_line_beyond_the_source_returns_line_beyond_source() {
        let result = chunk_at(CONSTANT_OUTSIDE_A_FUNCTION, "a.ts:100");

        assert_eq!(
            result,
            Err(ChunkingError::LineBeyondSource { total_lines: 7 })
        );
    }

    #[test]
    fn test_chunk_of_a_function_that_never_closes_returns_unterminated_function() {
        let result = chunk_at(UNTERMINATED_FUNCTION, "a.ts:2");

        assert_eq!(
            result,
            Err(ChunkingError::UnterminatedFunction { start: line(1) })
        );
    }
}
