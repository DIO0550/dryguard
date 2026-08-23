//! 構文木を正規化したトークン列と、その重なり。
//!
//! 識別子は名前を、リテラルは値を捨てる。**名前や値の違いで類似度が落ちない**ようにするのが
//! 正規化の目的で、そこだけが違う同じ構造は同じトークン列になる
//! （`docs/dryguard-plan.md`「Stage 1: 候補抽出」の Type-2 相当）。
//! 名前そのものが要る判定（依存先のドメインなど）は `syntax::import` と Stage 2 / Stage 3 の担当。
//!
//! **正規化の粒度は [`normalization_of`] の表 1 箇所で決まる。** どこまで潰すかを
//! 後から変えられるようにするのが、Stage 1 を外部ツールへ委譲しない理由の 1 つ
//! （`docs/dryguard-plan.md`「差別化ポイント」）。

use std::collections::HashMap;

use tree_sitter::Node;

use crate::similarity::Similarity;

/// 正規化したトークン。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    /// 名前を持つもの。名前そのものは捨てる
    Identifier,
    /// リテラル。値は捨てて型だけ残す
    Literal(LiteralType),
    /// それ以外の構文。ノードの種別を残す
    Syntax(SyntaxKind),
}

/// リテラルの型タグ。
///
/// 値を捨てて型だけ残すので、`0.1` と `42` は同じ [`LiteralType::Number`] になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiteralType {
    Number,
    Text,
    Boolean,
    Null,
    Undefined,
    Regex,
}

impl LiteralType {
    /// そのノード種別が表すリテラルの型。リテラルでなければ `None`。
    fn from_kind(kind: &str) -> Option<Self> {
        LITERAL_KINDS
            .iter()
            .find(|(literal_kind, _)| *literal_kind == kind)
            .map(|(_, literal_type)| *literal_type)
    }
}

/// 構文ノードの種別。記号や予約語では、その字句そのもの（`"("` / `"const"`）。
///
/// 中身を作れるのはこのモジュールの中だけで、材料は tree-sitter が返した
/// [`Node::kind`] に限る。**grammar が知らない種別を持つ状態が作れない**
/// (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyntaxKind(&'static str);

/// ノード 1 つを正規化した結果と、その子をどう扱うか。
///
/// 「トークンにするか」と「子へ降りるか」を別々の値で持つと、**落とすのに子へ降りる**
/// のような組み合わせが作れてしまう (rules/coding.md「不正な状態を型で表現できなくする」)。
enum Normalization {
    /// トークン 1 つに潰し、子は見ない。
    Collapsed(Token),
    /// トークンを 1 つ出し、子も続けて見る。
    Expanded(Token),
    /// トークンにしない。子も見ない。
    Dropped,
}

/// 名前として潰すノードの種別。
///
/// 型名（`type_identifier` / `predefined_type`）を混ぜているのは、**ここが構造だけを見る層**で、
/// 型の違いは Stage 2 が見るため。`x: number` と `x: string` を別物にすると、
/// 型注釈を書き換えただけで構造が違って見える。
const NAME_KINDS: [&str; 8] = [
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
    "private_property_identifier",
    "statement_identifier",
    "type_identifier",
    "predefined_type",
];

/// リテラルのノード種別と、その型タグ。
///
/// `string_fragment` が要るのは、テンプレートリテラルへ降りるため。`string` は
/// そこで潰れるので `string_fragment` が現れないが、`` `a ${b} c` `` では
/// 文字の部分が `string_fragment` として出てくる。
const LITERAL_KINDS: [(&str, LiteralType); 8] = [
    ("number", LiteralType::Number),
    ("string", LiteralType::Text),
    ("string_fragment", LiteralType::Text),
    ("regex", LiteralType::Regex),
    ("true", LiteralType::Boolean),
    ("false", LiteralType::Boolean),
    ("null", LiteralType::Null),
    ("undefined", LiteralType::Undefined),
];

/// コメントのノード種別。
const COMMENT_KIND: &str = "comment";

/// 突き合わせる並びの長さ。
///
/// 1 にすると並びを見ないのと同じで、長くすると少しの違いで共通する並びが無くなる。
/// クローン検出で使われる 3-5 の下限を採る。
const GRAM_LENGTH: usize = 3;

