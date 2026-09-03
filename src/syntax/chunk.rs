//! 比較の単位（チャンク）と、ソースからの切り出し。
//!
//! 範囲は tree-sitter の構文木から採る（`docs/dryguard-plan.md`「Stage 1: 候補抽出」）。
//! **どのノードがチャンクかという語彙をここが持つ**。木を歩く手順そのものは
//! `syntax::tree` にある。
//!
//! 切り出しを `extract_*` と呼ばないのは、このツールの語彙では `extract` が
//! `EXTRACT-CANDIDATE`（共通化してよい）の意味で埋まっているため
//! (rules/naming.md「このツールの語彙を固定する」)。

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::line_number::LineNumber;
use crate::location::Location;
use crate::source_position::SourcePosition;
use crate::syntax::import::ImportSet;
use crate::syntax::line_range::LineRange;
use crate::syntax::token::TokenSequence;
use crate::syntax::tree::{SyntaxTree, source_position_of};
use crate::syntax::type_reference::{TypeReference, type_references_of};

/// 比較の単位。関数・メソッド 1 つ分のソースと、それがどこにあったか。
///
/// 正規化トークン列は切り出しと同じ構文木から採る。**チャンクのソース文字列を
/// 読み直す形にはできない**（`total(): number { .. }` のようなメソッドは、
/// 単体ではトップレベルの構文にならない）。
///
/// 依存先の集合だけはチャンクの範囲ではなく**ファイル全体**から採る。
/// import は関数の外に書かれるので、範囲を関数に合わせると必ず空になる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    path: PathBuf,
    lines: LineRange,
    name_position: Option<SourcePosition>,
    type_references: Vec<TypeReference>,
    source: String,
    tokens: Option<TokenSequence>,
    imports: Option<ImportSet>,
}

impl Chunk {
    /// 指定位置を含む関数を、構文木から切り出す。
    ///
    /// `location` は切り出したい位置、`tree` はそのファイルの構文木。
    /// 入れ子になっている場合は、指定位置を含む**もっとも内側**の関数を返す。
    ///
    /// # Errors
    ///
    /// 指定行がソースの行数を超えている / 指定行を含む関数が無い /
    /// 指定行を含む関数に構文エラーがあるとき。
    pub fn find_enclosing(
        location: &Location,
        tree: &SyntaxTree<'_>,
    ) -> Result<Self, ChunkingError> {
        let total_lines = tree.source().lines().count();
        if location.line().get() > total_lines {
            return Err(ChunkingError::LineBeyondSource { total_lines });
        }

        let enclosing = innermost_chunk_node(tree, location.line())
            .ok_or(ChunkingError::NoEnclosingFunction)?;
        let lines = line_range_of(enclosing);

        // 木全体ではなくこのノードの部分木だけを見る。関係のない場所の構文エラーで
        // 切り出しを止めると、壊れている 1 関数がファイル全体を巻き込む
        if enclosing.has_error() {
            return Err(ChunkingError::UnparsableFunction {
                start: lines.start(),
            });
        }

        Ok(Self::new(
            location.path().to_path_buf(),
            lines,
            name_position_of(enclosing, tree.source()),
            type_references_of(enclosing, tree.source()),
            source_of_lines(tree.source(), lines),
            TokenSequence::from_node(enclosing),
            ImportSet::from_tree(tree, location.path()),
        ))
    }

