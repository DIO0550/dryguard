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

use tree_sitter::{Node, Parser};

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
    /// TypeScript のソースを構文木にする。
    ///
    /// 構文エラーのあるソースでも木は返る（tree-sitter は壊れた場所を木の中に印として
    /// 残す）。**壊れているかを決めるのは呼び出し側**で、ここでは木を作れたかどうかだけを返す。
    ///
    /// # Errors
    ///
    /// grammar を実行時が受け付けなかった / パーサが木を返さなかったとき。
    pub fn from_typescript(source: &'source str) -> Result<Self, ParseError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
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

    const A_FUNCTION: &str = r#"export function applyDiscount(price: number): number {
  return price * 0.9;
}
"#;

    fn tree_of(source: &str) -> SyntaxTree<'_> {
        SyntaxTree::from_typescript(source).expect("テストが渡すソースは木にできる")
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

        let tree = SyntaxTree::from_typescript(unterminated);

        assert!(tree.is_ok(), "構文エラーがあっても木は返る");
    }

    #[test]
    fn test_syntax_tree_of_a_source_with_a_syntax_error_marks_the_error() {
        // 対照は上のテスト。木が返ることと、壊れていると分かることは別
        let unterminated = "function broken() {\n  return 1;\n";
        let tree = tree_of(unterminated);

        let broken = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_expression")
            .expect("閉じていない関数もノードにはなる");

        assert!(broken.has_error(), "閉じブレースの欠けが印として残る");
    }
}
