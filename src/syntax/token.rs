//! ソースを正規化したトークンの列にする。
//!
//! Phase 0 の素朴実装。tree-sitter は使わず、文字を前から見てトークンに切る
//! （`docs/dryguard-plan.md`「Phase 0: 貫通させる (LSPなし)」）。AST 正規化は Phase 1。
//!
//! テンプレートリテラルの `${}` の中と正規表現リテラルは、この切り方では読めない。
//! どちらも 1 つの [`Token::Text`] / 記号の並びとして潰れる。ここを詰めるより
//! tree-sitter へ移すほうが確実なので、Phase 0 では踏み込まない。

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

impl Token {
    /// ソースを正規化トークンの列にする。空白とコメントは落とす。
    pub fn collect_from(source: &str) -> Vec<Self> {
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
                tokens.push(Self::Text);
                continue;
            }
            if current.is_ascii_digit() {
                position = end_of_number(&characters, position);
                tokens.push(Self::Number);
                continue;
            }
            if is_word_start(current) {
                let end = end_of_word(&characters, position);
                let word: String = characters[position..end].iter().collect();
                position = end;
                tokens.push(Self::from_word(&word));
                continue;
            }

            tokens.push(Self::Symbol(current));
            position += 1;
        }

        tokens
    }

    /// 語 1 つ分をトークンにする。予約語でなければ識別子。
    fn from_word(word: &str) -> Self {
        Keyword::new(word).map_or(Self::Identifier, Self::Keyword)
    }
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

/// 引用符として文字列・テンプレートリテラルを開く文字か。
fn is_quote(character: char) -> bool {
    matches!(character, '\'' | '"' | '`')
}

/// 識別子の 1 文字目になれる文字か。
fn is_word_start(character: char) -> bool {
    character.is_alphabetic() || character == '_' || character == '$'
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
/// 英数字と `.` `_` を続けて読む。`0x1f` `1e5` `1_000` `0.1` をまとめて 1 つの
/// リテラルとして扱うため（値は捨てるので、切り方の細かさは類似度に効かない）。
fn end_of_number(characters: &[char], start: usize) -> usize {
    let is_part =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '.' | '_');

    characters[start..]
        .iter()
        .position(|&character| !is_part(character))
        .map_or(characters.len(), |offset| start + offset)
}

/// 識別子・予約語の次の位置。
fn end_of_word(characters: &[char], start: usize) -> usize {
    let is_part = |character: char| character.is_alphanumeric() || matches!(character, '_' | '$');

    characters[start..]
        .iter()
        .position(|&character| !is_part(character))
        .map_or(characters.len(), |offset| start + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyword(word: &str) -> Keyword {
        Keyword::new(word).expect("テストが渡すのは予約語")
    }

    #[test]
    fn test_tokens_of_an_identifier_drop_its_name() {
        assert_eq!(Token::collect_from("invoice"), vec![Token::Identifier]);
    }

    #[test]
    fn test_tokens_of_sources_that_differ_only_in_names_are_the_same() {
        // 名前の違いで類似度が落ちないようにするのが正規化の目的
        assert_eq!(
            Token::collect_from("const discounted = invoice.amount;"),
            Token::collect_from("const shortage = stock.quantity;")
        );
    }

    #[test]
    fn test_tokens_of_a_reserved_word_keep_the_word() {
        assert_eq!(
            Token::collect_from("return"),
            vec![Token::Keyword(keyword("return"))]
        );
    }

    #[test]
    fn test_tokens_of_a_number_literal_drop_its_value() {
        assert_eq!(Token::collect_from("0.1"), vec![Token::Number]);
    }

    #[test]
    fn test_tokens_of_a_string_literal_drop_its_content() {
        // 中の語をトークンにすると、文字列を書き換えただけで構造が違って見える
        assert_eq!(Token::collect_from("\"const invoice\""), vec![Token::Text]);
    }

    #[test]
    fn test_tokens_of_a_string_literal_with_an_escaped_quote_end_at_the_real_quote() {
        assert_eq!(
            Token::collect_from("\"a\\\"b\" + 1"),
            vec![Token::Text, Token::Symbol('+'), Token::Number]
        );
    }

    #[test]
    fn test_tokens_of_a_template_literal_drop_its_content() {
        assert_eq!(Token::collect_from("`a ${b} c`"), vec![Token::Text]);
    }

    #[test]
    fn test_tokens_skip_a_line_comment_but_keep_the_next_line() {
        assert_eq!(
            Token::collect_from("// const invoice\n1"),
            vec![Token::Number]
        );
    }

    #[test]
    fn test_tokens_skip_a_block_comment_but_keep_what_follows() {
        assert_eq!(
            Token::collect_from("/* const invoice */ 1"),
            vec![Token::Number]
        );
    }

    #[test]
    fn test_tokens_of_an_operator_are_one_symbol_per_character() {
        assert_eq!(
            Token::collect_from("==="),
            vec![Token::Symbol('='), Token::Symbol('='), Token::Symbol('=')]
        );
    }

    #[test]
    fn test_tokens_of_a_source_with_only_whitespace_and_comments_are_none() {
        assert_eq!(Token::collect_from("  \n // 何も無い\n"), vec![]);
    }

    #[test]
    fn test_keyword_of_a_reserved_word_is_created() {
        assert!(Keyword::new("function").is_some(), "function は予約語");
    }

    #[test]
    fn test_keyword_of_an_ordinary_word_cannot_be_created() {
        assert_eq!(Keyword::new("invoice"), None);
    }
}
