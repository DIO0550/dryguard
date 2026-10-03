//! 正規化で捨てた葉の綴りを、位置が揃う 2 つのチャンクのあいだで突き合わせる
//! （`rules/naming.md` の `leaf divergence`）。
//!
//! 正規化トークン列（`syntax::token`）は名前とリテラルの値を捨てるので、**同じ形のまま
//! 別の相手へ委譲する関数**（`capabilities.hover_provider` を見る関数と
//! `capabilities.references_provider` を見る関数）は構造類似度 1.0 になる。
//! ここは捨てた綴りを位置ごとに残しておき、**列が揃うペアに限って**同じ位置どうしを比べる。
//!
//! **比べるのは正規化トークン列が完全に一致するペアだけ。** 揃わなければ位置の対応が無く、
//! 「違いが無い」とも「違う」とも言えない（[`LeafComparison::UnalignedTokens`]）。構造類似度は
//! gram の多重集合で測るので、1.0 でも列が一致しないことがある。
//!
//! **チャンクが束縛した名前は、一貫した付け替えなら違いに数えない。** 引数や `let` の名前は
//! 書いた人の都合で、付け替えただけのペアは本物の複製（Type-2）。束縛は**宣言の形の
//! 許可リスト**で集めるので、一覧から漏れた形の名前は外を指す名前として綴りで比べる —
//! 付け替えただけのペアが「違う」側（共通化しない側）へ倒れ、偽の `EXTRACT-CANDIDATE` は
//! 出ない（`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。
//!
//! **チャンク自身のシグネチャに書かれた型は比べない。** そこは型シグネチャ（Stage 2）が見る。
//! 綴りで比べると、エイリアスと右辺のように**同じ型を指す別の綴り**が違いに見え、
//! 型シグネチャが単一化可能と答えたペアを綴りで覆す（`rules/naming.md`
//! 「`type reference` と `resolved type` を混ぜない」）。**本体の中の型名は比べる** —
//! `Vec::<u8>::new()` と `VecDeque::<u8>::new()` は Stage 2 が見ない位置で違う相手を使っている。
//! 外す範囲はシグネチャの構文の位置で決め、型名のノードの種別では決めない
//! （種別の一覧で外すと、一覧から漏れた式の位置の型名が比べない側＝偽の
//! `EXTRACT-CANDIDATE` の側へ倒れる）。

use std::collections::HashMap;
use std::fmt;

use tree_sitter::Node;

use crate::syntax::token::{Token, token_nodes_of};
use crate::syntax::tree::Grammar;

/// 束縛として扱ってよい葉の種別。**これ以外の葉は、束縛と同じ綴りでも綴りで比べる。**
///
/// `x.foo` の `foo`（`property_identifier` / `field_identifier`）は束縛と同じ綴りでも
/// フィールドを指す。束縛として付け替えを許すと、**読むフィールドが変わったペアを
/// 付け替えただけと読む**。
const BINDABLE_LEAF_KINDS: [&str; 2] = ["identifier", "type_identifier"];

/// 子に置かれた `identifier` が、束縛ではなく外の名前を指す親の種別（Rust）。
///
/// パス（`inner::hover` の `inner` / `hover`）は局所の名前を指せないので、チャンク自身と
/// 同じ綴りでも束縛にしない。構造体式の短縮形（`Request { hover }`）は局所の名前と同時に
/// **フィールド**を指すので、付け替えると埋めるフィールドが変わる。
const OUTSIDE_NAME_PARENT_KINDS: [&str; 3] = [
    "scoped_identifier",
    "scoped_type_identifier",
    "shorthand_field_initializer",
];

/// 正規化トークン 1 つ分の綴りと、それをどう突き合わせるか。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Spelling {
    /// 綴りを比べないトークン。子を持つ構文ノード（綴りは子の側にある）と、
    /// チャンク自身のシグネチャに書かれた型の中の葉。
    Unspelled,
    /// チャンクが束縛した名前。**一貫した付け替えは違いに数えない。**
    Bound(String),
    /// それ以外の葉（外を指す名前・リテラル・子を持たない構文ノード）。綴りで比べる。
    Fixed(String),
}

