//! ソース 1 ファイル分の構文木。
//!
//! tree-sitter でソースを読み、ノードを歩くところまでを持つ。**どのノードが
//! チャンクか / import かは知らない**。その語彙はチャンクなら `syntax::chunk`、
//! import なら `syntax::import` が持つ (rules/naming.md「このツールの語彙を固定する」)。
//!
//! パースはプロセスを起動せずメモリの中だけで済むので、`syntax` に I/O を
//! 持ち込まない (rules/coding.md「禁止事項」)。

use std::error::Error;
use std::fmt;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::line_number::LineNumber;
use crate::source_position::SourcePosition;

/// ソースを読むのに使う grammar。
///
/// TypeScript と TSX は tree-sitter では別の grammar で、**片方で兼ねられない**。
/// JSX は TypeScript の grammar では構文エラーになり、型アサーション `<T>value` は
/// TSX の grammar では JSX の開始タグとして読まれる。どちらで読むかは拡張子で決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    /// `.ts`
    TypeScript,
    /// `.tsx`
    Tsx,
}

impl Grammar {
    /// そのファイルを読むための grammar。読める拡張子でなければ `None`。
    ///
    /// **読める拡張子の一覧をここ 1 箇所に置く。** 走査の対象を決める側（`codebase`）が
    /// 別の一覧を持つと、拡張子を足したときに片方だけが古くなる。
    pub fn of_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }

    /// tree-sitter に渡す言語。
    fn language(self) -> tree_sitter::Language {
        match self {
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

/// パース済みの構文木と、その元になったソース。
///
/// ソースを一緒に持つのは、ノードが指すのがバイト範囲だけで、テキストを取り出すには
/// 元のソースが要るため。別々に持ち回すと、**別のファイルのソースでノードのテキストを
/// 取り出す**組み合わせが作れてしまう
/// (rules/coding.md「不正な状態を型で表現できなくする」)。
#[derive(Debug)]
pub struct SyntaxTree<'source> {
    tree: tree_sitter::Tree,
    source: &'source str,
}

impl<'source> SyntaxTree<'source> {
    /// ソースを、指定した grammar で構文木にする。
    ///
    /// `grammar` は [`Grammar::of_path`] が拡張子から決めたもの。
    ///
    /// 構文エラーのあるソースでも木は返る（tree-sitter は壊れた場所を木の中に印として
    /// 残す）。**壊れているかを決めるのは呼び出し側**で、ここでは木を作れたかどうかだけを返す。
    ///
    /// # Errors
    ///
    /// grammar を実行時が受け付けなかった / パーサが木を返さなかったとき。
    pub fn from_source(source: &'source str, grammar: Grammar) -> Result<Self, ParseError> {
        let mut parser = Parser::new();
        parser
            .set_language(&grammar.language())
            .map_err(|_| ParseError::GrammarRejected)?;

        let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;

        Ok(Self { tree, source })
    }

    /// 木の中の名前付きノードを、根から前順（ソースに書かれた順）で返す。
    ///
    /// 名前付きだけを返すのは、`{` や `,` のような字句そのもののノードが
    /// 構造を表さないため。特定の字句が要る場面では、返ったノードから
    /// [`Node::child`] で降りる。
    ///
    /// **歩き方をここ 1 箇所に置く。** チャンクの切り出しと import の収集が
    /// どちらも木を歩くので、両方に書くと同じ処理が 2 箇所に現れる。
    pub fn named_descendants(&self) -> Vec<Node<'_>> {
        let mut nodes = Vec::new();
        let mut pending = vec![self.tree.root_node()];

        while let Some(node) = pending.pop() {
            nodes.push(node);

            let mut cursor = node.walk();
            let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
            // 逆順に積む。取り出すのが末尾からなので、そのまま積むと兄弟が逆に出る
            pending.extend(children.into_iter().rev());
        }

        nodes
    }

    /// 木のどこかに壊れた場所が残っているか。
    ///
    /// **根から見る。** 欠けた字句は名前を持たないノードとして木に残ることがあり
    /// （`Map<string` の閉じ `>`）、[`SyntaxTree::named_descendants`] は
    /// **名前付きだけを返す**ので、そこを歩いても見つからない。
    pub fn has_error(&self) -> bool {
        self.tree.root_node().has_error()
    }

    /// そのノードが覆っているソースのテキスト。
    ///
    /// ノードのバイト範囲が文字の境界に乗っていないときは `None`。
    pub fn text_of(&self, node: Node<'_>) -> Option<&'source str> {
        self.source.get(node.byte_range())
    }

    /// 元になったソース。
    pub fn source(&self) -> &'source str {
        self.source
    }
}

