//! 正規化したトークンと、その集合の重なり。
//!
//! Phase 0 の素朴実装。tree-sitter は使わず、文字を前から見てトークンに切る
//! （`docs/dryguard-plan.md`「Phase 0: 貫通させる (LSPなし)」）。AST 正規化は Phase 1。
//!
//! テンプレートリテラルの `${}` の中と正規表現リテラルは、この切り方では読めない。
//! どちらも 1 つの [`Token::Text`] / 記号の並びとして潰れる。ここを詰めるより
//! tree-sitter へ移すほうが確実なので、Phase 0 では踏み込まない。

use std::collections::HashSet;

use crate::similarity::Similarity;
use crate::syntax::source_character::{is_quote, is_word_part, is_word_start};

/// 正規化したトークン。
///
/// 識別子とリテラルは中身を捨てる。**名前の違いで類似度が落ちない**ようにするのが
/// 正規化の目的で、名前が違うだけの同じ構造は同じトークン列になる。
/// 名前そのものが要る判定（依存先のドメインなど）は Stage 2 / Stage 3 の担当。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    /// 予約語。語を残す
    Keyword(Keyword),
    /// 識別子。名前は捨てる
    Identifier,
    /// 数値リテラル。値は捨てる
    Number,
    /// 文字列・テンプレートリテラル。中身は捨てる
    Text,
    /// 記号・演算子。1 文字ずつ持つ
    Symbol(char),
}

/// ソースを正規化トークンの列にする。空白とコメントは落とす。
pub fn tokens_of(source: &str) -> Vec<Token> {
    let characters: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut position = 0;

    while position < characters.len() {
        let current = characters[position];
        let next = characters.get(position + 1).copied();

        if current.is_whitespace() {
            position += 1;
            continue;
        }
        if current == '/' && next == Some('/') {
            position = end_of_line_comment(&characters, position);
            continue;
        }
        if current == '/' && next == Some('*') {
            position = end_of_block_comment(&characters, position);
            continue;
        }
        if is_quote(current) {
            position = end_of_text(&characters, position);
            tokens.push(Token::Text);
            continue;
        }
        if current.is_ascii_digit() {
            position = end_of_number(&characters, position);
            tokens.push(Token::Number);
            continue;
        }
        if is_word_start(current) {
            let end = end_of_word(&characters, position);
            let word: String = characters[position..end].iter().collect();
            position = end;
            tokens.push(token_of_word(&word));
            continue;
        }

        tokens.push(Token::Symbol(current));
        position += 1;
    }

    tokens
}

/// 語 1 つ分をトークンにする。予約語でなければ識別子。
fn token_of_word(word: &str) -> Token {
    Keyword::new(word).map_or(Token::Identifier, Token::Keyword)
}

/// TypeScript の予約語。
///
/// 予約語であることを生成時に確かめるので、`Token::Keyword` が予約語でない語を
/// 持つ状態は作れない (rules/coding.md「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Keyword(&'static str);

impl Keyword {
    /// 予約語表に載っていれば予約語として作る。載っていなければ `None`。
    pub fn new(word: &str) -> Option<Self> {
        RESERVED_WORDS
            .iter()
            .copied()
            .find(|&it| it == word)
            .map(Self)
    }
}

/// 予約語表。
///
/// `number` / `string` のような組み込み型名は**入れない**。ここは構造だけを見る層で、
/// 型の違いは Stage 2 が見る。識別子として潰しておくほうが「名前の違いで類似度が
/// 落ちない」に沿う。
///
/// `type` / `get` / `from` のような文脈依存キーワードも入れない。`x.type` のような
/// ふつうの識別子として書かれる場所があり、**同じ語が場所によって別のトークンになる**。
const RESERVED_WORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// チャンク 1 つ分の、正規化トークンの集合。
///
/// 空では作れない。トークンが 1 つも取れなかったことを空の集合として通すと、
/// 後段が「共通するトークンが無い」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSet(HashSet<Token>);

impl TokenSet {
    /// ソースを正規化トークンの集合にする。
    ///
    /// トークンが 1 つも取れなかったときは作れないので `None` を返す。
    pub fn from_source(source: &str) -> Option<Self> {
        let tokens: HashSet<Token> = tokens_of(source).into_iter().collect();
        if tokens.is_empty() {
            return None;
        }
        Some(Self(tokens))
    }