/// 正規化トークン 1 つと、その綴り。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpelledToken {
    token: Token,
    spelling: Spelling,
}

/// チャンク 1 つ分の、正規化トークンとその葉の綴りの並び。
///
/// **トークンと綴りを組で持つ。** 綴りだけの並びを別に持つと、位置が揃っているかを
/// 呼び出し側が確かめることになり、確かめずに比べる呼び出しが書けてしまう
/// （`rules/coding.md`「生成時に検証し、不正な値を存在させない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLeaves(Vec<SpelledToken>);

impl ChunkLeaves {
    /// チャンクのノードから作る。
    ///
    /// `node` はチャンクのノード、`own_name` はそのチャンクの名前のノード（無名なら `None`）、
    /// `source` はそれを含むファイル全体のソース、`grammar` は束縛の形を見分けるのに使う文法。
    pub fn from_node(
        node: Node<'_>,
        own_name: Option<Node<'_>>,
        source: &str,
        grammar: Grammar,
    ) -> Self {
        let bound = bound_names_of(node, own_name, source, grammar);
        let signature_types = signature_type_nodes_of(node, grammar);
        let context = SpellingContext {
            own_name,
            source,
            bound: &bound,
            signature_types: &signature_types,
        };

        let spelled = token_nodes_of(node)
            .into_iter()
            .map(|(token, token_node)| SpelledToken {
                token,
                spelling: context.spelling_of(token, token_node),
            })
            .collect();

        Self(spelled)
    }

    /// 2 つのチャンクの葉の綴りを、同じ位置どうしで突き合わせる。
    ///
    /// 正規化トークン列が一致しなければ [`LeafComparison::UnalignedTokens`]。
    pub fn compared_with(&self, other: &Self) -> LeafComparison {
        let aligned = self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(&other.0)
                .all(|(mine, theirs)| mine.token == theirs.token);
        if !aligned {
            return LeafComparison::UnalignedTokens;
        }

        let mut renames = Renames::default();
        let mut divergent: Vec<DivergentLeaf> = Vec::new();
        for (mine, theirs) in self.0.iter().zip(&other.0) {
            let diverged = match (&mine.spelling, &theirs.spelling) {
                (Spelling::Unspelled, Spelling::Unspelled) => None,
                (Spelling::Bound(spelling_a), Spelling::Bound(spelling_b)) => {
                    if renames.is_consistent(spelling_a, spelling_b) {
                        None
                    } else {
                        Some((spelling_a, spelling_b))
                    }
                }
                (Spelling::Fixed(spelling_a), Spelling::Fixed(spelling_b))
                    if spelling_a == spelling_b =>
                {
                    None
                }
                // 束縛と外の名前が向き合った組は、綴りが同じでも違いに数える
                // （片方では局所の名前、もう片方では外の名前を指している）
                (
                    Spelling::Bound(spelling_a) | Spelling::Fixed(spelling_a),
                    Spelling::Bound(spelling_b) | Spelling::Fixed(spelling_b),
                ) => Some((spelling_a, spelling_b)),
                // 同じ種別のトークンを、片方でだけ比べる。位置の対応が崩れている
                (Spelling::Unspelled, Spelling::Bound(_) | Spelling::Fixed(_))
                | (Spelling::Bound(_) | Spelling::Fixed(_), Spelling::Unspelled) => {
                    return LeafComparison::UnalignedTokens;
                }
            };

            let Some((spelling_a, spelling_b)) = diverged else {
                continue;
            };
            let leaf = DivergentLeaf::new(spelling_a.as_str(), spelling_b.as_str());
            if !divergent.contains(&leaf) {
                divergent.push(leaf);
            }
        }

        match DivergentLeaves::new(divergent) {
            Some(divergent) => LeafComparison::Diverged(divergent),
            None => LeafComparison::NoDivergence,
        }
    }
}

/// 2 つのチャンクの葉の綴りを突き合わせた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafComparison {
    /// 位置が揃い、違いは束縛した名前の一貫した付け替えだけ。
    NoDivergence,
    /// 位置が揃い、綴りの違う葉がある。
    Diverged(DivergentLeaves),
    /// 正規化トークン列が一致せず、位置の対応が無い。
    UnalignedTokens,
}