    /// 切り出した結果を組み立てる。
    ///
    /// モジュールの外から呼べないのは、`lines` と `source` と `tokens` が食い違った
    /// チャンクを作れないようにするため。組み立てるのは [`Chunk::find_enclosing`] だけで、
    /// そこでは 3 つとも同じノードから採っている
    /// (rules/coding.md「不正な状態を型で表現できなくする」)。
    fn new(
        path: PathBuf,
        lines: LineRange,
        name_position: Option<SourcePosition>,
        type_references: Vec<TypeReference>,
        source: String,
        tokens: Option<TokenSequence>,
        imports: Option<ImportSet>,
    ) -> Self {
        Self {
            path,
            lines,
            name_position,
            type_references,
            source,
            tokens,
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

    /// このチャンクの名前が置かれている位置。名前が無ければ `None`。
    ///
    /// 位置を指す問い合わせ（`semantics` が送る hover）を、どこへ向けるかを決める値。
    ///
    /// `None` は「無名なので聞けない」。**チャンクの先頭位置で代用しない**のは、
    /// そこを指した問い合わせが答えなかったのか、答えた結果が空だったのかを
    /// 後段が区別できなくなるため (rules/architecture.md
    /// 「取れなかったシグナルを既定値で埋めない」)。
    pub fn name_position(&self) -> Option<SourcePosition> {
        self.name_position
    }

    /// このチャンクのシグネチャに書かれた型名。1 つも書かれていなければ空。
    ///
    /// **解決前の綴りと位置だけ**を持つ。その名前が何を指しているかを尋ねるのは
    /// `semantics` の担当で、ここが決めるのはどこを指して尋ねればよいかまで。
    ///
    /// 空を `None` にしないのは、**集められなかったという状態が無い**ため。
    /// キーワードの型だけで書かれたシグネチャでは、書かれていないことが空で表される。
    pub fn type_references(&self) -> &[TypeReference] {
        &self.type_references
    }

    /// 切り出したソース。行の区切りは改行 1 文字。
    pub fn source(&self) -> &str {
        &self.source
    }

    /// このチャンクを正規化したトークン列。
    ///
    /// トークンが 1 つも取れなかったときは `None`。空の列を返さないのは、後段が
    /// 「並びが食い違っている」と「材料が無い」を区別できるようにするため
    /// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    pub fn tokens(&self) -> Option<&TokenSequence> {
        self.tokens.as_ref()
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

/// 1 ファイルから切り出したチャンクと、構文エラーで切り出せなかった関数。
///
/// 切り出せなかった関数を捨てずに残すのは、スキャンの結果を読む側が
/// 「比べた上で似ていなかった」と「そもそも比べていない」を区別できるようにするため
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunks {
    chunks: Vec<Chunk>,
    unparsable_starts: Vec<LineNumber>,
}

impl FileChunks {
    /// そのファイルにある関数・メソッドを、構文木からすべて切り出す。
    ///
    /// `tree` はそのファイルの構文木、`path` は切り出したチャンクに持たせる位置。
    /// 入れ子になった関数は外側と内側の両方が返る。**別のファイルに同じ形の内側が
    /// あれば見つけたい**ので、外側に含まれることを理由に落とさない。
    ///
    /// 構文エラーのある関数はチャンクにせず、始まりの行だけを残す
    /// ([`Chunk::find_enclosing`] が [`ChunkingError::UnparsableFunction`] で断るのと同じ扱い)。
    pub fn from_tree(tree: &SyntaxTree<'_>, path: &Path) -> Self {
        let imports = ImportSet::from_tree(tree, path);
        let mut chunks = Vec::new();
        let mut unparsable_starts = Vec::new();

        for node in tree.named_descendants() {
            if !CHUNK_KINDS.contains(&node.kind()) {
                continue;
            }

            let lines = line_range_of(node);
            if node.has_error() {
                unparsable_starts.push(lines.start());
                continue;
            }

            chunks.push(Chunk::new(
                path.to_path_buf(),
                lines,
                name_position_of(node, tree.source()),
                type_references_of(node, tree.source()),
                source_of_lines(tree.source(), lines),
                TokenSequence::from_node(node),
                imports.clone(),
            ));
        }

        Self {
            chunks,
            unparsable_starts,
        }
    }

    /// 切り出せたチャンク。ソースに書かれた順に並ぶ。
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// 構文エラーで切り出せなかった関数の、始まりの行。
    pub fn unparsable_starts(&self) -> &[LineNumber] {
        &self.unparsable_starts
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
    /// 指定行を含む関数に構文エラーがある。保持しているのは始まりの行。
    ///
    /// 「閉じるブレースが無い」に限らないのは、構文木から見えるのが
    /// **その関数が構文として壊れていること**までのため。閉じ忘れはその一例。
    UnparsableFunction { start: LineNumber },
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
            Self::UnparsableFunction { start } => {
                write!(
                    formatter,
                    "{start} 行目から始まる関数に構文エラーがあります"
                )
            }
        }
    }
}

impl Error for ChunkingError {}

/// チャンクとして拾うノードの種別。
///
/// 「関数・メソッド 1 つ分」を tree-sitter の語彙に写したもの
/// (rules/naming.md「このツールの語彙を固定する」の `chunk`)。
/// クラスのメソッドとオブジェクトのメソッドは、grammar が同じ `method_definition` で表す。
///
/// `class_declaration` を入れないのは、比較の単位が関数だから。impl ブロックは
/// Phase 4 の Rust 対応で grammar ごと足す。
/// 代入を表すノードの種別。左辺が関数の名前になる形を見分けるのに使う。
const ASSIGNMENT_KIND: &str = "assignment_expression";

/// 識別子 1 つを表すノードの種別。
const IDENTIFIER_KIND: &str = "identifier";

const CHUNK_KINDS: [&str; 6] = [
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
];

/// 指定行を含むチャンクノードのうち、もっとも内側のもの。1 つも無ければ `None`。
///
/// 内側かどうかはバイト範囲の短さで決める。入れ子になったノードは必ず外側の範囲に
/// 収まるので、短いほうが内側になる。
fn innermost_chunk_node<'tree>(
    tree: &'tree SyntaxTree<'_>,
    line: LineNumber,
) -> Option<Node<'tree>> {
    tree.named_descendants()
        .into_iter()
        .filter(|node| CHUNK_KINDS.contains(&node.kind()))
        .filter(|node| line_range_of(*node).contains(line))
        .min_by_key(|node| node.byte_range().len())
}