    /// 2 つのトークン集合の Jaccard 係数（共通しているトークンが、合わせたうちの何割か）。
    ///
    /// これが Phase 0 の構造類似度。出現回数は見ないので、同じトークンが何度出ても
    /// 1 つとして数える。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        let shared = self.0.intersection(&other.0).count();
        let combined = self.0.union(&other.0).count();

        Similarity::from_shared_count(shared, combined)
    }
}

/// 行コメントの次の位置。改行そのものは空白として落ちる。
fn end_of_line_comment(characters: &[char], start: usize) -> usize {
    characters[start..]
        .iter()
        .position(|&character| character == '\n')
        .map_or(characters.len(), |offset| start + offset)
}

/// ブロックコメントの次の位置。閉じないまま末尾に達したら末尾。
fn end_of_block_comment(characters: &[char], start: usize) -> usize {
    let mut position = start + 2;

    while position + 1 < characters.len() {
        if characters[position] == '*' && characters[position + 1] == '/' {
            return position + 2;
        }
        position += 1;
    }

    characters.len()
}

/// 文字列・テンプレートリテラルの次の位置。閉じないまま末尾に達したら末尾。
///
/// `\` の次の 1 文字は読み飛ばす。`"a\"b"` を 2 文字目の `"` で閉じると、
/// そこから先の中身が**コードとして**トークンになる。
fn end_of_text(characters: &[char], start: usize) -> usize {
    let quote = characters[start];
    let mut position = start + 1;

    while position < characters.len() {
        if characters[position] == '\\' {
            position += 2;
            continue;
        }
        if characters[position] == quote {
            return position + 1;
        }
        position += 1;
    }

    characters.len()
}

/// 数値リテラルの次の位置。
///
/// 英数字と `_` を続けて読む。`0x1f` `1e5` `1_000` をまとめて 1 つのリテラルとして
/// 扱うため（値は捨てるので、切り方の細かさは類似度に効かない）。
///
/// `.` は**数字が続くときだけ**数値の一部にする。`1.0.toFixed(2)` の `.toFixed` まで
/// 飲み込むと、メンバーアクセスがトークン列から消えて構造が違って見える。
fn end_of_number(characters: &[char], start: usize) -> usize {
    let mut position = start;

    while let Some(&current) = characters.get(position) {
        let continues_the_number = if current == '.' {
            characters
                .get(position + 1)
                .is_some_and(char::is_ascii_digit)
        } else {
            current.is_ascii_alphanumeric() || current == '_'
        };

        if !continues_the_number {
            return position;
        }
        position += 1;
    }

    characters.len()
}