/// そのノードの先頭が置かれている位置。バイト範囲が文字の境界に乗っていなければ `None`。
///
/// `source` はそのノードを含むファイル全体のソース。列を UTF-16 のコード単位で数え直すのに、
/// 行の先頭からノードの手前までの文字列が要る（[`SourcePosition::from_preceding_text`]）。
///
/// **歩き方と同じく、位置の数え直しもここ 1 箇所に置く。** チャンクの名前
/// （`syntax::chunk`）と型名（`syntax::type_reference`）がどちらも同じ数え直しを要る。
pub(super) fn source_position_of(node: Node<'_>, source: &str) -> Option<SourcePosition> {
    let start = node.start_byte();
    let line_start = source
        .get(..start)?
        .rfind('\n')
        .map_or(0, |index| index + '\n'.len_utf8());

    Some(SourcePosition::from_preceding_text(
        LineNumber::from_index(node.start_position().row),
        source.get(line_start..start)?,
    ))
}

/// 構文木を作れなかった理由。
///
/// 2 つに分けているのは直す先が違うため。grammar が弾かれたのは依存の版の問題で、
/// 木が返らなかったのはパースが中断された結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// tree-sitter の実行時が grammar を受け付けなかった。
    GrammarRejected,
    /// パーサが木を返さなかった。
    NoTree,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GrammarRejected => write!(
                formatter,
                "TypeScript の grammar が tree-sitter の実行時と噛み合っていません"
            ),
            Self::NoTree => write!(formatter, "ソースをパースできませんでした"),
        }
    }
}