/// そのチャンクの名前が置かれている位置。名前が無ければ `None`。
///
/// `source` はそのノードを含むファイル全体のソース。
fn name_position_of(node: Node<'_>, source: &str) -> Option<SourcePosition> {
    source_position_of(name_node_of(node)?, source)
}

/// そのチャンクの名前になっている識別子のノード。無名なら `None`。
///
/// 自分の `name` を持たないチャンク（無名の関数式・アロー関数）は、**代入先の名前**を
/// 使う。`const format = (…) => …` の `format`、クラスのプロパティ、オブジェクトの
/// プロパティがこれで、grammar 上はそれぞれ親ノードの `name` / `key` に載っている。
///
/// **Why（自分の名前を先に見る）**: `const named = function inner(…) {}` は両方持つ。
/// 代入先を指すと**変数に書かれた型**が返るので、注釈が付いていれば
/// （`const named: Formatter = function inner(…)`）関数自身の型ではなくその注釈を見ることになる。
fn name_node_of(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(name) = node.child_by_field_name("name") {
        return Some(name);
    }

    let parent = node.parent()?;
    if let Some(name) = parent
        .child_by_field_name("name")
        .or_else(|| parent.child_by_field_name("key"))
    {
        return Some(name);
    }

    assigned_name_of(parent)
}