/// ノードの種別から、正規化の仕方を決める。
///
/// **正規化の粒度はここだけで決まる。** 潰しすぎると別物が似て見え、潰さなすぎると
/// 名前の違いで落ちるので、調整はこの表を書き換えて行う。
fn normalization_of(kind: &'static str) -> Normalization {
    if NAME_KINDS.contains(&kind) {
        return Normalization::Collapsed(Token::Identifier);
    }
    if let Some(literal_type) = LiteralType::from_kind(kind) {
        return Normalization::Collapsed(Token::Literal(literal_type));
    }
    if kind == COMMENT_KIND {
        return Normalization::Dropped;
    }

    Normalization::Expanded(Token::Syntax(SyntaxKind(kind)))
}

/// そのノードが覆う範囲を、正規化トークンの列にする。書かれた順に並ぶ。
///
/// 記号や予約語のノードも読む。落とすと `a + b` と `a - b` が同じ列になり、
/// **Type-2 クローンが許す違い（名前・値・型）を超えたものまで同じに見える**。
///
/// リテラルの中へは降りない。`/ab-[0-9]+/` の中身は値であって構造ではないので、
/// 降りると正規表現を書き換えただけで構造が違って見える。
pub fn tokens_of(node: Node<'_>) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pending = vec![node];

    while let Some(current) = pending.pop() {
        match normalization_of(current.kind()) {
            Normalization::Dropped => continue,
            Normalization::Collapsed(token) => tokens.push(token),
            Normalization::Expanded(token) => {
                tokens.push(token);

                let mut cursor = current.walk();
                let children: Vec<Node<'_>> = current.children(&mut cursor).collect();
                // 逆順に積む。取り出すのが末尾からなので、そのまま積むと兄弟が逆に出る
                pending.extend(children.into_iter().rev());
            }
        }
    }

    tokens
}

/// チャンク 1 つ分の、正規化トークンの列。
///
/// 空では作れない。トークンが 1 つも取れなかったことを空の列として通すと、
/// 後段が「共通する並びが無い」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSequence(Vec<Token>);

impl TokenSequence {
    /// そのノードが覆う範囲を正規化トークンの列にする。
    ///
    /// トークンが 1 つも取れなかったとき（コメントだけを覆うノードなど）は `None`。
    pub fn from_node(node: Node<'_>) -> Option<Self> {
        let tokens = tokens_of(node);
        if tokens.is_empty() {
            return None;
        }
        Some(Self(tokens))
    }

    /// 2 つのトークン列の似かた。1.0 が完全一致。
    ///
    /// 長さ [`GRAM_LENGTH`] の並び（gram）を**出現回数を数えたまま**突き合わせ、
    /// 共通している分が合わせたうちの何割かを返す。
    ///
    /// **回数を数えるのは、同じ並びの繰り返し回数だけが違うペアを見分けるため。**
    /// 集合として見ると `x;` と `x; x; x;` が完全一致になる。
    pub fn similarity_with(&self, other: &Self) -> Similarity {
        let mine = gram_counts_of(&self.0);
        let theirs = gram_counts_of(&other.0);

        let mut shared = 0;
        let mut combined = 0;
        for (gram, count) in &mine {
            let matching = theirs.get(gram).copied().unwrap_or(0);
            shared += (*count).min(matching);
            combined += (*count).max(matching);
        }
        for (gram, count) in &theirs {
            if !mine.contains_key(gram) {
                combined += *count;
            }
        }

        Similarity::from_shared_count(shared, combined)
    }
}

