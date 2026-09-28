//! rust-analyzer の hover が返す関数の綴りを、単一化の可否を比べられる形にする。
//!
//! **TypeScript の `type_structure` とは分ける。** 文法が別で、構文木のノードの種別で
//! 分岐する関数群に 2 つの言語を混ぜると、片方の形を足すたびに他方へ漏れる。
//!
//! **書かれ方の違いを落とす。** hover は inline の境界を `where` へ寄せて返すが、
//! `+` の並びと、同じ型変数への述語の分け方は書かれたとおりに残す
//! （rust-analyzer 1.94.1 で実測）。境界を**左辺の型と境界 1 つの組の集合**で持つと、
//! この 3 つの書き分けがまとめて落ちる。
//!
//! **LSP を知らない。** 綴りを受け取って形を返すだけなので、`semantics` へ依存しない
//! （`rules/architecture.md`「依存方向のルール」）。

use std::collections::BTreeSet;

use tree_sitter::Node;

use crate::syntax::tree::{Grammar, SyntaxTree};

/// 付け替えた型変数の綴りの前置き。
///
/// `%` は識別子に使えない文字なので、**元の型名と衝突しない**
/// （`syntax::type_structure` と同じ印）。
const PLACEHOLDER_PREFIX: char = '%';

/// hover の綴りを 1 つの宣言として読むために末尾へ足す字句。
///
/// hover は本体の無いシグネチャを返し、`where` 句は末尾の `,` で終わる。`;` を足すと
/// 本体の無い関数の宣言（`function_signature_item`）として読める（`where T: A, ;` も
/// Rust の文法上正しい）。
const DECLARATION_TERMINATOR: &str = ";";

/// 本体の無い関数の宣言を表すノードの種別。
const FUNCTION_SIGNATURE_KIND: &str = "function_signature_item";

/// ライフタイムを表すノードの種別。
const LIFETIME_KIND: &str = "lifetime";

/// どこにも束縛されない、プログラム全体で生きるライフタイム。
///
/// **これだけは落とさない。** 型変数ではなく具体的な寿命で、受け付ける値が変わる
/// （`&'static str` には一時的な文字列を渡せない）。
const STATIC_LIFETIME: &str = "'static";

/// 引数を持たない呼び出しの戻り値の綴り。
///
/// **戻り値の型を省いた綴りと `-> ()` を同じにする。** どちらも同じ型を返す。
const UNIT_TYPE: &str = "()";

/// `impl` の対象を指す型の名前。
const SELF_TYPE: &str = "Self";

/// Rust の関数 1 つ分の、単一化の可否を比べられる形。
///
/// 関数名・引数名・可視性を落とし、**関数が束縛した型変数を出現順に付け替えてある**
/// （引数 → 戻り値 → 境界の順）。**ライフタイムは `'static` を除いて落としてある。**
///
/// **Why（ライフタイムを落とす）**: 省略した綴り（`fn(x: &str) -> &str`）と
/// 書いた綴り（`fn<'a>(x: &'a str) -> &'a str`）は同じシグネチャだが、hover は
/// 書かれたとおりに返す。含めて比べると、**省略規則を実装しない限り同じ型が単一化不能に出る**。
///
/// **Why not（型変数と同じく付け替えて残す）**: 上の食い違いが解けない。省略規則まで
/// 読み解くと、綴りの読み方が hover の書き方の数だけ増える。落とした結果、
/// **ライフタイムの結び付けだけが違う 2 つは単一化可能に出る**が、その違いは本体
/// （どちらの引数を返すか）の違いとして構造類似度の側に出る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustCallable {
    /// `async` / `unsafe` / `const` / `extern "C"`。**並びは落とす**（書く順は自由）。
    ///
    /// **落とさない。** `async` は戻り値の型を、`unsafe` は呼び出しの約束を変える。
    modifiers: BTreeSet<String>,
    /// 引数の型の綴り。**書かれた順**（呼び出しの位置が型の一部）。
    parameters: Vec<String>,
    /// 戻り値の型の綴り。省かれていれば [`UNIT_TYPE`]。
    value_type: String,
    /// 関数が束縛した型変数の数。
    ///
    /// **使われない型変数も数える。** `f::<u8>()` と書けるかが変わる。
    type_parameter_count: usize,
    /// 境界。**左辺の型の綴りと、境界 1 つの綴りの組**の集合。
    ///
    /// **集合で持つ。** inline と `where` の書き分け・`+` の並び・同じ左辺への述語の
    /// 分け方は、どれも同じ要求を 2 通り以上に綴ったもの。
    trait_bounds: BTreeSet<(String, String)>,
    /// 比較に残る綴りに現れた型名。名前順。
    ///
    /// **束縛した型変数・プリミティブ・`?Sized`・関連型の名前（`Item = u8` の `Item`）は
    /// 入らない。** どれも宣言を辿る相手が居ない。パスで書かれた型は**パスごと** 1 つの名前。
    type_names: BTreeSet<String>,
    /// `Self` かレシーバ（`&self`）を持つか。
    ///
    /// **指す先が書かれた場所で決まる。** 別々の `impl` の `&self` は綴りが同じでも
    /// 別の型を受け取る。
    refers_to_self: bool,
}

