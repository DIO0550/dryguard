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
/// 付け替えただけと読む**。Swift は名前もプロパティも `simple_identifier` なので、
/// プロパティの側は並びで外す（[`is_outside_name`]）。
const BINDABLE_LEAF_KINDS: [&str; 3] = ["identifier", "type_identifier", "simple_identifier"];

/// 子に置かれた名前の葉が、束縛ではなく外の名前を指す親の種別。
///
/// パス（`inner::hover` の `inner` / `hover`）は局所の名前を指せないので、チャンク自身と
/// 同じ綴りでも束縛にしない。構造体式の短縮形（`Request { hover }`）は局所の名前と同時に
/// **フィールド**を指すので、付け替えると埋めるフィールドが変わる。Swift の呼び出しの
/// `g(to: a)` の `to`（`value_argument_label`）はラベルで、付け替えると呼ぶ API が変わる。
const OUTSIDE_NAME_PARENT_KINDS: [&str; 4] = [
    "scoped_identifier",
    "scoped_type_identifier",
    "shorthand_field_initializer",
    "value_argument_label",
];

/// 子に置かれた `identifier` が、束縛ではなく外の名前を指す位置（Java）。親の種別と、その子が
/// 置かれるフィールドの組。
///
/// Java はメソッドと注釈の要素の名前も `identifier` なので、[`BINDABLE_LEAF_KINDS`] の種別では
/// 止まらない。局所の `size` を束縛すると、修飾の無い呼び出し `size()` まで束縛になり、
/// **呼ぶメソッドが変わったペアを付け替えただけと読む**。`this.rate` / `data.size()` のように
/// `.` の後ろに来る名前は、Swift のメンバーと同じ並びの判定（[`is_outside_name`]）が外す。
/// 親の種別だけで引かないのは、同じ親の `object`（`list.add(x)` の `list`）が局所の名前を指せるため。
const OUTSIDE_NAME_FIELDS: [(&str, &str); 2] =
    [("method_invocation", "name"), ("element_value_pair", "key")];

/// 引数のラベルを載せるフィールド（Swift の `parameter` / `lambda_parameter`）。
///
/// ラベルは呼び出しの API で、内部名と同じ親の下に並ぶので種別でも親でも分けられない。
const EXTERNAL_NAME_FIELD: &str = "external_name";

/// その名前の葉が、束縛ではなく外の名前を指すか。
///
/// 親の種別（[`OUTSIDE_NAME_PARENT_KINDS`]）と引数のラベルのほかに、Swift の 2 つの形を
/// 並びで見る。`.` の直後の名前はメンバーで、プロパティ（`self.rate` / key path の `\.rate`）と
/// 暗黙のメンバー（`.rate` / `case .low`。列挙子や静的メンバー）がこれに当たる。
/// タプル式・タプル型・パターンのラベル（`(x: 1)` / `(x: Int)` / `case .pt(low: let a)`）は
/// `:` の直前の名前で、要素の値（`(x, y)` の `x`）と同じ親の下に並ぶので種別では分けられない。
///
/// `.` の直後の名前は Java のフィールド・メソッド（`this.rate` / `data.size()`）にも当たる。
/// Java は修飾の無いメソッド名と注釈の要素名（[`OUTSIDE_NAME_FIELDS`]）・メソッド参照の
/// `::` より後ろも位置で外す。
///
/// **漏れは束縛の側へ倒れる**（拒否リスト）。ただし束縛に効くのは、同じ綴りが先に
/// 束縛されたときだけ（[`SpellingContext::spelling_of`]）。
fn is_outside_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let names_member = node
        .prev_sibling()
        .is_some_and(|previous| !previous.is_named() && previous.kind() == ".");
    let labels_element = LABELLED_PARENT_KINDS.contains(&parent.kind())
        && node
            .next_sibling()
            .is_some_and(|next| !next.is_named() && next.kind() == ":");

    let at_outside_field = OUTSIDE_NAME_FIELDS.iter().any(|(kind, field)| {
        parent.kind() == *kind && parent.child_by_field_name(field) == Some(node)
    });
    let after_method_reference = parent.kind() == "method_reference" && follows_double_colon(node);

    OUTSIDE_NAME_PARENT_KINDS.contains(&parent.kind())
        || parent.child_by_field_name(EXTERNAL_NAME_FIELD) == Some(node)
        || names_member
        || labels_element
        || at_outside_field
        || after_method_reference
}

/// 前の兄弟のどれかが `::` か（Java のメソッド参照）。
///
/// メソッド参照（`Foo::bar`）はフィールドを持たないので、`::` より左の局所の名前
/// （`rate::apply` の `rate`）を外さないよう位置で見分ける。直前の兄弟では見ない —
/// `data::<T>size` では `::` と名前の間に型引数が入る。
fn follows_double_colon(node: Node<'_>) -> bool {
    let mut previous = node.prev_sibling();
    while let Some(sibling) = previous {
        if sibling.kind() == "::" {
            return true;
        }
        previous = sibling.prev_sibling();
    }
    false
}