/// 代入の左辺のうち、問い合わせられる名前になっているノード。代入でなければ `None`。
///
/// `obj.handler = (…) => …` や `exports.run = function (…) {…}` は、宣言ではなく
/// 代入なので親に `name` も `key` も無い。関数の名前になっているのは左辺の側。
///
/// **Why（プロパティを指す）**: 左辺全体（`obj.handler`）の先頭は `obj` で、そこを
/// 指すと入れ物の型が返る。名前として問い合わせられるのはプロパティのほう。
///
/// `target["key"] = …` のように名前になる識別子が無い形は `None`。
fn assigned_name_of(parent: Node<'_>) -> Option<Node<'_>> {
    if parent.kind() != ASSIGNMENT_KIND {
        return None;
    }

    let assigned = parent.child_by_field_name("left")?;
    if let Some(property) = assigned.child_by_field_name("property") {
        return Some(property);
    }
    if assigned.kind() == IDENTIFIER_KIND {
        return Some(assigned);
    }

    None
}

/// そのノードが覆っている行範囲。
fn line_range_of(node: Node<'_>) -> LineRange {
    let start_index = node.start_position().row;
    let end_index = node.end_position().row;

    LineRange::starting_at(
        LineNumber::from_index(start_index),
        end_index.saturating_sub(start_index),
    )
}

/// その行範囲のソース。行の区切りは改行 1 文字。
///
/// ノードが覆っているテキストではなく**行ごと**採る。ノードは行の途中から始まることが
/// あり（`export function f() {}` の `function` は `export ` の後ろ）、そこだけを持つと
/// `lines` が指す範囲と `source` の中身がずれる。
fn source_of_lines(source: &str, lines: LineRange) -> String {
    let start_index = lines.start().to_index();
    let line_count = lines.end().to_index() - start_index + 1;

    source
        .lines()
        .skip(start_index)
        .take(line_count)
        .collect::<Vec<&str>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::tree::Grammar;
    use crate::test_support::line;
    use std::path::Path;

    fn range(start: usize, end: usize) -> LineRange {
        LineRange::new(line(start), line(end)).expect("テストが渡す範囲は start <= end")
    }

    fn chunk_at(source: &str, location: &str) -> Result<Chunk, ChunkingError> {
        let location: Location = location.parse().expect("テストが渡す位置は解釈できる");
        let tree = SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる");

        Chunk::find_enclosing(&location, &tree)
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
    fn test_chunk_of_a_function_that_never_closes_returns_unparsable_function() {
        let result = chunk_at(UNTERMINATED_FUNCTION, "a.ts:2");

        assert_eq!(
            result,
            Err(ChunkingError::UnparsableFunction { start: line(1) })
        );
    }

    #[test]
    fn test_chunk_of_a_function_next_to_a_broken_one_is_still_taken() {
        // 対照は上のテスト。壊れているのが別の関数なら切り出しは止まらない。
        // 木全体のエラーを見ていると、ここで NoEnclosingFunction に落ちる
        let broken_then_sound = r#"function broken(: {
}

export function sound(value: number): number {
  return value;
}
"#;

        let chunk = chunk_at(broken_then_sound, "a.ts:5").expect("壊れていない側は切り出せる");

        assert_eq!(chunk.lines(), range(4, 6));
    }

    #[test]
    fn test_chunk_of_a_function_with_a_multiline_signature_covers_the_whole_function() {
        // 行を 1 本ずつ見る切り出しでは、`): number {` だけの行を関数の始まりと
        // 読めず、この形を丸ごと取り逃がしていた
        let multiline_signature = r#"export function applyDiscount(
  price: number,
  rate: number,
): number {
  return price * (1 - rate);
}
"#;

        let chunk = chunk_at(multiline_signature, "a.ts:5").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 6));
    }

    #[test]
    fn test_chunk_of_a_line_inside_an_object_method_covers_the_method() {
        let object_with_a_method = r#"export const handlers = {
  run(value: number): number {
    return value;
  },
};
"#;

        let chunk = chunk_at(object_with_a_method, "a.ts:3").expect("切り出せる");

        assert_eq!(chunk.lines(), range(2, 4));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_function_expression_covers_that_function() {
        let function_expression = r#"export const applyDiscount = function (price: number): number {
  return price * 0.9;
};
"#;

        let chunk = chunk_at(function_expression, "a.ts:2").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 3));
    }

    #[test]
    fn test_chunk_of_an_arrow_function_without_a_body_block_covers_that_function() {
        // ブレースで終わらない行なので、行の見た目で判断する切り出しでは拾えなかった
        let expression_bodied_arrow = r#"export const double = (value: number): number => value * 2;
"#;

        let chunk = chunk_at(expression_bodied_arrow, "a.ts:1").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 1));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_generator_function_covers_that_function() {
        let generator = r#"export async function* pages(total: number): AsyncGenerator<number> {
  yield total;
}
"#;

        let chunk = chunk_at(generator, "a.ts:2").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 3));
    }

    #[test]
    fn test_chunk_of_a_line_inside_a_generator_function_expression_covers_that_function() {
        // 名前の付いたジェネレータとは別のノード種別になる（宣言と式で分かれている）
        let generator_expression = r#"export const pages = function* (total: number) {
  yield total;
};
"#;

        let chunk = chunk_at(generator_expression, "a.ts:2").expect("切り出せる");

        assert_eq!(chunk.lines(), range(1, 3));
    }

    #[test]
    fn test_chunk_of_a_line_inside_an_arrow_nested_in_a_function_covers_the_arrow() {
        // 外側の関数も指定行を含む。内側を選べていないと 1-3 行目が返る
        let arrow_inside_a_function = r#"export function makeAdder(a: number) {
  return (b: number) => a + b;
}
"#;

        let chunk = chunk_at(arrow_inside_a_function, "a.ts:2").expect("切り出せる");

        assert_eq!(chunk.lines(), range(2, 2));
    }

    fn chunks_at(source: &str, path: &str) -> FileChunks {
        let tree = SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる");

        FileChunks::from_tree(&tree, Path::new(path))
    }

    #[test]
    fn test_file_chunks_from_a_file_with_two_functions_keeps_both_in_source_order() {
        let two_functions = r#"export function first(value: number): number {
  return value;
}