impl RustCallable {
    /// hover が返した関数の綴りから読む。関数の宣言として読めなければ `None`。
    ///
    /// **const generics を持つ関数も `None`。** hover は使用箇所を `[u8; {const}]` と
    /// 綴るので型として読めず、読めた部分だけで比べると長さの違う配列が重なる。
    pub(crate) fn from_spelling(spelling: &str) -> Option<Self> {
        let declaration = format!("{spelling}{DECLARATION_TERMINATOR}");
        let tree = SyntaxTree::from_source(&declaration, Grammar::Rust).ok()?;
        if tree.has_error() {
            return None;
        }

        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == FUNCTION_SIGNATURE_KIND)?;

        Spelling::new(&declaration).callable_of(function)
    }

    /// 比較に残る綴りに現れた型名。名前順。
    pub(crate) fn type_names(&self) -> &BTreeSet<String> {
        &self.type_names
    }

    /// `Self` かレシーバを持ち、指す先が書かれた場所で決まるか。
    pub(crate) fn refers_to_self(&self) -> bool {
        self.refers_to_self
    }
}

/// 1 つの関数の綴りを読み進める途中の状態。
///
/// **付け替えの番号は読んだ順に振る**ので、引数 → 戻り値 → 境界の順に歩く。
struct Spelling<'source> {
    source: &'source str,
    /// 関数が束縛した型変数の名前。宣言された順。
    declared: Vec<String>,
    /// 付け替えた型変数の名前。**番号の順**。
    numbered: Vec<String>,
    type_names: BTreeSet<String>,
    refers_to_self: bool,
}