/// `:` の直前の名前がラベルになる親の種別（Swift）。
///
/// 三項演算子の `a ? b : c` の `b` も `:` の直前に来るので、親を限らずに並びだけで見ない。
const LABELLED_PARENT_KINDS: [&str; 3] = ["tuple_expression", "tuple_type_item", "pattern"];

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

        let bindable = BINDABLE_LEAF_KINDS.contains(&node.kind()) && !is_outside_name(node);
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

/// チャンク自身のシグネチャに書かれた型のノード。引数の型・戻り値の型・型引数・`where` 節
/// （Java は `throws` と、名前の後ろに書いた配列の次元も）。
///
/// **型シグネチャ（Stage 2）が見る範囲だけを外す。** 引数の既定値はシグネチャに書かれるが
/// 型ではない（式）ので外さない。一覧から漏れたシグネチャの形は比べる側へ倒れ、
/// 型の違いが綴りの違いとしても出るだけ（REVIEW 側）。Java は Stage 2 の綴りを読む側が
/// まだ無いが、TypeScript・Rust でサーバに尋ねられなかったときと同じく、型の違いは
/// 葉の綴りではなく型シグネチャの答えとして扱う。
fn signature_type_nodes_of(node: Node<'_>, grammar: Grammar) -> Vec<Node<'_>> {
    let mut types: Vec<Node<'_>> = ["return_type", "type_parameters"]
        .into_iter()
        .filter_map(|field| node.child_by_field_name(field))
        .collect();
    if grammar == Grammar::Java {
        // 戻り値は `return_type` ではなく `type`。`int f()[]` の次元も戻り値の型
        types.extend(
            ["type", "dimensions"]
                .into_iter()
                .filter_map(|field| node.child_by_field_name(field)),
        );
    }

    // Swift の `type_parameters` はフィールドではなく子として並ぶ
    let clause_kinds: &[&str] = match grammar {
        Grammar::Rust | Grammar::TypeScript | Grammar::Tsx => &["where_clause"],
        // `throws` は例外の型で、hover の綴りにも出る（Swift の `throws` は型ではない）
        Grammar::Java => &["throws"],
        Grammar::Swift => &["type_parameters", "type_constraints"],
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if clause_kinds.contains(&child.kind()) {
            types.push(child);
        }
    }

    // Swift は引数リストの包みが無く、引数が宣言の直接の子として並ぶ
    let parameters = match grammar {
        Grammar::Rust | Grammar::TypeScript | Grammar::Tsx | Grammar::Java => {
            node.child_by_field_name("parameters")
        }
        Grammar::Swift => Some(node),
    };
    let Some(parameters) = parameters else {
        return types;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        types.extend(parameter_type_nodes_of(parameter, grammar));
    }

    types
}

/// 引数 1 つのうち、型を書いた部分のノード。型を持たない引数なら空。
fn parameter_type_nodes_of(parameter: Node<'_>, grammar: Grammar) -> Vec<Node<'_>> {
    let typed_fields = |fields: &[&str]| -> Vec<Node<'_>> {
        fields
            .iter()
            .filter_map(|field| parameter.child_by_field_name(field))
            .collect()
    };
    // 一覧に無い引数の種別は外さないので、引数の型の違いも葉の綴りの違いに数える（違う側へ倒れる）
    match grammar {
        Grammar::Rust => match parameter.kind() {
            "parameter" => typed_fields(&["type"]),
            _ => Vec::new(),
        },
        Grammar::TypeScript | Grammar::Tsx => match parameter.kind() {
            "required_parameter" | "optional_parameter" => typed_fields(&["type"]),
            _ => Vec::new(),
        },
        Grammar::Java => match parameter.kind() {
            // `int a[]` の次元も引数の型
            "formal_parameter" => typed_fields(&["type", "dimensions"]),
            // `T... xs` と `Foo this`。型はフィールドを持たない子として、名前や注釈と並ぶ
            "spread_parameter" | "receiver_parameter" => {
                let mut cursor = parameter.walk();
                parameter
                    .named_children(&mut cursor)
                    .filter(|child| !JAVA_NON_TYPE_PARAMETER_CHILD_KINDS.contains(&child.kind()))
                    .collect()
            }
            _ => Vec::new(),
        },
        Grammar::Swift => match parameter.kind() {
            "parameter" => typed_fields(&["type"]),
            _ => Vec::new(),
        },
    }
}

/// Java の引数のうち、フィールドを持たない子で型ではないものの種別。
///
/// 注釈は型ではなく値を渡しうるので比べる側に残す。`identifier` は `Outer.this` の外側の
/// クラス名。
const JAVA_NON_TYPE_PARAMETER_CHILD_KINDS: [&str; 6] = [
    "modifiers",
    "annotation",
    "marker_annotation",
    "variable_declarator",
    "identifier",
    "this",
];

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
/// 束縛として扱う。そこを閉じるにはスコープ解析を TypeScript・Rust・Java・Swift のそれぞれに
/// 書くことになる。
/// パス・フィールド・ラベルは種別と並び・位置で外し（[`is_outside_name`]）、束縛より前の出現は
/// 位置で外してあるので、穴は
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
            Grammar::Java => java_binders_of(current, &mut binders),
            Grammar::Swift => swift_binders_of(current, &mut binders),
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

/// Java のノード 1 つが束縛する名前のノード。束縛しなければ何も足さない。
///
/// **メソッド・コンストラクタの名前は集めない。** Rust の入れ子の `function_item` と違い、
/// 匿名クラス・ローカルクラスの中のメソッドはクラスのメンバーで、`run` を `go` に替えると
/// 上書きしていたものが新しいメソッドになる。チャンク自身の名前は位置で数える
/// （[`SpellingContext::spelling_of`]）。
///
/// 束縛は宣言の `name` の位置で決まり、Rust のパターンのように定数と同じ形にならないので、
/// 大文字で始まる名前も束縛に数える（型変数・ローカルクラス）。`_`（`underscore_pattern`）は
/// 名前ではないので足さない。`try (r)` のように既存の変数を書いたリソースは `name` を持たない。
fn java_binders_of<'tree>(node: Node<'tree>, binders: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "class_declaration"
        | "formal_parameter"
        | "catch_formal_parameter"
        | "enhanced_for_statement"
        | "variable_declarator"
        | "resource"
        | "instanceof_expression" => {
            java_name_binders_of(node.child_by_field_name("name"), binders);
        }
        "lambda_expression" => {
            // `(int a) -> a` は `formal_parameters` で、`formal_parameter` の枝が拾う
            let parameters = node.child_by_field_name("parameters");
            match parameters.map(|parameters| parameters.kind()) {
                Some("identifier") => java_name_binders_of(parameters, binders),
                Some("inferred_parameters") => java_children_binders_of(parameters, binders),
                _ => {}
            }
        }
        // 型変数の名前と、パターンの末尾の名前（`case Foo f` / `Point(int x, var y)` の `x`）。
        // `record_pattern` の先頭の `identifier` は record の型名なので見ない
        "type_parameter" | "type_pattern" | "record_pattern_component" => {
            let mut cursor = node.walk();
            let last = node
                .named_children(&mut cursor)
                .filter(|child| matches!(child.kind(), "identifier" | "type_identifier"))
                .last();
            java_name_binders_of(last, binders);
        }
        _ => {}
    }
}