export function second(value: number): number {
  return value + 1;
}
"#;

        let file_chunks = chunks_at(two_functions, "src/a.ts");

        let ranges: Vec<LineRange> = file_chunks.chunks().iter().map(Chunk::lines).collect();
        assert_eq!(ranges, vec![range(1, 3), range(5, 7)]);
    }

    #[test]
    fn test_file_chunks_from_a_file_with_a_nested_function_keeps_the_outer_and_the_inner() {
        // 別のファイルに同じ形のアロー関数があれば見つけたいので、入れ子の内側も
        // 1 つのチャンクとして返す
        let file_chunks = chunks_at(NESTED_FUNCTIONS, "src/a.ts");

        let ranges: Vec<LineRange> = file_chunks.chunks().iter().map(Chunk::lines).collect();
        assert_eq!(ranges, vec![range(1, 6), range(2, 4)]);
    }

    #[test]
    fn test_file_chunks_from_a_file_without_any_function_keeps_nothing() {
        let only_a_constant = "export const rate = 0.1;\n";

        let file_chunks = chunks_at(only_a_constant, "src/a.ts");

        assert!(file_chunks.chunks().is_empty());
    }

    #[test]
    fn test_file_chunks_from_a_file_gives_every_chunk_the_path_it_was_asked_for() {
        let file_chunks = chunks_at(FUNCTION_AFTER_A_CONSTANT, "src/billing/discount.ts");

        let paths: Vec<&Path> = file_chunks.chunks().iter().map(Chunk::path).collect();
        assert_eq!(paths, vec![Path::new("src/billing/discount.ts")]);
    }

    #[test]
    fn test_file_chunks_from_a_file_with_an_import_gives_every_chunk_that_dependency() {
        let file_chunks = chunks_at(CONSTANT_OUTSIDE_A_FUNCTION, "src/billing/invoice.ts");

        let dependencies: Vec<Option<&ImportSet>> =
            file_chunks.chunks().iter().map(Chunk::imports).collect();
        assert!(
            dependencies.iter().all(Option::is_some),
            "import はファイル全体から採るので、そのファイルのチャンクすべてが持つ"
        );
    }

    #[test]
    fn test_file_chunks_from_a_file_reports_the_start_of_a_function_it_could_not_parse() {
        // 対照として壊れていない関数を同じソースに置く。壊れた側だけが
        // チャンクから外れて、始まりの行として残る
        let sound_then_broken = r#"export function sound(value: number): number {
  return value;
}