impl Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const A_FUNCTION: &str = r#"export function applyDiscount(price: number): number {
  return price * 0.9;
}
"#;

    /// JSX を返す関数。TypeScript の grammar では読めない。
    const A_FUNCTION_RETURNING_JSX: &str = r#"export function Badge(label: string) {
  return <span className="badge">{label}</span>;
}
"#;

    /// 型アサーション。TSX の grammar では JSX の開始タグとして読まれる。
    const A_FUNCTION_WITH_A_TYPE_ASSERTION: &str = r#"export function widen(value: unknown) {
  return <string>value;
}
"#;

    fn tree_of(source: &str) -> SyntaxTree<'_> {
        SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる")
    }

    fn has_error(source: &str, grammar: Grammar) -> bool {
        let tree =
            SyntaxTree::from_source(source, grammar).expect("テストが渡すソースは木にできる");

        tree.named_descendants()
            .into_iter()
            .any(|node| node.has_error())
    }

    #[test]
    fn test_grammar_of_a_typescript_path_reads_it_as_typescript() {
        assert_eq!(
            Grammar::of_path(Path::new("src/billing/discount.ts")),
            Some(Grammar::TypeScript)
        );
    }

    #[test]
    fn test_grammar_of_a_tsx_path_reads_it_as_tsx() {
        // 既定と違う値を選ぶ。`.ts` と同じ grammar になっていたら区別が付かない
        assert_eq!(
            Grammar::of_path(Path::new("src/report/Badge.tsx")),
            Some(Grammar::Tsx)
        );
    }

    #[test]
    fn test_grammar_of_a_path_with_another_extension_is_not_readable() {
        // 対照は上の 2 つ。読める拡張子と同じディレクトリにあっても選べない
        assert_eq!(Grammar::of_path(Path::new("src/report/notes.md")), None);
        assert_eq!(Grammar::of_path(Path::new("src/report/Badge")), None);
    }

    #[test]
    fn test_syntax_tree_of_jsx_read_as_tsx_has_no_error() {
        assert!(!has_error(A_FUNCTION_RETURNING_JSX, Grammar::Tsx));
    }

    #[test]
    fn test_syntax_tree_of_jsx_read_as_typescript_marks_an_error() {
        // 対照は上のテスト。**拡張子だけ増やして grammar を選ばないと、
        // `.tsx` の関数が丸ごと「構文エラーで切り出せない」に落ちる**
        assert!(has_error(A_FUNCTION_RETURNING_JSX, Grammar::TypeScript));
    }

    #[test]
    fn test_syntax_tree_of_a_type_assertion_read_as_tsx_marks_an_error() {
        // 逆向きの非対称。TSX で両方を兼ねられないので、拡張子で選ぶ必要がある
        assert!(has_error(A_FUNCTION_WITH_A_TYPE_ASSERTION, Grammar::Tsx));
        assert!(!has_error(
            A_FUNCTION_WITH_A_TYPE_ASSERTION,
            Grammar::TypeScript
        ));
    }

    #[test]
    fn test_syntax_tree_of_a_function_contains_the_function_declaration_node() {
        let tree = tree_of(A_FUNCTION);

        let kinds: Vec<&str> = tree
            .named_descendants()
            .iter()
            .map(|node| node.kind())
            .collect();

        assert!(
            kinds.contains(&"function_declaration"),
            "関数宣言がノードとして出る: {kinds:?}"
        );
    }

    #[test]
    fn test_syntax_tree_returns_named_descendants_in_source_order() {
        // 兄弟を積む向きを逆にすると、2 つの関数が入れ替わって出る
        let two_functions = "function first() {}\nfunction second() {}\n";
        let tree = tree_of(two_functions);

        let names: Vec<&str> = tree
            .named_descendants()
            .iter()
            .filter(|node| node.kind() == "identifier")
            .filter_map(|node| tree.text_of(*node))
            .collect();

        assert_eq!(names, vec!["first", "second"]);
    }

    #[test]
    fn test_syntax_tree_gives_the_text_a_node_covers() {
        let tree = tree_of(A_FUNCTION);

        let declaration = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_declaration")
            .expect("関数宣言がある");

        assert_eq!(
            tree.text_of(declaration),
            Some("function applyDiscount(price: number): number {\n  return price * 0.9;\n}")
        );
    }

    #[test]
    fn test_syntax_tree_of_a_source_with_a_syntax_error_is_still_built() {
        // 壊れているかを決めるのは呼び出し側。ここで木を返さないと、
        // 「どこが壊れているか」を後段が見られなくなる
        let unterminated = "function broken() {\n  return 1;\n";

        let tree = SyntaxTree::from_source(unterminated, Grammar::TypeScript);

        assert!(tree.is_ok(), "構文エラーがあっても木は返る");
    }

    #[test]
    fn test_syntax_tree_of_a_source_with_a_syntax_error_marks_the_error() {
        // 対照は上のテスト。木が返ることと、壊れていると分かることは別
        let unterminated = "function broken() {\n  return 1;\n";
        let tree = tree_of(unterminated);

        // 探すのが `function_declaration` ではないのは、閉じブレースが欠けると
        // tree-sitter が宣言として復元できず、式文の中の関数式として組み直すため。
        // `function broken()` と書いてあっても、木に宣言のノードは現れない
        let broken = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_expression")
            .expect("閉じていない関数もノードにはなる");

        assert!(broken.has_error(), "閉じブレースの欠けが印として残る");
    }
}