/// `name` に置かれた Java の名前を束縛に足す。`_` のように名前でないものは足さない。
fn java_name_binders_of<'tree>(name: Option<Node<'tree>>, binders: &mut Vec<Node<'tree>>) {
    binders.extend(name.filter(|name| BINDABLE_LEAF_KINDS.contains(&name.kind())));
}

/// 子に並んだ Java の名前（`(a, b) -> a` の `a` / `b`）を束縛に足す。
fn java_children_binders_of<'tree>(node: Option<Node<'tree>>, binders: &mut Vec<Node<'tree>>) {
    let Some(node) = node else {
        return;
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        java_name_binders_of(Some(child), binders);
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

/// Swift のノード 1 つが束縛する名前のノード。束縛しなければ何も足さない。
///
/// **引数のラベル（`external_name`）は入れない。** ラベルは呼び出しの API で、付け替えると
/// 呼び出し側が変わる。比べる側（[`is_outside_name`]）もフィールドで外すので、
/// ラベルの違いは葉の違いに数える。
///
/// **ラベルを書かない関数の引数（`func f(x: Int)`）は束縛に入れない。** 内部名がラベルを
/// 兼ねるので、付け替えるとラベルも変わる。束縛に入れると、ラベルの違うペアが付け替えに
/// 見える（倒れる向きは「違う」側）。クロージャの引数は呼び出しのラベルを持たないので入れる。
///
/// `while let` / `repeat` / キャプチャリスト（`[x = y]`）の束縛は集めない（「違う」側へ倒れる）。
fn swift_binders_of<'tree>(node: Node<'tree>, binders: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "function_declaration" => binders.extend(node.child_by_field_name("name")),
        "parameter" if node.child_by_field_name(EXTERNAL_NAME_FIELD).is_some() => {
            binders.extend(node.child_by_field_name("name"));
        }
        "lambda_parameter" => binders.extend(
            node.child_by_field_name("name")
                .filter(|name| name.kind() == "simple_identifier"),
        ),
        "type_parameter" => {
            let mut cursor = node.walk();
            binders.extend(
                node.named_children(&mut cursor)
                    .find(|child| child.kind() == "type_identifier"),
            );
        }
        "property_declaration" => {
            let mut cursor = node.walk();
            for pattern in node.named_children(&mut cursor) {
                if pattern.kind() == SWIFT_PATTERN_KIND {
                    swift_pattern_binders_of(pattern, true, binders);
                }
            }
        }
        "for_statement" => {
            if let Some(item) = node.child_by_field_name("item") {
                // `for case .some(x) in` の `x` は式のパターン。`case` が無ければ束縛する
                let binding = !has_child_of_kind(item, "case");
                swift_pattern_binders_of(item, binding, binders);
            }
        }
        "catch_block" => {
            if let Some(error) = node.child_by_field_name("error") {
                swift_pattern_binders_of(error, false, binders);
            }
        }
        "switch_pattern" => {
            let mut cursor = node.walk();
            for pattern in node.named_children(&mut cursor) {
                swift_pattern_binders_of(pattern, false, binders);
            }
        }
        "if_statement" | "guard_statement" => swift_condition_binders_of(node, binders),
        _ => {}
    }
}