/// 同じ位置で綴りが違った葉の組 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergentLeaf {
    spelling_a: String,
    spelling_b: String,
}

impl DivergentLeaf {
    /// 2 つの綴りから作る。`spelling_a` は先に指定されたほうのチャンクの綴り。
    pub fn new(spelling_a: impl Into<String>, spelling_b: impl Into<String>) -> Self {
        Self {
            spelling_a: spelling_a.into(),
            spelling_b: spelling_b.into(),
        }
    }

    /// 先に指定されたほうのチャンクの綴り。
    pub fn spelling_a(&self) -> &str {
        &self.spelling_a
    }

    /// 後に指定されたほうのチャンクの綴り。
    pub fn spelling_b(&self) -> &str {
        &self.spelling_b
    }
}

impl fmt::Display for DivergentLeaf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ↔ {}", self.spelling_a, self.spelling_b)
    }
}

/// 綴りの違う葉の組。**1 組以上ある。** 最初に現れた順で、同じ組は 1 度だけ。
///
/// 空では作れない。空の組を「違いがある」側で運ぶと、違いが無いペアを共通化しない側へ
/// 傾けられてしまう（`rules/coding.md`「不正な状態を型で表現できなくする」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergentLeaves(Vec<DivergentLeaf>);

impl DivergentLeaves {
    /// 組の並びから作る。空なら `None`。
    pub fn new(divergent: Vec<DivergentLeaf>) -> Option<Self> {
        if divergent.is_empty() {
            return None;
        }
        Some(Self(divergent))
    }

    /// 組の並び。最初に現れた順。
    pub fn as_slice(&self) -> &[DivergentLeaf] {
        &self.0
    }
}

/// 束縛した名前の付け替えの対応。**両向きで 1 対 1** でなければ一貫していない。
///
/// 片向きだけ見ると、`(a, b) => a + b` と `(x, y) => x + x` が付け替えに見える
/// （`a → x` と `b → y` は両立するが、`x` の相手が `a` と `b` の 2 つになる）。
#[derive(Debug, Default)]
struct Renames<'a> {
    forward: HashMap<&'a str, &'a str>,
    backward: HashMap<&'a str, &'a str>,
}

impl<'a> Renames<'a> {
    /// `spelling_a` を `spelling_b` へ付け替えたと見て、それまでの対応と両立するか。
    /// **両立したときだけ**対応に加える。
    ///
    /// 両立しなかった組を登録すると、後から来る正しい付け替えまで違いとして並ぶ。
    fn is_consistent(&mut self, spelling_a: &'a str, spelling_b: &'a str) -> bool {
        let forward_fits = self
            .forward
            .get(spelling_a)
            .is_none_or(|known| *known == spelling_b);
        let backward_fits = self
            .backward
            .get(spelling_b)
            .is_none_or(|known| *known == spelling_a);
        if !(forward_fits && backward_fits) {
            return false;
        }

        self.forward.insert(spelling_a, spelling_b);
        self.backward.insert(spelling_b, spelling_a);
        true
    }
}

/// 綴りを決めるのに要る、チャンク 1 つ分の材料。
struct SpellingContext<'tree, 'context> {
    /// チャンク自身の名前のノード。無名なら `None`。
    own_name: Option<Node<'tree>>,
    /// チャンクを含むファイル全体のソース。
    source: &'context str,
    /// チャンクが束縛した名前と、その最初の束縛の始まり（[`bound_names_of`]）。
    bound: &'context HashMap<String, usize>,
    /// チャンク自身のシグネチャに書かれた型のノード（[`signature_type_nodes_of`]）。
    signature_types: &'context [Node<'tree>],
}