impl<'source> Spelling<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            declared: Vec::new(),
            numbered: Vec::new(),
            type_names: BTreeSet::new(),
            refers_to_self: false,
        }
    }

    /// 関数の宣言のノードから組み立てる。読めない形があれば `None`。
    fn callable_of(mut self, function: Node<'_>) -> Option<RustCallable> {
        let modifiers = self.modifiers_of(function)?;

        // 境界は型変数をすべて宣言し終えてから読む（`T: Into<U>` の `U` が後ろで宣言されうる）
        let mut bounded = Vec::new();
        if let Some(type_parameters) = function.child_by_field_name("type_parameters") {
            bounded = self.declared_type_parameters_of(type_parameters)?;
        }

        let parameter_list = function.child_by_field_name("parameters")?;
        let mut parameters = Vec::new();
        for parameter in named_children_of(parameter_list) {
            parameters.push(self.parameter_spelling_of(parameter)?);
        }

        let value_type = match function.child_by_field_name("return_type") {
            Some(return_type) => self.spelling_of(return_type)?.unwrap_or_default(),
            None => UNIT_TYPE.to_owned(),
        };

        let mut trait_bounds = BTreeSet::new();
        for (left, bounds) in bounded {
            self.insert_trait_bounds(&mut trait_bounds, left, bounds)?;
        }
        for clause in named_children_of(function).filter(|node| node.kind() == "where_clause") {
            for predicate in named_children_of(clause) {
                self.insert_predicate(&mut trait_bounds, predicate)?;
            }
        }

        // どこにも現れない型変数も番号を持たせる。数が型の一部なので
        for name in self.declared.clone() {
            self.number_of(&name);
        }

        Some(RustCallable {
            modifiers,
            parameters,
            value_type,
            type_parameter_count: self.declared.len(),
            trait_bounds,
            type_names: self.type_names,
            refers_to_self: self.refers_to_self,
        })
    }

    /// 関数の修飾子。読めなければ `None`。
    fn modifiers_of(&self, function: Node<'_>) -> Option<BTreeSet<String>> {
        let mut modifiers = BTreeSet::new();
        for node in named_children_of(function).filter(|node| node.kind() == "function_modifiers") {
            let mut cursor = node.walk();
            for modifier in node.children(&mut cursor) {
                modifiers.insert(collapsed(self.text_of(modifier)?));
            }
        }

        Some(modifiers)
    }

    /// 型変数を宣言し、inline の境界を `(左辺, 境界の並び)` で返す。読めなければ `None`。
    fn declared_type_parameters_of<'tree>(
        &mut self,
        type_parameters: Node<'tree>,
    ) -> Option<Vec<(Node<'tree>, Node<'tree>)>> {
        let mut bounded = Vec::new();

        for parameter in named_children_of(type_parameters) {
            match parameter.kind() {
                // 落とす（[`RustCallable`] の doc）。`'a: 'b` もライフタイムどうしの約束なので一緒に落ちる
                "lifetime_parameter" => {}
                "type_parameter" => {
                    let name = parameter.child_by_field_name("name")?;
                    self.declared.push(self.text_of(name)?.to_owned());
                    if let Some(bounds) = parameter.child_by_field_name("bounds") {
                        bounded.push((name, bounds));
                    }
                }
                // const generics（[`RustCallable::from_spelling`]）と、一覧に無い形
                _ => return None,
            }
        }

        Some(bounded)
    }

    /// 引数 1 つの型の綴り。読めなければ `None`。
    fn parameter_spelling_of(&mut self, parameter: Node<'_>) -> Option<String> {
        match parameter.kind() {
            "parameter" => {
                let parameter_type = parameter.child_by_field_name("type")?;
                self.spelling_of(parameter_type)?
            }
            "self_parameter" => {
                self.refers_to_self = true;
                self.spelling_of(parameter)?
            }
            _ => None,
        }
    }

    /// `where` 句の述語 1 つを境界の集合へ足す。読めなければ `None`。
    fn insert_predicate(
        &mut self,
        trait_bounds: &mut BTreeSet<(String, String)>,
        predicate: Node<'_>,
    ) -> Option<()> {
        let left = predicate.child_by_field_name("left")?;
        let bounds = predicate.child_by_field_name("bounds")?;

        // ライフタイムどうしの約束（`'a: 'b`）は落とす（[`RustCallable`] の doc）
        if left.kind() == LIFETIME_KIND {
            return Some(());
        }

        self.insert_trait_bounds(trait_bounds, left, bounds)
    }

    /// 左辺 1 つに課された境界の並びを、1 つずつ集合へ足す。読めなければ `None`。
    fn insert_trait_bounds(
        &mut self,
        trait_bounds: &mut BTreeSet<(String, String)>,
        left: Node<'_>,
        bounds: Node<'_>,
    ) -> Option<()> {
        let Some(left) = self.spelling_of(left)? else {
            return Some(());
        };

        for bound in named_children_of(bounds) {
            if let Some(bound) = self.spelling_of(bound)? {
                trait_bounds.insert((left.clone(), bound));
            }
        }

        Some(())
    }

    /// 型 1 つ分の、書かれ方を落とした綴り。
    ///
    /// 外側の `Option` は読めたか、内側は**落としたか**（`'static` 以外のライフタイム）。
    /// 並びの中で落ちたものは区切りごと消す（`Foo<'a, T>` は `Foo<T>`）。
    fn spelling_of(&mut self, node: Node<'_>) -> Option<Option<String>> {
        let spelled = match node.kind() {
            LIFETIME_KIND => {
                let lifetime = self.text_of(node)?;
                return Some((lifetime == STATIC_LIFETIME).then(|| lifetime.to_owned()));
            }
            // 束縛するライフタイムは、使う側と一緒に落ちる
            "for_lifetimes" => return Some(None),
            // `for<'a> F: ..` と `F: for<'a> ..` のどちらも、束縛するライフタイムごと落として型だけを残す
            "higher_ranked_trait_bound" => {
                return self.spelling_of(node.child_by_field_name("type")?);
            }
            "unit_type" => UNIT_TYPE.to_owned(),
            "type_identifier" => self.type_identifier_spelling_of(node)?,
            "scoped_type_identifier" => self.scoped_type_spelling_of(node)?,
            // `?Sized` は `Sized` にしか付けられず、宣言を辿る相手ではない
            "removed_trait_bound" => collapsed(self.text_of(node)?),
            // 関連型の名前は、それを持つトレイトの側で決まる
            "type_binding" => {
                let name = self.text_of(node.child_by_field_name("name")?)?;
                let bound = node.child_by_field_name("type")?;
                format!("{name} = {}", self.spelling_of(bound)?.unwrap_or_default())
            }
            "generic_type" => {
                let generic = self.spelling_of(node.child_by_field_name("type")?)?;
                let arguments =
                    self.listed_spellings_of(node.child_by_field_name("type_arguments")?)?;
                let generic = generic.unwrap_or_default();
                if arguments.is_empty() {
                    generic
                } else {
                    format!("{generic}<{}>", arguments.join(", "))
                }
            }
            // `+` で並ぶ要求は並びを問わない
            "bounded_type" | "trait_bounds" => {
                let mut members = BoundedMembers::default();
                self.bounded_members_of(node, &mut members)?;
                members.spelled.sort();
                let joined = members.spelled.join(" + ");
                match members.keyword {
                    Some(keyword) => format!("{keyword} {joined}"),
                    None => joined,
                }
            }
            _ => self.joined_children_of(node)?,
        };

        Some(Some(spelled))
    }

    /// 名前付きの子を 1 つずつ綴り、落ちたものを除いて返す。読めなければ `None`。
    fn listed_spellings_of(&mut self, list: Node<'_>) -> Option<Vec<String>> {
        let mut spelled = Vec::new();
        for child in named_children_of(list) {
            if let Some(child) = self.spelling_of(child)? {
                spelled.push(child);
            }
        }

        Some(spelled)
    }

    /// `+` で並ぶ要求を、入れ子を開いて集める。読めなければ `None`。
    fn bounded_members_of(&mut self, node: Node<'_>, members: &mut BoundedMembers) -> Option<()> {
        for child in named_children_of(node) {
            match child.kind() {
                "bounded_type" => self.bounded_members_of(child, members)?,
                // tree-sitter は `dyn A + B` の `dyn` を先頭の `A` にだけ付ける。
                // 並べ替える前に外へ出さないと、先頭が替わった綴りで `dyn` の付く相手が替わる
                "dynamic_type" | "abstract_type" => {
                    members.keyword = Some(self.text_of(child.child(0)?)?.to_owned());
                    let bound = child.child_by_field_name("trait")?;
                    if let Some(member) = self.spelling_of(bound)? {
                        members.spelled.push(member);
                    }
                }
                _ => {
                    if let Some(member) = self.spelling_of(child)? {
                        members.spelled.push(member);
                    }
                }
            }
        }

        Some(())
    }

    /// 子を順に綴り、空白 1 つで繋ぐ。読めなければ `None`。
    ///
    /// **字句を持たないノードは自分のテキストを返す**（`i32` / `&` / `mut`）。
    fn joined_children_of(&mut self, node: Node<'_>) -> Option<String> {
        if node.child_count() == 0 {
            return Some(self.text_of(node)?.to_owned());
        }

        let mut spelled = Vec::new();
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        for child in children {
            if let Some(child) = self.spelling_of(child)? {
                spelled.push(child);
            }
        }

        Some(spelled.join(" "))
    }

    /// 型名 1 つ。関数が束縛した型変数なら付け替え、そうでなければ型名として数える。
    fn type_identifier_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        let name = self.text_of(node)?;
        if let Some(number) = self.number_of(name) {
            return Some(format!("{PLACEHOLDER_PREFIX}{number}"));
        }

        if name == SELF_TYPE {
            self.refers_to_self = true;
        } else {
            self.type_names.insert(name.to_owned());
        }

        Some(name.to_owned())
    }

    /// パスで書かれた型。
    ///
    /// **先頭が型変数なら関連型**（`T::Item`）で、型変数だけを付け替える。
    /// **先頭がモジュールなら、パスごと 1 つの型名。** 末尾の名前だけを数えると、
    /// 別のモジュールの同名の型と重なる。
    fn scoped_type_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        let path = node.child_by_field_name("path")?;
        let name = self.text_of(node.child_by_field_name("name")?)?.to_owned();

        match path.kind() {
            "identifier" | "type_identifier" => {
                let head = self.text_of(path)?;
                if let Some(number) = self.number_of(head) {
                    return Some(format!("{PLACEHOLDER_PREFIX}{number}::{name}"));
                }
                if head == SELF_TYPE {
                    self.refers_to_self = true;
                    return Some(format!("{SELF_TYPE}::{name}"));
                }
                self.whole_path_of(node)
            }
            "scoped_identifier" | "self" | "super" | "crate" => self.whole_path_of(node),
            // `<T as Trait>::Out` / `Vec<T>::Out`。先頭を型として綴る
            _ => {
                let head = self.spelling_of(path)?.unwrap_or_default();
                Some(format!("{head}::{name}"))
            }
        }
    }

    /// パスごと 1 つの型名として数えた綴り。
    fn whole_path_of(&mut self, node: Node<'_>) -> Option<String> {
        let path = collapsed(self.text_of(node)?);
        self.type_names.insert(path.clone());

        Some(path)
    }

    /// 関数が束縛した型変数なら、その番号。初めて現れたなら次の番号を振る。
    fn number_of(&mut self, name: &str) -> Option<usize> {
        if !self.declared.iter().any(|declared| declared == name) {
            return None;
        }

        match self.numbered.iter().position(|numbered| numbered == name) {
            Some(number) => Some(number),
            None => {
                self.numbered.push(name.to_owned());
                Some(self.numbered.len() - 1)
            }
        }
    }

    fn text_of(&self, node: Node<'_>) -> Option<&'source str> {
        self.source.get(node.byte_range())
    }
}