function broken() {
  return 1;
"#;

        let file_chunks = chunks_at(sound_then_broken, "src/a.ts");

        let ranges: Vec<LineRange> = file_chunks.chunks().iter().map(Chunk::lines).collect();
        assert_eq!(ranges, vec![range(1, 3)], "壊れた関数はチャンクにしない");
        assert_eq!(
            file_chunks.unparsable_starts(),
            [line(5)],
            "飛ばしたことが始まりの行として残る"
        );
    }

    /// 名前の位置を、行と列で読める形にする。
    fn name_position_of_chunk(chunk: &Chunk) -> Option<(usize, usize)> {
        chunk
            .name_position()
            .map(|position| (position.line().get(), position.character()))
    }

    #[test]
    fn test_chunk_of_a_function_declaration_points_at_its_own_name() {
        // 先頭（`export` の上）ではなく識別子を指す。位置を指す問い合わせは
        // 識別子の上でしか答えない
        let chunk = chunk_at(FUNCTION_AFTER_A_CONSTANT, "a.ts:3").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((3, 16)));
    }

    #[test]
    fn test_chunk_of_an_arrow_function_points_at_the_name_it_is_assigned_to() {
        // アロー関数は自分の名前を持たない。先頭（`(` の上）を指しても答えは返らない
        let assigned_arrow = "export const format = (value: string): string => value;\n";

        let chunk = chunk_at(assigned_arrow, "a.ts:1").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((1, 13)));
    }

    #[test]
    fn test_chunk_of_a_named_function_expression_points_at_the_function_name() {
        // 両方の名前がある形。代入先を指すと変数に書かれた型のほうを見ることになる
        let named_function_expression = "const named = function inner(value: string): string {\n\
                                         \x20 return value;\n\
                                         };\n";

        let chunk = chunk_at(named_function_expression, "a.ts:1").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((1, 23)));
    }

    #[test]
    fn test_chunk_of_a_method_points_at_the_method_name() {
        let class_with_a_method = "export class Cart {\n\
                                   \x20 total(items: number[]): number {\n\
                                   \x20   return items.length;\n\
                                   \x20 }\n\
                                   }\n";

        let chunk = chunk_at(class_with_a_method, "a.ts:2").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((2, 2)));
    }

    #[test]
    fn test_chunk_of_an_anonymous_callback_has_no_name_position() {
        // 対照として同じソースに名前のある関数を 1 つ置く。名前が無いものだけが
        // None になり、「そもそもチャンクが 1 つも無い」で通ってしまわないようにする
        let named_then_anonymous = "export function doubled(values: number[]): number[] {\n\
                                    \x20 return values.map((value) => value * 2);\n\
                                    }\n";

        let file_chunks = chunks_at(named_then_anonymous, "a.ts");

        let positions: Vec<Option<(usize, usize)>> = file_chunks
            .chunks()
            .iter()
            .map(name_position_of_chunk)
            .collect();
        assert_eq!(
            positions,
            vec![Some((1, 16)), None],
            "無名のコールバックだけが名前の位置を持たない"
        );
    }

    #[test]
    fn test_chunk_assigned_to_a_property_points_at_the_property_name() {
        // 代入なので親に name も key も無い。左辺全体の先頭（`obj`）を指すと
        // 入れ物の型が返るので、プロパティのほうを指す
        let assigned_to_a_property = "obj.handler = (value: string): string => value;\n";

        let chunk = chunk_at(assigned_to_a_property, "a.ts:1").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((1, 4)));
    }

    #[test]
    fn test_chunk_assigned_to_a_variable_points_at_the_variable_name() {
        // 宣言と代入が別の行に分かれている形。宣言側にチャンクは無い
        let assigned_later = "let later;\n\
                              later = (value: string): string => value;\n";

        let chunk = chunk_at(assigned_later, "a.ts:2").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((2, 0)));
    }

    #[test]
    fn test_chunk_assigned_to_a_computed_key_has_no_name_position() {
        // 対照として同じソースにプロパティへの代入を 1 件置く。添字の側だけが
        // 名前を持たない（`target["key"]` に問い合わせられる識別子は無い）
        let assigned_to_a_property_then_a_key = "obj.handler = (value: string): string => value;\n\
             target[\"keyed\"] = (value: string): string => value;\n";

        let file_chunks = chunks_at(assigned_to_a_property_then_a_key, "a.ts");

        let positions: Vec<Option<(usize, usize)>> = file_chunks
            .chunks()
            .iter()
            .map(name_position_of_chunk)
            .collect();
        assert_eq!(
            positions,
            vec![Some((1, 4)), None],
            "添字への代入だけが名前の位置を持たない"
        );
    }

    #[test]
    fn test_chunk_whose_line_has_non_ascii_before_the_name_counts_the_column_in_utf16() {
        // 名前より手前に非 ASCII がある形。バイトで数えていると列が 32 になり、
        // サーバは名前より後ろを見る
        let non_ascii_before_the_name =
            "const handlers = { \"請求\": 1, format: (value: string): string => value };\n";

        let chunk = chunk_at(non_ascii_before_the_name, "a.ts:1").expect("切り出せる");

        assert_eq!(name_position_of_chunk(&chunk), Some((1, 28)));
    }

    /// そのチャンクのシグネチャに書かれた型名の綴り。
    fn type_names_of(chunk: &Chunk) -> Vec<&str> {
        chunk
            .type_references()
            .iter()
            .map(TypeReference::name)
            .collect()
    }

    #[test]
    fn test_chunk_type_references_cover_the_parameters_and_the_return_type() {
        let annotated =
            "export function scale(amount: Amount, rate: Rate): Total {\n  return amount;\n}\n";

        let chunk = chunk_at(annotated, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Amount", "Rate", "Total"]);
    }

    #[test]
    fn test_chunk_type_references_leave_out_the_keyword_types() {
        // 対照として名前の付いた型を 1 つ置く。キーワードの型まで集めると、
        // 尋ねる相手が居ない名前へ問い合わせを送ることになる
        let mixed =
            "export function scale(amount: number, rate: Rate): string {\n  return \"\";\n}\n";

        let chunk = chunk_at(mixed, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Rate"]);
    }

    #[test]
    fn test_chunk_type_references_leave_out_the_type_variables_it_declares() {
        // 型変数を解決するとファイルごとに違う結果になり、総称型どうしが
        // 単一化できなくなる。対照として制約に書かれた型名を 1 つ置く
        let generic =
            "export function pick<T extends Amount>(items: T[]): T {\n  return items[0];\n}\n";

        let chunk = chunk_at(generic, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Amount"]);
    }

    #[test]
    fn test_chunk_type_references_leave_out_a_type_variable_declared_inside_the_signature() {
        // 内側の `<T>` が外側の綴りを覆う形。外側だけを解決すると、差し込みが
        // 内側の宣言まで書き換えて `<number>(x: number) => number` になる。
        // 対照として、覆われていない型名を 1 つ置く
        let shadowed =
            "export function run(a: T, rate: Rate, cb: <T>(x: T) => T): void {\n  cb(a);\n}\n";

        let chunk = chunk_at(shadowed, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Rate"]);
    }

    #[test]
    fn test_chunk_type_references_leave_out_a_name_bound_by_a_mapped_type() {
        // `[K in "a"]` の `K` はこのシグネチャの中でだけ意味を持つ。外側の同じ綴りを
        // 解決すると、差し込みが `{ [string in "a"]: string }` を作る。
        // 対照として、束縛されていない型名を 1 つ置く
        let mapped = "export function pick(x: K, m: { [K in \"a\"]: K }, rate: Rate): void {\n  return;\n}\n";

        let chunk = chunk_at(mapped, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Rate"]);
    }

    #[test]
    fn test_chunk_type_references_keep_the_constraint_of_a_mapped_type() {
        // 対照は上のテスト。`[K in Keys]` の `Keys` は制約であって束縛ではない
        let constrained =
            "export function pick(m: { [K in Keys]: number }): void {\n  return;\n}\n";

        let chunk = chunk_at(constrained, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Keys"]);
    }

    #[test]
    fn test_chunk_type_references_name_the_same_type_only_once() {
        // 尋ねる先は綴りごとに 1 箇所でよい。畳まないと同じ名前へ 2 度送る
        let repeated =
            "export function scale(amount: Amount, other: Amount): Amount {\n  return amount;\n}\n";

        let chunk = chunk_at(repeated, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Amount"]);
    }

    #[test]
    fn test_chunk_type_references_of_an_anonymous_chunk_cover_the_annotation_it_is_assigned_to() {
        // 無名のチャンクでは hover が代入先の名前を指すので、返る綴りは
        // `const aliased: Handler`。注釈を集めないと、この形は解決できない
        let assigned = "export const aliased: Handler = (value) => value.length;\n";

        let chunk = chunk_at(assigned, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Handler"]);
    }

    #[test]
    fn test_chunk_type_references_of_a_named_chunk_leave_out_the_annotation_it_is_assigned_to() {
        // 対照は上のテスト。自分の名前を持つので hover は関数自身の型を返し、
        // 代入先の注釈は綴りに現れない
        let named =
            "const named: Formatter = function inner(value: Text): Text {\n  return value;\n};\n";

        let chunk = chunk_at(named, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Text"]);
    }

    #[test]
    fn test_chunk_type_references_cover_the_arguments_of_a_generic_type() {
        // 総称型は名前と型引数が同じ部分木にいる。名前で止めると型引数を数え落とす
        let nested = "export function unwrap(box: Box<User>): User {\n  return box.value;\n}\n";

        let chunk = chunk_at(nested, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["Box", "User"]);
    }

    #[test]
    fn test_chunk_type_reference_points_at_the_name_in_the_source() {
        // 位置がずれると、サーバは別の識別子について答える
        let annotated = "export function scale(amount: Amount): number {\n  return amount;\n}\n";

        let chunk = chunk_at(annotated, "a.ts:1").expect("切り出せる");

        let reference = chunk
            .type_references()
            .first()
            .expect("型名が 1 つ書かれている");
        assert_eq!(reference.position().line(), line(1));
        assert_eq!(reference.position().character(), 30);
    }

    #[test]
    fn test_chunk_type_references_hold_a_qualified_type_name_as_one_spelling() {
        // 末尾の `Amount` だけを集めると、差し込みが `money.number` を作る。
        // 対照として修飾されていない型名を 1 つ置く
        let qualified =
            "export function scale(amount: money.Amount, rate: Rate): number {\n  return 0;\n}\n";

        let chunk = chunk_at(qualified, "a.ts:1").expect("切り出せる");

        assert_eq!(type_names_of(&chunk), vec!["money.Amount", "Rate"]);
    }

    #[test]
    fn test_a_qualified_type_name_is_asked_about_at_its_leaf() {
        // 先頭の `money` は名前空間で、そこへ尋ねても型の宣言は返らない
        let qualified = "export function scale(amount: money.Amount): number {\n  return 0;\n}\n";

        let chunk = chunk_at(qualified, "a.ts:1").expect("切り出せる");
        let asked = chunk
            .type_references()
            .first()
            .expect("型名が 1 つある")
            .position();

        // `money.` の分だけ後ろを指す
        assert_eq!(asked.character(), 36);
    }

    #[test]
    fn test_chunk_type_references_of_a_signature_written_with_keyword_types_only_are_empty() {
        let plain = "export function scale(amount: number): number {\n  return amount;\n}\n";

        let chunk = chunk_at(plain, "a.ts:1").expect("切り出せる");

        assert!(chunk.type_references().is_empty());
    }
}