impl<'tree> SpellingContext<'tree, '_> {
    /// トークン 1 つの綴り。
    ///
    /// **チャンク自身の名前は、位置で束縛に数える**（`own_name` のノードそのもの）。比べる 2 つの
    /// チャンクは名前が違って当たり前で、数えるとどのペアにも違いが出る。種別では決めない —
    /// メソッドの名前は `property_identifier` で、**同じ綴りで委譲する `this.inner.traceable()` の
    /// `traceable`** と種別が同じになる。
    ///
    /// **束縛より前に現れた同じ綴りは、束縛にしない。** `use(x); const g = (x) => x` の 1 つ目の
    /// `x` は外の名前で、内側の引数の付け替えに乗せると外の名前の差し替えが付け替えに見える。
    fn spelling_of(&self, token: Token, node: Node<'tree>) -> Spelling {
        let is_leaf = match token {
            Token::Identifier | Token::Literal(_) => true,
            Token::Syntax(_) => node.child_count() == 0,
        };
        if !is_leaf || self.is_in_signature_type(node) {
            return Spelling::Unspelled;
        }

        let text = text_of(node, self.source).to_owned();
        if self.own_name == Some(node) {
            return Spelling::Bound(text);
        }

        let bindable = BINDABLE_LEAF_KINDS.contains(&node.kind())
            && !node
                .parent()
                .is_some_and(|parent| OUTSIDE_NAME_PARENT_KINDS.contains(&parent.kind()));
        let bound_by_then = self
            .bound
            .get(&text)
            .is_some_and(|first_binder| node.start_byte() >= *first_binder);
        if bindable && bound_by_then {
            return Spelling::Bound(text);
        }
        Spelling::Fixed(text)
    }

    /// そのノードが、チャンク自身のシグネチャに書かれた型の中にあるか。
    fn is_in_signature_type(&self, node: Node<'_>) -> bool {
        let range = node.byte_range();
        self.signature_types.iter().any(|signature_type| {
            let covering = signature_type.byte_range();
            covering.start <= range.start && range.end <= covering.end
        })
    }
}

/// チャンク自身のシグネチャに書かれた型のノード。引数の型・戻り値の型・型引数・`where` 節。
///
/// **型シグネチャ（Stage 2）が見ている範囲だけを外す。** 引数の既定値はシグネチャに書かれるが
/// 型ではない（式）ので外さない。一覧から漏れたシグネチャの形は比べる側へ倒れ、
/// 型の違いが綴りの違いとしても出るだけ（REVIEW 側）。
fn signature_type_nodes_of(node: Node<'_>, grammar: Grammar) -> Vec<Node<'_>> {
    let mut types: Vec<Node<'_>> = ["return_type", "type_parameters"]
        .into_iter()
        .filter_map(|field| node.child_by_field_name(field))
        .collect();

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "where_clause" {
            types.push(child);
        }
    }

    let Some(parameters) = node.child_by_field_name("parameters") else {
        return types;
    };
    let parameter_kinds: &[&str] = match grammar {
        Grammar::Rust => &["parameter"],
        Grammar::TypeScript | Grammar::Tsx => &["required_parameter", "optional_parameter"],
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter_kinds.contains(&parameter.kind()) {
            types.extend(parameter.child_by_field_name("type"));
        }
    }

    types
}

/// そのチャンクが束縛した名前の綴り。
///
/// `own_name` はチャンク自身の名前のノード。**これは入れない**（位置で数える。
/// [`SpellingContext::spelling_of`]）。
/// 入れると、チャンクと同じ綴りの外の関数へ委譲する形（`fn hover(x) { hover(x) }` が
/// 別の場所の `hover` を呼ぶ形）を付け替えと読む。代わりに再帰する 2 つは違いに数えられる
/// （倒れる向きは「違う」側）。
///
/// 綴りごとに、**最初の束縛の始まり**（バイト位置）を返す。
///
/// **スコープは辿らない。** 束縛より後に現れた同じ綴りは、束縛の外（兄弟のブロック）でも
/// 束縛として扱う。そこを閉じるにはスコープ解析を TypeScript と Rust の両方に書くことになる。
/// パスとフィールドは種別で外し、束縛より前の出現は位置で外してあるので、穴は
/// **束縛より後に現れた同じ綴りの、外の素の名前**に限られる（[`SpellingContext::spelling_of`]）。
fn bound_names_of(
    node: Node<'_>,
    own_name: Option<Node<'_>>,
    source: &str,
    grammar: Grammar,
) -> HashMap<String, usize> {
    let mut binders = Vec::new();

    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        match grammar {
            Grammar::Rust => rust_binders_of(current, source, &mut binders),
            Grammar::TypeScript | Grammar::Tsx => typescript_binders_of(current, &mut binders),
        }

        let mut cursor = current.walk();
        pending.extend(current.named_children(&mut cursor));
    }

    let mut first_binders: HashMap<String, usize> = HashMap::new();
    for binder in binders {
        if Some(binder) == own_name {
            continue;
        }
        let first = first_binders
            .entry(text_of(binder, source).to_owned())
            .or_insert(binder.start_byte());
        *first = (*first).min(binder.start_byte());
    }
    first_binders
}