/// `+` で並ぶ要求を集めた途中の形。
#[derive(Default)]
struct BoundedMembers {
    /// `dyn` / `impl`。並び全体に掛かる。
    keyword: Option<String>,
    /// 要求 1 つずつの綴り。
    spelled: Vec<String>,
}

/// 名前付きの子。書かれた順。
fn named_children_of(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
    children.into_iter()
}

/// 空白の並びを 1 つに畳む。hover は `where` 句を改行と字下げで返す。
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(spelling: &str) -> RustCallable {
        RustCallable::from_spelling(spelling).expect("関数の綴りとして読める")
    }

    #[test]
    fn test_from_spelling_inline_and_where_bounds_are_the_same_callable() {
        let inline = read("pub fn a<T: Display + Clone>(items: &[T]) -> String");
        let where_clause =
            read("pub fn b<T>(items: &[T]) -> String\nwhere\n    T: Display + Clone,");

        assert_eq!(inline, where_clause);
    }

    #[test]
    fn test_from_spelling_bound_order_is_dropped() {
        let written = read("pub fn a<T>(items: &[T]) -> String\nwhere\n    T: Display + Clone,");
        let reordered = read("pub fn b<T>(items: &[T]) -> String\nwhere\n    T: Clone + Display,");

        assert_eq!(written, reordered);
    }

    #[test]
    fn test_from_spelling_split_predicates_are_the_same_as_one() {
        let split = read("fn a<T>(t: T)\nwhere\n    T: Display,\n    T: Clone,");
        let joined = read("fn b<T>(t: T)\nwhere\n    T: Clone + Display,");

        assert_eq!(split, joined);
    }

    #[test]
    fn test_from_spelling_different_trait_bounds_are_different_callables() {
        // Issue の例。構造は同じだが、要求している能力が違う
        let display = read("fn process<T>(items: &[T]) -> String\nwhere\n    T: Display,");
        let serialize = read("fn process<T>(items: &[T]) -> String\nwhere\n    T: Serialize,");

        assert_ne!(display, serialize);
    }

    #[test]
    fn test_from_spelling_missing_bound_is_a_different_callable() {
        let bounded = read("fn a<T>(t: T)\nwhere\n    T: Clone,");
        let unbounded = read("fn b<T>(t: T)");

        assert_ne!(bounded, unbounded);
    }

    #[test]
    fn test_from_spelling_type_variable_and_parameter_names_are_dropped() {
        let written = read("fn a<T, U>(first: T, second: U) -> U\nwhere\n    U: Clone,");
        let renamed = read("fn b<K, V>(x: K, y: V) -> V\nwhere\n    V: Clone,");

        assert_eq!(written, renamed);
    }

    #[test]
    fn test_from_spelling_bound_follows_the_variable_it_constrains() {
        // 番号は出現順なので、境界がどちらの引数の型に付いたかが残る
        let first = read("fn a<T, U>(t: T, u: U)\nwhere\n    T: Clone,");
        let second = read("fn b<T, U>(t: T, u: U)\nwhere\n    U: Clone,");

        assert_ne!(first, second);
    }

    #[test]
    fn test_from_spelling_declaration_order_of_type_variables_is_dropped() {
        let declared = read("fn a<T, U>(t: T, u: U)\nwhere\n    T: Clone,");
        let swapped = read("fn b<U, T>(t: T, u: U)\nwhere\n    T: Clone,");

        assert_eq!(declared, swapped);
    }

    #[test]
    fn test_from_spelling_elided_and_named_lifetimes_are_the_same_callable() {
        let elided = read("fn a(x: &str) -> &str");
        let named = read("fn b<'a>(x: &'a str) -> &'a str");

        assert_eq!(elided, named);
    }

    #[test]
    fn test_from_spelling_lifetime_bound_is_dropped() {
        let outlives = read("fn a<'a, T>(x: &'a T) -> &'a str\nwhere\n    T: 'a + Debug,");
        let plain = read("fn b<T>(x: &T) -> &str\nwhere\n    T: Debug,");

        assert_eq!(outlives, plain);
    }

    #[test]
    fn test_from_spelling_lifetime_argument_is_dropped_with_its_separator() {
        let with_lifetime = read("fn a<'a>(x: Cow<'a, str>)");
        let without = read("fn b(x: Cow<str>)");

        assert_eq!(with_lifetime, without);
    }

    #[test]
    fn test_from_spelling_higher_ranked_bound_is_the_same_as_elided() {
        let higher_ranked = read("fn a<F>(f: F)\nwhere\n    for<'a> F: Fn(&'a str) -> &'a str,");
        let elided = read("fn b<F>(f: F)\nwhere\n    F: Fn(&str) -> &str,");

        assert_eq!(higher_ranked, elided);
    }

    #[test]
    fn test_from_spelling_higher_ranked_bound_on_the_right_is_the_same_as_elided() {
        let higher_ranked = read("fn a<F>(f: F)\nwhere\n    F: for<'a> Fn(&'a str) -> &'a str,");
        let elided = read("fn b<F>(f: F)\nwhere\n    F: Fn(&str) -> &str,");

        assert_eq!(higher_ranked, elided);
    }

    #[test]
    fn test_from_spelling_static_lifetime_is_kept() {
        let borrowed = read("fn a(x: &str)");
        let static_str = read("fn b(x: &'static str)");

        assert_ne!(borrowed, static_str);
    }

    #[test]
    fn test_from_spelling_unit_return_is_the_same_as_omitted() {
        assert_eq!(read("fn a(x: i32)"), read("fn b(x: i32) -> ()"));
    }

    #[test]
    fn test_from_spelling_async_is_a_different_callable() {
        assert_ne!(
            read("fn a(x: i32) -> i32"),
            read("async fn b(x: i32) -> i32")
        );
    }

    #[test]
    fn test_from_spelling_visibility_is_dropped() {
        assert_eq!(read("pub fn a(x: i32) -> i32"), read("fn b(x: i32) -> i32"));
    }

    #[test]
    fn test_from_spelling_parameter_order_is_kept() {
        assert_ne!(read("fn a(x: i32, y: u8)"), read("fn b(x: u8, y: i32)"));
    }

    #[test]
    fn test_from_spelling_unused_type_variable_is_a_different_callable() {
        assert_ne!(read("fn a<T>(x: i32)"), read("fn b(x: i32)"));
    }

    #[test]
    fn test_from_spelling_auto_trait_order_in_dyn_is_dropped() {
        assert_eq!(
            read("fn a(f: Box<dyn Fn(&str) -> bool + Send>)"),
            read("fn b(f: Box<dyn Send + Fn(&str) -> bool>)"),
        );
    }

    #[test]
    fn test_from_spelling_type_names_exclude_bound_variables_and_primitives() {
        let callable = read(
            "fn a<T>(items: &[T], count: usize) -> Vec<String>\nwhere\n    T: Display + ?Sized,",
        );

        let expected: BTreeSet<String> = ["Display", "String", "Vec"].map(String::from).into();
        assert_eq!(callable.type_names(), &expected);
    }

    #[test]
    fn test_from_spelling_type_names_exclude_associated_types() {
        let callable = read("fn a<I>(items: I) -> I::Item\nwhere\n    I: Iterator<Item = u8>,");

        let expected: BTreeSet<String> = ["Iterator"].map(String::from).into();
        assert_eq!(callable.type_names(), &expected);
    }

    #[test]
    fn test_from_spelling_path_is_one_type_name() {
        let callable = read("fn a(x: fmt::Result, y: std::io::Error)");

        let expected: BTreeSet<String> = ["fmt::Result", "std::io::Error"].map(String::from).into();
        assert_eq!(callable.type_names(), &expected);
    }

    #[test]
    fn test_from_spelling_receiver_refers_to_self() {
        assert!(read("pub fn m(&self, v: Vec<u8>) -> usize").refers_to_self());
    }

    #[test]
    fn test_from_spelling_self_type_refers_to_self() {
        assert!(read("pub async unsafe fn o(self: Box<Self>)").refers_to_self());
    }

    #[test]
    fn test_from_spelling_associated_type_of_self_refers_to_self() {
        assert!(read("fn new(item: Self::Item) -> usize").refers_to_self());
    }

    #[test]
    fn test_from_spelling_free_function_does_not_refer_to_self() {
        assert!(!read("pub fn first(value: i32) -> i32").refers_to_self());
    }

    #[test]
    fn test_from_spelling_const_generics_are_unreadable() {
        // hover は使用箇所を `{const}` と綴る
        assert_eq!(
            RustCallable::from_spelling("pub fn h<const N: usize>(x: [u8; {const}]) -> (i32, u8)"),
            None
        );
    }

    #[test]
    fn test_from_spelling_non_function_is_unreadable() {
        assert_eq!(RustCallable::from_spelling("pub struct S"), None);
    }
}