/// トークン列に現れる gram と、その出現回数。
///
/// 列が [`GRAM_LENGTH`] に満たないときは列全体で 1 つの gram にする。切り出せないことを
/// 空として返すと、短い関数どうしがどれも「共通する並びが無い」になる。
fn gram_counts_of(tokens: &[Token]) -> HashMap<&[Token], usize> {
    let mut counts = HashMap::new();

    if tokens.len() < GRAM_LENGTH {
        counts.insert(tokens, 1);
        return counts;
    }

    for gram in tokens.windows(GRAM_LENGTH) {
        *counts.entry(gram).or_insert(0) += 1;
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::tree::SyntaxTree;

    fn syntax(kind: &'static str) -> Token {
        Token::Syntax(SyntaxKind(kind))
    }

    /// ソース全体を覆うノードの、正規化トークン列。
    ///
    /// `named_descendants` は根から前順に返すので、先頭が根になる。
    fn tokens(source: &str) -> Vec<Token> {
        let tree = SyntaxTree::from_typescript(source).expect("テストが渡すソースは木にできる");
        let root = *tree.named_descendants().first().expect("木には根がある");

        tokens_of(root)
    }

    fn sequence(source: &str) -> TokenSequence {
        let tree = SyntaxTree::from_typescript(source).expect("テストが渡すソースは木にできる");
        let root = *tree.named_descendants().first().expect("木には根がある");

        TokenSequence::from_node(root).expect("テストが渡すソースにはトークンがある")
    }

    /// ソースの中の、その種別のノードだけを覆うトークン列。
    fn sequence_of_kind(source: &str, kind: &str) -> TokenSequence {
        let tree = SyntaxTree::from_typescript(source).expect("テストが渡すソースは木にできる");
        let node = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == kind)
            .expect("テストが渡すソースにその種別のノードがある");

        TokenSequence::from_node(node).expect("テストが渡すノードにはトークンがある")
    }

    fn similarity(source_a: &str, source_b: &str) -> f64 {
        sequence(source_a)
            .similarity_with(&sequence(source_b))
            .value()
    }

    #[test]
    fn test_tokens_of_an_identifier_drop_its_name() {
        assert_eq!(
            tokens("invoice"),
            vec![
                syntax("program"),
                syntax("expression_statement"),
                Token::Identifier
            ]
        );
    }

    #[test]
    fn test_tokens_of_sources_that_differ_only_in_names_are_the_same() {
        // 名前の違いで類似度が落ちないようにするのが正規化の目的
        assert_eq!(
            tokens("const discounted = invoice.amount;"),
            tokens("const shortage = stock.quantity;")
        );
    }

    #[test]
    fn test_tokens_of_a_number_literal_drop_its_value() {
        assert_eq!(
            tokens("0.1"),
            tokens("42"),
            "値を捨てるので桁も表記も類似度に効かない"
        );
    }

    #[test]
    fn test_tokens_of_a_string_literal_drop_its_content() {
        // 中の語をトークンにすると、文字列を書き換えただけで構造が違って見える
        assert_eq!(
            tokens("\"const invoice\""),
            vec![
                syntax("program"),
                syntax("expression_statement"),
                Token::Literal(LiteralType::Text)
            ]
        );
    }

    #[test]
    fn test_tokens_of_a_regex_literal_drop_its_pattern() {
        // 正規表現の中身は値。文字を前から見る切り方ではここがコードとして読まれていた
        assert_eq!(tokens("/ab-[0-9]+/g"), tokens("/xy/"));
    }

    #[test]
    fn test_tokens_of_a_boolean_literal_are_the_same_for_true_and_false() {
        assert_eq!(tokens("const flag = true;"), tokens("const flag = false;"));
    }

    #[test]
    fn test_tokens_of_a_type_annotation_drop_which_type_it_is() {
        // 型の違いを見るのは Stage 2。ここで別物にすると型注釈を書き換えただけで落ちる
        assert_eq!(
            tokens("function f(x: number) {}"),
            tokens("function f(x: string) {}")
        );
    }

    #[test]
    fn test_tokens_keep_which_operator_was_used() {
        // 対照は上の型注釈のテスト。名前・値・型は潰すが、演算子は潰さない
        assert_ne!(tokens("a + b"), tokens("a - b"));
    }

    #[test]
    fn test_tokens_of_a_template_literal_keep_the_structure_inside_its_substitutions() {
        // `${}` の中は式であって値ではない。丸ごと 1 つの文字列に潰すと構造が消える
        assert_ne!(tokens("`a ${b} c`"), tokens("`a ${b.c()} c`"));
    }

    #[test]
    fn test_tokens_of_a_template_literal_drop_the_text_around_its_substitutions() {
        // 対照は上のテスト。式の構造は残すが、文字の部分は値なので潰す
        assert_eq!(tokens("`a ${b} c`"), tokens("`x ${b} y`"));
    }

    #[test]
    fn test_tokens_skip_a_line_comment_but_keep_the_next_line() {
        assert_eq!(tokens("// const invoice\n1"), tokens("1"));
    }

    #[test]
    fn test_tokens_skip_a_block_comment_but_keep_what_follows() {
        assert_eq!(tokens("/* const invoice */ 1"), tokens("1"));
    }

    #[test]
    fn test_token_sequence_of_a_node_covering_only_a_comment_cannot_be_created() {
        // 「トークンが取れなかった」を空の列として通すと、後段が
        // 「似ていない」と「見ていない」を区別できない
        let source = "// 何も無い";
        let tree = SyntaxTree::from_typescript(source).expect("木にできる");
        let comment = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "comment")
            .expect("コメントのノードがある");

        assert_eq!(TokenSequence::from_node(comment), None);
    }

    #[test]
    fn test_similarity_of_the_same_source_is_one() {
        let source = "function f() { return Math.max(shortage, 0); }";

        assert_eq!(similarity(source, source), 1.0);
    }

    #[test]
    fn test_similarity_of_sources_that_differ_only_in_names_is_one() {
        let discount = "function applyDiscount(invoice: Invoice): number {\n  \
                        const discounted = invoice.amount * (1 - RATE);\n  \
                        return Math.max(discounted, 0);\n}";
        let reorder = "function reorderAmount(stock: Stock): number {\n  \
                       const shortage = stock.quantity * (1 - LIMIT);\n  \
                       return Math.max(shortage, 0);\n}";

        assert_eq!(similarity(discount, reorder), 1.0);
    }

    #[test]
    fn test_similarity_of_sources_that_differ_only_in_a_regex_pattern_is_one() {
        // 文字を前から見る切り方では、正規表現の中身がコードとして読まれて落ちていた。
        // 片方にフラグを付けてあるのは、リテラルの中へ降りる実装なら
        // フラグのノードが差として残り、このテストが落ちるようにするため
        let long_pattern =
            "function isCode(text: string): boolean { return /ab-[0-9]+/g.test(text); }";
        let short_pattern = "function isTag(value: string): boolean { return /xy/.test(value); }";

        assert_eq!(similarity(long_pattern, short_pattern), 1.0);
    }

    #[test]
    fn test_similarity_of_sources_that_repeat_a_statement_a_different_number_of_times_is_below_one()
    {
        // 対照は上の 2 つ。名前と値は潰すが、同じ並びを何度書いたかは潰さない
        let once = "function once(invoice: number): number {\n  return invoice;\n}";
        let thrice = "function thrice(stock: number): number {\n  \
                      stock; stock; stock;\n  return stock;\n}";
        let ratio = similarity(once, thrice);

        assert!(
            ratio < 1.0,
            "繰り返しの回数だけが違うペアは完全一致にならない: {ratio}"
        );
    }

    #[test]
    fn test_similarity_of_a_source_and_its_periodic_repetition_is_below_one() {
        // 境界。gram を集合で数えると、周期が gram の長さ以下の繰り返しは
        // 同じ gram しか増やさないので完全一致に戻る
        let twice = "a; b; a; b;";
        let three_times = "a; b; a; b; a; b;";
        let ratio = similarity(twice, three_times);

        assert!(ratio < 1.0, "周期的な繰り返しでも回数の違いが残る: {ratio}");
    }

    #[test]
    fn test_similarity_of_structurally_different_sources_is_below_one() {
        // 名前を潰した結果すべてが似て見えるなら、このシグナルは何も言っていない
        let assignment = "function f() { const discounted = invoice.amount; }";
        let loop_over_items = "function f() { for (const item of items) { total = total + 1; } }";

        assert!(
            similarity(assignment, loop_over_items) < 1.0,
            "構造が違うペアは完全一致にならない"
        );
    }

    #[test]
    fn test_similarity_of_sources_without_a_common_gram_is_zero() {
        assert_eq!(similarity("invoice", "class C {}"), 0.0);
    }

    #[test]
    fn test_similarity_of_sequences_shorter_than_a_gram_compares_them_whole() {
        // 境界。gram を切り出せない短さを「並びが無い」として通すと、共通 0 / 合わせて 0 の
        // 完全一致に落ちて、まったく違うものが 1.00 で返る
        let identifier = sequence_of_kind("invoice", "identifier");
        let number = sequence_of_kind("42", "number");

        assert_eq!(identifier.similarity_with(&number).value(), 0.0);
    }
}