/// TypeScript のノード 1 つが束縛する名前のノード。束縛しなければ何も足さない。
fn typescript_binders_of<'tree>(node: Node<'tree>, binders: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "function_expression"
        | "generator_function"
        | "class_declaration"
        | "class"
        | "type_parameter" => binders.extend(node.child_by_field_name("name")),
        "required_parameter" | "optional_parameter" => {
            typescript_pattern_binders_of(node.child_by_field_name("pattern"), binders);
        }
        "arrow_function" => binders.extend(node.child_by_field_name("parameter")),
        "variable_declarator" => {
            typescript_pattern_binders_of(node.child_by_field_name("name"), binders);
        }
        "for_in_statement" => {
            typescript_pattern_binders_of(node.child_by_field_name("left"), binders);
        }
        "catch_clause" => {
            typescript_pattern_binders_of(node.child_by_field_name("parameter"), binders);
        }
        _ => {}
    }
}

/// TypeScript のパターンが束縛する名前のノード。
///
/// 分割代入のキー（`{ amount: total }` の `amount`）と既定値は束縛しない。
fn typescript_pattern_binders_of<'tree>(
    pattern: Option<Node<'tree>>,
    binders: &mut Vec<Node<'tree>>,
) {
    let Some(pattern) = pattern else {
        return;
    };

    match pattern.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => binders.push(pattern),
        "object_pattern" | "array_pattern" | "rest_pattern" => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                typescript_pattern_binders_of(Some(child), binders);
            }
        }
        "pair_pattern" => {
            typescript_pattern_binders_of(pattern.child_by_field_name("value"), binders);
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            typescript_pattern_binders_of(pattern.child_by_field_name("left"), binders);
        }
        _ => {}
    }
}

/// Rust のノード 1 つが束縛する名前のノード。束縛しなければ何も足さない。
///
/// `source` はパターンの識別子が束縛か定数かを綴りで見分けるのに使う
/// （[`rust_pattern_binders_of`]）。
fn rust_binders_of<'tree>(node: Node<'tree>, source: &str, binders: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "function_item" | "type_parameter" | "const_parameter" => {
            binders.extend(node.child_by_field_name("name"));
        }
        "lifetime_parameter" => binders.extend(
            node.child_by_field_name("name")
                .and_then(|lifetime| lifetime.named_child(0)),
        ),
        "parameter" | "let_declaration" | "for_expression" | "let_condition" => {
            rust_pattern_binders_of(node.child_by_field_name("pattern"), source, binders);
        }
        "closure_parameters" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                // `|x: u32|` は `parameter` で、上の枝がそちらを訪ねたときに拾う
                if child.kind() != "parameter" {
                    rust_pattern_binders_of(Some(child), source, binders);
                }
            }
        }
        "match_pattern" => rust_pattern_binders_of(node.named_child(0), source, binders),
        _ => {}
    }
}