/// Swift のパターンのノードの種別。
const SWIFT_PATTERN_KIND: &str = "pattern";

/// Swift の `let` / `var` の印のノードの種別。
const SWIFT_VALUE_BINDING_KIND: &str = "value_binding_pattern";

/// `if` / `guard` の条件が束縛する名前のノード。
///
/// `if let a = b` の `a` は文の `bound_identifier` に載る。**`let` の直後で `=` が続くものだけを
/// 入れる** — `=` が続かない短縮形（`if let rate {`）は同じ名前で外の値を読み、`let` の無い
/// `if case low = v` の `low` は外の値と比べる式のパターンで、どちらも付け替えると読む相手が
/// 変わる。`if case let .some(r) = opt` は
/// `let`・列挙子・`pattern(r)` が**文の子として平らに並ぶ**。`let` から次の `,` までを
/// 1 つの条件として、その中のパターンだけを束縛の文脈で読む。
///
/// **Why not（`let` の次の兄弟を束縛にする）**: 平らな形では次の兄弟が列挙子の `some` で、
/// 付け替えを許すと別の列挙子を読む 2 つが同じに見える。
fn swift_condition_binders_of<'tree>(node: Node<'tree>, binders: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    binders.extend(
        node.children_by_field_name("bound_identifier", &mut cursor)
            .filter(|name| {
                let follows_let = name
                    .prev_sibling()
                    .is_some_and(|previous| previous.kind() == SWIFT_VALUE_BINDING_KIND);
                let assigned = name
                    .next_sibling()
                    .is_some_and(|next| !next.is_named() && next.kind() == "=");
                follows_let && assigned
            }),
    );

    let mut binding = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            SWIFT_VALUE_BINDING_KIND => binding = true,
            "," => binding = false,
            SWIFT_PATTERN_KIND => swift_pattern_binders_of(child, binding, binders),
            _ => {}
        }
    }
}

/// Swift のパターンが束縛する名前のノード。
///
/// `binding` はそのパターンが `let` / `var` の文脈にあるか。名前は `bound_identifier` に載るか、
/// タプルの要素（`let (a, b)` の `a`）のようにフィールドを持たず、子が名前 1 つだけの
/// パターンになる。**`let` の文脈の外では、どちらも束縛にしない** — `case .none:` /
/// `if case .some(c) = d` / `catch low` の名前は式のパターンで、外の名前を指す
/// （`catch low` の `low` も `bound_identifier` に載る）。
/// 列挙子（`case let .some(s)` の `some`）は名前 1 つのパターンにならないので入らない。
fn swift_pattern_binders_of<'tree>(
    pattern: Node<'tree>,
    binding: bool,
    binders: &mut Vec<Node<'tree>>,
) {
    let binding = binding || has_child_of_kind(pattern, SWIFT_VALUE_BINDING_KIND);
    if binding {
        let mut cursor = pattern.walk();
        binders.extend(pattern.children_by_field_name("bound_identifier", &mut cursor));
        let sole_name = (pattern.child_count() == 1)
            .then(|| pattern.child(0))
            .flatten()
            .filter(|child| child.kind() == "simple_identifier");
        binders.extend(sole_name);
    }

    let mut cursor = pattern.walk();
    for child in pattern.named_children(&mut cursor) {
        if child.kind() == SWIFT_PATTERN_KIND {
            swift_pattern_binders_of(child, binding, binders);
        }
    }
}