/// 識別子・予約語の次の位置。
fn end_of_word(characters: &[char], start: usize) -> usize {
    characters[start..]
        .iter()
        .position(|&character| !is_word_part(character))
        .map_or(characters.len(), |offset| start + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyword(word: &str) -> Keyword {
        Keyword::new(word).expect("テストが渡すのは予約語")
    }

    fn token_set(source: &str) -> TokenSet {
        TokenSet::from_source(source).expect("テストが渡すソースにはトークンがある")
    }

    #[test]
    fn test_tokens_of_an_identifier_drop_its_name() {
        assert_eq!(tokens_of("invoice"), vec![Token::Identifier]);
    }

    #[test]
    fn test_tokens_of_sources_that_differ_only_in_names_are_the_same() {
        // 名前の違いで類似度が落ちないようにするのが正規化の目的
        assert_eq!(
            tokens_of("const discounted = invoice.amount;"),
            tokens_of("const shortage = stock.quantity;")
        );
    }

    #[test]
    fn test_tokens_of_a_reserved_word_keep_the_word() {
        assert_eq!(tokens_of("return"), vec![Token::Keyword(keyword("return"))]);
    }

    #[test]
    fn test_tokens_of_a_number_literal_drop_its_value() {
        assert_eq!(tokens_of("0.1"), vec![Token::Number]);
    }

    #[test]
    fn test_tokens_of_a_property_access_on_a_number_keep_the_property_name() {
        // 数字が続かない `.` まで数値に飲み込むと、メンバーアクセスの構造が消える
        assert_eq!(
            tokens_of("1.0.toFixed(2)"),
            vec![
                Token::Number,
                Token::Symbol('.'),
                Token::Identifier,
                Token::Symbol('('),
                Token::Number,
                Token::Symbol(')')
            ]
        );
    }

    #[test]
    fn test_tokens_of_a_string_literal_drop_its_content() {
        // 中の語をトークンにすると、文字列を書き換えただけで構造が違って見える
        assert_eq!(tokens_of("\"const invoice\""), vec![Token::Text]);
    }

    #[test]
    fn test_tokens_of_a_string_literal_with_an_escaped_quote_end_at_the_real_quote() {
        assert_eq!(
            tokens_of("\"a\\\"b\" + 1"),
            vec![Token::Text, Token::Symbol('+'), Token::Number]
        );
    }

    #[test]
    fn test_tokens_of_a_template_literal_drop_its_content() {
        assert_eq!(tokens_of("`a ${b} c`"), vec![Token::Text]);
    }

    #[test]
    fn test_tokens_skip_a_line_comment_but_keep_the_next_line() {
        assert_eq!(tokens_of("// const invoice\n1"), vec![Token::Number]);
    }

    #[test]
    fn test_tokens_skip_a_block_comment_but_keep_what_follows() {
        assert_eq!(tokens_of("/* const invoice */ 1"), vec![Token::Number]);
    }

    #[test]
    fn test_tokens_of_an_operator_are_one_symbol_per_character() {
        assert_eq!(
            tokens_of("==="),
            vec![Token::Symbol('='), Token::Symbol('='), Token::Symbol('=')]
        );
    }

    #[test]
    fn test_tokens_of_a_source_with_only_whitespace_and_comments_are_none() {
        assert_eq!(tokens_of("  \n // 何も無い\n"), vec![]);
    }

    #[test]
    fn test_keyword_of_a_reserved_word_is_created() {
        assert!(Keyword::new("function").is_some(), "function は予約語");
    }

    #[test]
    fn test_keyword_of_an_ordinary_word_cannot_be_created() {
        assert_eq!(Keyword::new("invoice"), None);
    }

    #[test]
    fn test_token_set_of_a_source_without_tokens_cannot_be_created() {
        // 「トークンが取れなかった」を空の集合として通すと、後段が
        // 「似ていない」と「見ていない」を区別できない
        assert_eq!(TokenSet::from_source("  \n // 何も無い\n"), None);
    }

    #[test]
    fn test_jaccard_of_the_same_source_is_one() {
        let source = "return Math.max(shortage, 0);";

        assert_eq!(token_set(source).jaccard(&token_set(source)).value(), 1.0);
    }

    #[test]
    fn test_jaccard_of_sources_that_differ_only_in_names_is_one() {
        let discount = "const discounted = invoice.amount * (1 - RATE);";
        let reorder = "const shortage = stock.quantity * (1 - LIMIT);";

        assert_eq!(
            token_set(discount).jaccard(&token_set(reorder)).value(),
            1.0
        );
    }

    #[test]
    fn test_jaccard_of_sources_without_a_common_token_is_zero() {
        // 識別子だけの集合と数値だけの集合には共通のトークンが無い
        assert_eq!(token_set("invoice").jaccard(&token_set("42")).value(), 0.0);
    }

    #[test]
    fn test_jaccard_of_partially_overlapping_sources_is_the_shared_ratio() {
        // {Identifier, Number} と {Number, Symbol('+')} で、共通は Number の 1 つ、
        // 合わせて 3 つ
        let ratio = token_set("invoice 1").jaccard(&token_set("1 +")).value();

        assert!(
            (ratio - 1.0 / 3.0).abs() < f64::EPSILON,
            "共通 1 / 合併 3 になる: {ratio}"
        );
    }

    #[test]
    fn test_jaccard_of_structurally_different_sources_is_below_one() {
        // 名前を潰した結果すべてが似て見えるなら、このシグナルは何も言っていない
        let assignment = token_set("const discounted = invoice.amount;");
        let loop_over_items = token_set("for (const item of items) { total = total + 1; }");

        assert!(
            assignment.jaccard(&loop_over_items).value() < 1.0,
            "構造が違うペアは完全一致にならない"
        );
    }

    #[test]
    fn test_jaccard_does_not_count_how_many_times_a_token_appears() {
        // 集合で見るので出現回数は落ちる。Phase 0 の割り切り
        let once = token_set("invoice;");
        let three_times = token_set("invoice; invoice; invoice;");

        assert_eq!(once.jaccard(&three_times).value(), 1.0);
    }
}