/// Rust のパターンが束縛する名前のノード。
///
/// **大文字で始まる識別子は束縛に数えない。** 構文木では `None` / 定数も束縛と同じ
/// `identifier` で、見分けられない。束縛に数えると、`None` と `Empty` の一貫した
/// 差し替えが付け替えに見える（倒れる向きは「違う」側）。
fn rust_pattern_binders_of<'tree>(
    pattern: Option<Node<'tree>>,
    source: &str,
    binders: &mut Vec<Node<'tree>>,
) {
    let Some(pattern) = pattern else {
        return;
    };

    match pattern.kind() {
        "identifier" => {
            let starts_uppercase = text_of(pattern, source)
                .chars()
                .next()
                .is_some_and(char::is_uppercase);
            if !starts_uppercase {
                binders.push(pattern);
            }
        }
        "mut_pattern" | "ref_pattern" | "reference_pattern" | "tuple_pattern" | "slice_pattern"
        | "or_pattern" | "captured_pattern" => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                rust_pattern_binders_of(Some(child), source, binders);
            }
        }
        "tuple_struct_pattern" => {
            let path = pattern.child_by_field_name("type");
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                if Some(child) != path {
                    rust_pattern_binders_of(Some(child), source, binders);
                }
            }
        }
        "struct_pattern" => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                if child.kind() != "field_pattern" {
                    continue;
                }
                match child.child_by_field_name("pattern") {
                    Some(field_pattern) => {
                        rust_pattern_binders_of(Some(field_pattern), source, binders);
                    }
                    // 短縮形（`Point { x, .. }`）。葉はフィールドも指すので、束縛として
                    // 付け替えを許す葉にはならない（`BINDABLE_LEAF_KINDS` に無い種別）
                    None => binders.extend(child.child_by_field_name("name")),
                }
            }
        }
        _ => {}
    }
}