/// そのノードが、その種別の子（字句を含む）を持つか。
fn has_child_of_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
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

    fn java(source: &str) -> ChunkLeaves {
        leaves_of(source, Grammar::Java, "method_declaration")
    }

    fn swift(source: &str) -> ChunkLeaves {
        leaves_of(source, Grammar::Swift, "function_declaration")
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

    fn java_divergence(source_a: &str, source_b: &str) -> Vec<String> {
        divergence_of(java(source_a).compared_with(&java(source_b)))
    }

    fn swift_divergence(source_a: &str, source_b: &str) -> Vec<String> {
        divergence_of(swift(source_a).compared_with(&swift(source_b)))
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
    fn test_leaves_of_rust_functions_differing_in_comments_diverge_only_at_the_code() {
        // doc コメントの本文は葉の綴りを持ち、片側だけの `//` は列をずらす。どちらも構造ではない。
        // 対照に呼ぶ相手の名前を変え、比べる対象が残っていることを見る
        let divergence = rust_divergence(
            "fn f() { /// a\n let x = pad(1); }",
            "fn f() { /// b\n // c\n let x = trim(1); }",
        );

        assert_eq!(divergence, vec!["pad ↔ trim"]);
    }

    #[test]
    fn test_leaves_of_java_methods_differing_in_comments_diverge_only_at_the_code() {
        // Java のコメントは本文の綴りを持つ葉。対照は上の Rust のテストと同じく読む相手の名前
        let divergence = java_divergence(
            "class C { void f() { /* a */ int t = base; } }",
            "class C { void f() { /* b */ // c\n int t = other; } }",
        );

        assert_eq!(divergence, vec!["base ↔ other"]);
    }

    #[test]
    fn test_leaves_of_java_methods_differing_only_in_a_primitive_type_diverge_at_the_type() {
        // プリミティブ型の違いが `unaligned` に消えず、葉の違いとして出る
        let divergence = java_divergence(
            "class C { void f() { int total = base; } }",
            "class C { void f() { long total = base; } }",
        );

        assert_eq!(divergence, vec!["int ↔ long"]);
    }

    #[test]
    fn test_leaves_of_java_methods_differing_only_in_signature_types_and_throws_do_not_diverge() {
        // 引数・可変長引数・戻り値・`throws` の型は型シグネチャ（Stage 2）が見る
        let divergence = java_divergence(
            "class C { <T> int sum(int a, List<T> xs, Amount... rest) throws IOException { return a + 1; } }",
            "class C { <T> long sum(long a, Set<T> xs, Money... rest) throws SQLException { return a + 1; } }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_java_methods_building_different_types_in_their_bodies_diverge() {
        // 対照: シグネチャの型を外しても、本体の中の型名は比べる
        let divergence = java_divergence(
            "class C { List<String> names(List<String> source) { return new ArrayList<>(source); } }",
            "class C { List<String> names(List<String> source) { return new LinkedList<>(source); } }",
        );

        assert_eq!(divergence, vec!["ArrayList ↔ LinkedList"]);
    }

    #[test]
    fn test_leaves_of_java_methods_that_only_rename_what_they_bind_do_not_diverge() {
        // 引数・ローカル・拡張 for・lambda（引数 1 つ・推論・型付き）・型変数
        let divergence = java_divergence(
            "class C { <T> int total(List<T> items) { int sum = 0; for (T item : items) { sum += weight(item); } Function<T, T> same = x -> x; BiFunction<T, T, T> left = (a, b) -> a; BinaryOperator<Integer> add = (int p, int q) -> p + q; return add.apply(sum, 1); } }",
            "class C { <U> int count(List<U> values) { int acc = 0; for (U value : values) { acc += weight(value); } Function<U, U> keep = y -> y; BiFunction<U, U, U> first = (c, d) -> c; BinaryOperator<Integer> plus = (int m, int n) -> m + n; return plus.apply(acc, 1); } }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_java_methods_that_only_rename_pattern_and_resource_binders_do_not_diverge() {
        // catch・try のリソース・instanceof・switch の型パターン・record パターン・ローカルクラス
        let divergence = java_divergence(
            "class C { int f(Object o) { class Box {} try (var in = open()) { if (o instanceof Foo foo) { use(foo, in, new Box()); } } catch (IOException | RuntimeException e) { use(e); } if (o instanceof Point(int x, var y)) { return x + y; } return switch (o) { case Bar bar -> bar.size(); default -> 0; }; } }",
            "class C { int g(Object v) { class Cell {} try (var res = open()) { if (v instanceof Foo hit) { use(hit, res, new Cell()); } } catch (IOException | RuntimeException err) { use(err); } if (v instanceof Point(int a, var b)) { return a + b; } return switch (v) { case Bar found -> found.size(); default -> 0; }; } }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_java_methods_reading_fields_named_like_their_parameters_diverge_at_the_field()
    {
        // 引数 `rate` / `tax` の付け替えは許すが、同じ綴りのフィールドは別の相手を指す
        let divergence = java_divergence(
            "class C { double apply(double rate) { return this.rate * rate; } }",
            "class C { double apply(double tax) { return this.tax * tax; } }",
        );

        assert_eq!(divergence, vec!["rate ↔ tax"]);
    }

    #[test]
    fn test_leaves_of_java_methods_calling_methods_named_like_their_locals_diverge_at_the_call() {
        // 修飾の無い呼び出しのメソッドの名前も、同じ綴りの束縛とは別の相手
        let divergence = java_divergence(
            "class C { int f() { int size = 0; return size + size(); } }",
            "class C { int f() { int count = 0; return count + count(); } }",
        );

        assert_eq!(divergence, vec!["size ↔ count"]);
    }

    #[test]
    fn test_leaves_of_java_methods_referring_to_methods_named_like_their_locals_diverge() {
        // メソッド参照の `::` の後ろも別の相手。左の `data` は引数なので付け替えを許す
        let divergence = java_divergence(
            "class C { int f(Data data) { int size = 0; Supplier<Integer> s = data::size; return size + s.get(); } }",
            "class C { int f(Data other) { int count = 0; Supplier<Integer> s = other::count; return count + s.get(); } }",
        );

        assert_eq!(divergence, vec!["size ↔ count"]);
    }

    #[test]
    fn test_leaves_of_java_methods_referring_with_type_arguments_to_methods_named_like_locals_diverge()
     {
        // `::` と名前の間に型引数が入っても、名前は別の相手
        let divergence = java_divergence(
            "class C { int f(Data data) { int size = 0; Supplier<Integer> s = data::<T>size; return size + s.get(); } }",
            "class C { int f(Data data) { int count = 0; Supplier<Integer> s = data::<T>count; return count + s.get(); } }",
        );

        assert_eq!(divergence, vec!["size ↔ count"]);
    }

    #[test]
    fn test_leaves_of_java_methods_overriding_different_methods_in_anonymous_classes_diverge() {
        // 匿名クラスのメソッドの名前は束縛ではない。`run` の上書きと新しい `go` は別物
        let divergence = java_divergence(
            "class C { Thread f() { return new Thread() { public void run() { work(); } }; } }",
            "class C { Thread f() { return new Thread() { public void go() { work(); } }; } }",
        );

        assert_eq!(divergence, vec!["run ↔ go"]);
    }

    #[test]
    fn test_leaves_of_java_methods_naming_annotation_elements_like_their_locals_diverge() {
        // 注釈の要素の名前は、同じ綴りの束縛とは別の相手
        let divergence = java_divergence(
            "class C { int f() { int value = 0; @Tag(value = 1) int other = value; return other; } }",
            "class C { int f() { int level = 0; @Tag(level = 1) int other = level; return other; } }",
        );

        assert_eq!(divergence, vec!["value ↔ level"]);
    }

    #[test]
    fn test_leaves_of_java_methods_renaming_an_unlisted_binder_diverge() {
        // ラベルは束縛の一覧に無い形。一覧から漏れた形は「違う」側へ倒れる
        let divergence = java_divergence(
            "class C { void f(int n) { outer: for (int i = 0; i < n; i++) { break outer; } } }",
            "class C { void f(int n) { inner: for (int i = 0; i < n; i++) { break inner; } } }",
        );

        assert_eq!(divergence, vec!["outer ↔ inner"]);
    }

    #[test]
    fn test_leaves_of_java_methods_differing_only_in_a_number_notation_diverge_at_the_literal() {
        let divergence = java_divergence(
            "class C { void f() { long total = 42; } }",
            "class C { void f() { long total = 0x1F; } }",
        );

        assert_eq!(divergence, vec!["42 ↔ 0x1F"]);
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
    fn test_leaves_of_rust_functions_differing_only_in_a_primitive_and_a_named_type_diverge_at_the_type()
     {
        // 本体の中の型の違いが `unaligned` に消えず、葉の違いとして出る
        let divergence = rust_divergence(
            "fn f() { let total: i32 = base; }",
            "fn f() { let total: Amount = base; }",
        );

        assert_eq!(divergence, vec!["i32 ↔ Amount"]);
    }

    #[test]
    fn test_leaves_of_rust_functions_differing_only_in_primitive_and_named_signature_types_do_not_diverge()
     {
        // シグネチャの型は型シグネチャ（Stage 2）が見る。対照は上の本体の型のテスト
        let divergence = rust_divergence(
            "fn total(a: i32) -> i32 { a.checked() }",
            "fn total(a: Amount) -> Amount { a.checked() }",
        );

        assert_eq!(divergence, vec!["identical"]);
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

    #[test]
    fn test_leaves_of_swift_functions_that_only_rename_what_they_bind_do_not_diverge() {
        // 引数の内部名・`let`・`for`・クロージャの引数。ラベル（`to`）は揃えてある
        let divergence = swift_divergence(
            "func monthly(to value: [Int]) -> Int { let add = { (a: Int, b) in a + b }; var total = 0; for item in value { total = add(total, item) }; return total }",
            "func daily(to input: [Int]) -> Int { let sum = { (c: Int, d) in c + d }; var acc = 0; for entry in input { acc = sum(acc, entry) }; return acc }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_renaming_every_kind_of_binder_do_not_diverge() {
        // タプル・if let（複数）・guard let・if case let・switch の case let・catch let・型変数
        let divergence = swift_divergence(
            "func f<T>(_ o: T?) -> Int { let (a, (b, _)) = (1, (2, 3)); if let p = o, let q = o { use(p, q) }; guard let r = o else { return a }; if case let .some(s) = o { use(s) }; switch o { case .some(let t): use(t); case let u: use(u) }; do { try g() } catch let e { use(e) }; return b + T.zero }",
            "func g<U>(_ x: U?) -> Int { let (c, (d, _)) = (1, (2, 3)); if let m = x, let n = x { use(m, n) }; guard let k = x else { return c }; if case let .some(v) = x { use(v) }; switch x { case .some(let w): use(w); case let y: use(y) }; do { try g() } catch let z { use(z) }; return d + U.zero }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_differing_only_in_a_parameter_label_diverge_at_the_label() {
        // ラベルは呼び出しの API で、束縛でもシグネチャの型でもない
        let divergence = swift_divergence(
            "func f(to x: Int) -> Int { let y = x + 1; return y }",
            "func f(at x: Int) -> Int { let y = x + 1; return y }",
        );

        assert_eq!(divergence, vec!["to ↔ at"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_labelling_arguments_differently_diverge_at_the_label() {
        // 呼び出しのラベルが束縛と同じ綴りでも、付け替えに乗せない
        let divergence = swift_divergence(
            "func f(_ x: Int) -> Int { return g(x: x) }",
            "func f(_ y: Int) -> Int { return g(y: y) }",
        );

        assert_eq!(divergence, vec!["x ↔ y"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_reading_different_properties_diverge_at_the_property() {
        // `rate` / `tax` は束縛と同じ綴りでもプロパティを指す
        let divergence = swift_divergence(
            "func f(_ rate: Int) -> Int { return self.rate * rate }",
            "func g(_ tax: Int) -> Int { return self.tax * tax }",
        );

        assert_eq!(divergence, vec!["rate ↔ tax"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_matching_different_cases_without_let_diverge() {
        // `let` の無いパターンの名前は式のパターンで、外の名前を指す
        let divergence = swift_divergence(
            "func f(_ o: Kind) -> Int { switch o { case .low: return 1; default: return 0 } }",
            "func f(_ o: Kind) -> Int { switch o { case .high: return 1; default: return 0 } }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_differing_only_in_signature_types_do_not_diverge() {
        // 引数・戻り値・型変数の制約・where 節の型は型シグネチャ（Stage 2）が見る
        let divergence = swift_divergence(
            "func total<T: Billing>(_ amount: Amount, _ item: T) -> Amount where T: Hashable { let next = amount + item.cents; return next }",
            "func sum<T: Stock>(_ amount: Double, _ item: T) -> Double where T: Equatable { let next = amount + item.cents; return next }",
        );

        assert_eq!(divergence, vec!["identical"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_building_different_types_in_their_bodies_diverge() {
        // 本体の中の型は型シグネチャが見ない
        let divergence = swift_divergence(
            "func ordered(_ amount: Int) -> Int { let items = [Int](); return items.count + amount }",
            "func queued(_ amount: Int) -> Int { let items = [Double](); return items.count + amount }",
        );

        assert_eq!(divergence, vec!["Int ↔ Double"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_labelling_a_parameter_after_a_binder_of_the_same_spelling_diverge()
     {
        // 内側の関数のラベル `to` は、先に束縛した `to` と同じ綴りでもラベル
        let divergence = swift_divergence(
            "func f(_ to: Int) -> Int { func g(to x: Int) -> Int { return x + to }; return to }",
            "func f(_ at: Int) -> Int { func g(at x: Int) -> Int { return x + at }; return at }",
        );

        assert_eq!(divergence, vec!["to ↔ at"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_matching_without_let_after_a_let_condition_diverge() {
        // `let` が効くのは次の `,` まで。2 つ目の条件の `low` / `high` は外の名前
        let divergence = swift_divergence(
            "func f(_ o: Int?) -> Int { if case let .some(s) = o, case .some(low) = o { return s }; return 0 }",
            "func f(_ o: Int?) -> Int { if case let .some(s) = o, case .some(high) = o { return s }; return 0 }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_looping_over_cases_without_let_diverge() {
        // `for case` の名前は式のパターンで、外の名前を指す
        let divergence = swift_divergence(
            "func f(_ xs: [Int?]) -> Int { var n = 0; for case .some(low) in xs { n += 1 }; return n }",
            "func f(_ xs: [Int?]) -> Int { var n = 0; for case .some(high) in xs { n += 1 }; return n }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_unwrapping_different_outer_names_by_shorthand_diverge() {
        // `if let rate {` は同じ名前で外の `rate` を読む。付け替えると読む相手が変わる
        let divergence = swift_divergence(
            "func f() -> Int { if let rate { return rate }; guard let rate else { return 0 }; return rate }",
            "func f() -> Int { if let tax { return tax }; guard let tax else { return 0 }; return tax }",
        );

        assert_eq!(divergence, vec!["rate ↔ tax"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_renaming_an_unlabelled_parameter_diverge() {
        // ラベルを書かない引数は内部名がラベルを兼ねる。付け替えると呼び出しの API が変わる
        let divergence = swift_divergence(
            "func f(x: Int) -> Int { let y = x + 1; return y }",
            "func f(z: Int) -> Int { let y = z + 1; return y }",
        );

        assert_eq!(divergence, vec!["x ↔ z"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_passing_different_implicit_members_diverge() {
        // `.low` / `.high` は先に束縛した `low` / `high` と同じ綴りでも列挙子
        let divergence = swift_divergence(
            "func f(_ low: Int) -> Int { return g(.low) + low }",
            "func f(_ high: Int) -> Int { return g(.high) + high }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_labelling_tuple_elements_differently_diverge() {
        // タプルのラベルは先に束縛した名前と同じ綴りでもラベル。要素の値は束縛として付け替える
        let divergence = swift_divergence(
            "func f(_ x: Int) -> Int { let t = (x: x, 1); return t.0 }",
            "func f(_ y: Int) -> Int { let t = (y: y, 1); return t.0 }",
        );

        assert_eq!(divergence, vec!["x ↔ y"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_binding_inside_different_cases_diverge_at_the_case() {
        // `case let .low(s)` の `low` は列挙子で、束縛は `s` だけ
        let divergence = swift_divergence(
            "func f(_ o: Kind) -> Int { switch o { case let .low(s): return s; default: return 0 } }",
            "func f(_ o: Kind) -> Int { switch o { case let .high(t): return t; default: return 0 } }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_initializers_differing_only_in_parameter_types_do_not_diverge() {
        let leaves_a = leaves_of(
            "struct S { init(_ amount: Amount) { self.value = amount.cents } }",
            Grammar::Swift,
            "init_declaration",
        );
        let leaves_b = leaves_of(
            "struct S { init(_ amount: Double) { self.value = amount.cents } }",
            Grammar::Swift,
            "init_declaration",
        );

        assert_eq!(
            divergence_of(leaves_a.compared_with(&leaves_b)),
            vec!["identical"]
        );
    }

    #[test]
    fn test_leaves_of_swift_functions_matching_different_names_by_if_case_without_let_diverge() {
        // `if case low = v` / `guard case low = v` の `low` は外の値と比べる式のパターン
        let divergence = swift_divergence(
            "func f(_ v: Int) -> Int { if case low = v { return 1 }; guard case low = v else { return 0 }; return 2 }",
            "func f(_ v: Int) -> Int { if case high = v { return 1 }; guard case high = v else { return 0 }; return 2 }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_catching_different_names_without_let_diverge() {
        // `catch low` の `low` は式のパターン。`catch let e` だけが束縛する
        let divergence = swift_divergence(
            "func f() -> Int { do { try g() } catch low { return 1 } catch { return 0 }; return 2 }",
            "func f() -> Int { do { try g() } catch high { return 1 } catch { return 0 }; return 2 }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_matching_payloads_by_different_labels_diverge() {
        // パターンのラベルは先に束縛した名前と同じ綴りでも、どの case かを決める
        let divergence = swift_divergence(
            "func f(_ low: Kind) -> Int { switch low { case .pt(low: let a): return a; default: return 0 } }",
            "func f(_ high: Kind) -> Int { switch high { case .pt(high: let a): return a; default: return 0 } }",
        );

        assert_eq!(divergence, vec!["low ↔ high"]);
    }

    #[test]
    fn test_leaves_of_swift_functions_declaring_tuple_types_with_different_labels_diverge() {
        // 本体のタプル型のラベルは、先に束縛した名前と同じ綴りでもラベル
        let divergence = swift_divergence(
            "func f(_ x: Int) -> Int { let t: (x: Int, z: Int) = (x, 1); return t.z }",
            "func f(_ y: Int) -> Int { let t: (y: Int, z: Int) = (y, 1); return t.z }",
        );

        assert_eq!(divergence, vec!["x ↔ y"]);
    }
}