/// ノードが覆うソースの綴り。
///
/// tree-sitter が返す範囲は、構文木を作ったソースの中に収まっている。
fn text_of<'source>(node: Node<'_>, source: &'source str) -> &'source str {
    source.get(node.byte_range()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::tree::SyntaxTree;

    /// ソースの中で最初に現れる、その種別のノードの葉の綴り。
    fn leaves_of(source: &str, grammar: Grammar, kind: &str) -> ChunkLeaves {
        let tree =
            SyntaxTree::from_source(source, grammar).expect("テストが渡すソースは木にできる");
        let node = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == kind)
            .expect("テストが渡すソースにその種別のノードがある");

        ChunkLeaves::from_node(node, node.child_by_field_name("name"), source, grammar)
    }

    fn typescript(source: &str) -> ChunkLeaves {
        leaves_of(source, Grammar::TypeScript, "function_declaration")
    }

    fn rust(source: &str) -> ChunkLeaves {
        leaves_of(source, Grammar::Rust, "function_item")
    }

    /// 違った葉の組を `a ↔ b` の綴りで並べる。違いが無い / 揃わないときは、その旨の 1 語。
    fn divergence_of(comparison: LeafComparison) -> Vec<String> {
        match comparison {
            LeafComparison::NoDivergence => vec!["identical".to_owned()],
            LeafComparison::UnalignedTokens => vec!["unaligned".to_owned()],
            LeafComparison::Diverged(divergent) => divergent
                .as_slice()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    fn typescript_divergence(source_a: &str, source_b: &str) -> Vec<String> {
        divergence_of(typescript(source_a).compared_with(&typescript(source_b)))
    }

    fn rust_divergence(source_a: &str, source_b: &str) -> Vec<String> {
        divergence_of(rust(source_a).compared_with(&rust(source_b)))
    }

    #[test]
    fn test_leaves_of_functions_that_only_rename_what_they_bind_do_not_diverge() {
        // Type-2 の本体。引数・`const`・関数自身の名前は書いた人の都合
        let divergence = typescript_divergence(
            "function monthly(value: Date) { const month = pad(value.getMonth()); return month; }",
            "function daily(input: Date) { const day = pad(input.getMonth()); return day; }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_functions_reading_different_members_diverge_at_the_member() {
        let divergence = typescript_divergence(
            "function monthly(value: Date) { return pad(value.getMonth()); }",
            "function daily(value: Date) { return pad(value.getDate()); }",
        );

        assert_eq!(divergence, vec!["getMonth ↔ getDate"]);
    }

    #[test]
    fn test_leaves_of_functions_returning_different_strings_diverge_at_the_literal() {
        let divergence = typescript_divergence(
            r#"function unrooted() { return "印が見つからない"; }"#,
            r#"function outside() { return "範囲から外れている"; }"#,
        );

        assert_eq!(
            divergence,
            vec![r#""印が見つからない" ↔ "範囲から外れている""#]
        );
    }

    #[test]
    fn test_leaves_of_functions_folding_two_bound_names_into_one_diverge() {
        // 片向きには `a → x` / `b → y` で両立して見える。逆向きで `x` の相手が 2 つになる
        let divergence = typescript_divergence(
            "function sum(a: number, b: number) { return a + b + a; }",
            "function twice(x: number, y: number) { return x + y + y; }",
        );

        assert_eq!(divergence, vec!["a ↔ y"]);
    }

    #[test]
    fn test_leaves_of_functions_with_different_token_sequences_are_unaligned() {
        let divergence = typescript_divergence(
            "function sum(a: number, b: number) { return a + b; }",
            "function difference(a: number, b: number) { return a - b; }",
        );

        assert_eq!(divergence, vec!["unaligned"]);
    }

    #[test]
    fn test_leaves_of_functions_differing_only_in_annotated_types_do_not_diverge() {
        // 型の違いは型シグネチャ（Stage 2）が見る。綴りで比べるとエイリアスと右辺が別物になる
        let divergence = typescript_divergence(
            "function total(amount: Amount): Amount { return amount; }",
            "function sum(amount: number): number { return amount; }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_methods_with_different_names_do_not_diverge_at_their_own_name() {
        let leaves_a = leaves_of(
            "class C { hover() { return this.inner.call(); } }",
            Grammar::TypeScript,
            "method_definition",
        );
        let leaves_b = leaves_of(
            "class C { references() { return this.inner.call(); } }",
            Grammar::TypeScript,
            "method_definition",
        );

        assert_eq!(
            divergence_of(leaves_a.compared_with(&leaves_b)),
            vec!["identical"]
        );
    }

    #[test]
    fn test_leaves_of_methods_delegating_under_their_own_names_diverge_at_the_delegate() {
        // 名前はメソッド自身の名前と同じ綴りの `property_identifier` だが、委譲先を指す
        let leaves_a = leaves_of(
            "class C { hover() { return this.inner.hover(); } }",
            Grammar::TypeScript,
            "method_definition",
        );
        let leaves_b = leaves_of(
            "class C { references() { return this.inner.references(); } }",
            Grammar::TypeScript,
            "method_definition",
        );

        assert_eq!(
            divergence_of(leaves_a.compared_with(&leaves_b)),
            vec!["hover ↔ references"]
        );
    }

    #[test]
    fn test_leaves_of_rust_functions_reading_different_fields_inside_a_macro_diverge() {
        // `matches!` の中は `token_tree` で、フィールドも `identifier` になる
        let divergence = rust_divergence(
            "fn provides_hover(capabilities: &Caps) -> bool {\n    matches!(capabilities.hover_provider, Some(Kind::Simple(true)))\n}",
            "fn provides_references(capabilities: &Caps) -> bool {\n    matches!(capabilities.references_provider, Some(Kind::Simple(true)))\n}",
        );

        assert_eq!(divergence, vec!["hover_provider ↔ references_provider"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_calling_paths_named_after_themselves_diverge() {
        // パスの要素はチャンク自身と同じ綴りでも外の名前
        let divergence = rust_divergence(
            "fn hover(x: u32) -> u32 { inner::hover(x) }",
            "fn references(x: u32) -> u32 { inner::references(x) }",
        );

        assert_eq!(divergence, vec!["hover ↔ references"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_building_different_variants_diverge() {
        // 構造体式の名前は型名のノードだが、型の位置ではなく作る値
        let divergence = rust_divergence(
            "fn imports(lean: Lean) -> Reason { Reason::Imports { lean } }",
            "fn distance(lean: Lean) -> Reason { Reason::Distance { lean } }",
        );

        assert_eq!(divergence, vec!["Imports ↔ Distance"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_reading_different_primitive_constants_diverge() {
        let divergence = rust_divergence(
            "fn narrow() -> u64 { u32::MAX as u64 }",
            "fn wide() -> u64 { u64::MAX as u64 }",
        );

        assert_eq!(divergence, vec!["u32 ↔ u64"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_that_only_rename_bindings_and_lifetimes_do_not_diverge() {
        let divergence = rust_divergence(
            "fn first<'a, T>(items: &'a [T]) -> Option<&'a T> { let head = items.first(); match head { Some(item) => Some(item), None => None } }",
            "fn front<'b, U>(values: &'b [U]) -> Option<&'b U> { let lead = values.first(); match lead { Some(value) => Some(value), None => None } }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_matching_different_constants_diverge() {
        // 大文字で始まるパターンの識別子は定数かもしれないので、束縛として付け替えを許さない
        let divergence = rust_divergence(
            "fn is_low(level: u8) -> bool { match level { LOW => true, _ => false } }",
            "fn is_high(level: u8) -> bool { match level { HIGH => true, _ => false } }",
        );

        assert_eq!(divergence, vec!["LOW ↔ HIGH"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_using_different_types_in_their_bodies_diverge() {
        // 本体の中の型は型シグネチャが見ない。同じ形のまま違う型へ委譲している
        let divergence = rust_divergence(
            "fn ordered() -> usize { let items = Vec::<u8>::new(); items.len() }",
            "fn queued() -> usize { let items = VecDeque::<u8>::new(); items.len() }",
        );

        assert_eq!(divergence, vec!["Vec ↔ VecDeque"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_casting_to_different_types_diverge() {
        let divergence = rust_divergence(
            "fn narrow(x: u32) -> u32 { (x as u8) as u32 }",
            "fn wide(x: u32) -> u32 { (x as u16) as u32 }",
        );

        assert_eq!(divergence, vec!["u8 ↔ u16"]);
    }

    #[test]
    fn test_leaves_of_typescript_functions_building_different_generic_types_diverge() {
        let divergence = typescript_divergence(
            "function counts() { return new Map<string, number>(); }",
            "function labels() { return new Map<string, string>(); }",
        );

        assert_eq!(divergence, vec!["number ↔ string"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_naming_signature_types_by_different_paths_do_not_diverge() {
        // シグネチャの型は型シグネチャ（Stage 2）が見る。パスの要素も綴りで覆さない
        let divergence = rust_divergence(
            "fn billed(amount: crate::billing::Amount) -> u32 { amount.cents() }",
            "fn stocked(amount: crate::stock::Amount) -> u32 { amount.cents() }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_functions_renaming_an_outer_name_before_a_binder_of_the_same_spelling_diverge()
     {
        // 1 つ目の `x` / `z` は外の名前。後の引数の付け替えに乗せない
        let divergence = typescript_divergence(
            "function run() { use(x); const g = (x) => x; return g; }",
            "function walk() { use(z); const g = (z) => z; return g; }",
        );

        assert_eq!(divergence, vec!["x ↔ z"]);
    }

    #[test]
    fn test_leaves_of_functions_do_not_count_a_later_consistent_rename_after_a_mismatch() {
        // `b ↔ x` は両立しない（`x` の相手は `a`）。それを対応に登録すると、後の
        // `b ↔ y`（どちらもまだ相手が無い）まで違いに並ぶ
        let divergence = typescript_divergence(
            "function f() { const a = 1; const b = 2; const b = 3; }",
            "function g() { const x = 1; const x = 2; const y = 3; }",
        );

        assert_eq!(divergence, vec!["b ↔ x"]);
    }

    #[test]
    fn test_leaves_of_typescript_functions_renaming_every_kind_of_binder_do_not_diverge() {
        // for-in・catch・単一引数のアロー・分割代入（キー付き・既定値・残り）
        let divergence = typescript_divergence(
            "function f(o: object) { for (const k in o) { use(k); } try { use(o); } catch (e) { use(e); } const h = v => v; const { key: a, b = 1, ...rest } = o; return [h, a, b, rest]; }",
            "function g(p: object) { for (const n in p) { use(n); } try { use(p); } catch (err) { use(err); } const m = w => w; const { key: x, b = 1, ...others } = p; return [m, x, b, others]; }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_renaming_every_kind_of_binder_do_not_diverge() {
        // クロージャ引数（注釈あり・なし）・for・if let
        let divergence = rust_divergence(
            "fn f(items: &[Point]) -> u32 { let add = |a: u32, b| a + b; let mut total = 0; for item in items { if let Some(head) = item.first() { total = add(total, head.x); } } total }",
            "fn g(values: &[Point]) -> u32 { let sum = |c: u32, d| c + d; let mut acc = 0; for value in values { if let Some(lead) = value.first() { acc = sum(acc, lead.x); } } acc }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }
}
