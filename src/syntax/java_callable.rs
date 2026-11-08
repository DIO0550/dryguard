//! jdtls の hover が返すメソッド・コンストラクタの綴りを、単一化の可否を比べられる形にする。
//!
//! **TypeScript の `type_structure`・Rust の `rust_callable`・Swift の `swift_callable` とは分ける。**
//! 文法が別で、構文木のノードの種別で分岐する関数群に言語を混ぜると、片方の形を足すたびに
//! 他方へ漏れる。
//!
//! **ソースと hover の綴りを同じ読み手で読む。** 型名の綴り（パスの繋ぎ方）を 2 通りに
//! 書くと、ソースで集めた型名と hover の綴りに残った型名が食い違い、辿った記録が
//! 綴りで引けなくなる（`syntax::swift_callable` の `type_references_of` と同じ形）。
//!
//! **jdtls 1.63.0 の hover は `int shop.Total.total(List<Integer> items, double discount)` の形**
//! （戻り値・パッケージで修飾したクラス名・メソッド名・引数名付きの引数。#394 で実測）。
//! 修飾した名前を末尾の名前にすれば Java の宣言として読める。ジェネリック・コンストラクタ・
//! `throws`・可変長の綴りも同じ形と見て読み、**宣言として読めない綴りは `None`**（偽陰性側）。
//!
//! **LSP を知らない。** 綴りを受け取って形を返すだけなので、`semantics` へ依存しない
//! （`rules/architecture.md`「依存方向のルール」）。

use std::collections::BTreeSet;

use tree_sitter::Node;

use crate::syntax::tree::{Grammar, SyntaxTree, source_position_of};
use crate::syntax::type_reference::TypeReference;

/// hover の綴りを 1 つのメンバーとして読むための前置き。
///
/// **型の中に包む。** コンストラクタは型の外では宣言として読めない。包む型の名前は
/// 読まないので、tree-sitter-java が名前の一致を確かめないことに頼っている（0.23 で確かめた）。
const MEMBER_WRAPPER_START: &str = "class _ {\n";

/// 包みの後ろ。hover の綴りは本体を持たないので、空の本体を足して宣言として読む。
const MEMBER_WRAPPER_END: &str = " {}\n}\n";

/// チャンクになる宣言の種別。`syntax::chunk` の Java のチャンクと同じ。
const METHOD_KIND: &str = "method_declaration";
const CONSTRUCTOR_KIND: &str = "constructor_declaration";

/// 型の宣言の種別。メンバーを直接囲む型を探すときに止まる先。
const TYPE_DECLARATION_KINDS: [&str; 4] = [
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
];

/// 名前を持たない型の本体を持つ式。ここで止まったら、囲む型を名前で言えない。
const ANONYMOUS_BODY_OWNER_KINDS: [&str; 2] = ["object_creation_expression", "enum_constant"];

/// 宣言の前に付く修飾子（注釈を含む）と、その中の `static`。
const MODIFIERS_KIND: &str = "modifiers";
const STATIC_TOKEN: &str = "static";

/// 引数のリストと、その中の引数の種別。
const FORMAL_PARAMETER_KIND: &str = "formal_parameter";
const SPREAD_PARAMETER_KIND: &str = "spread_parameter";
const RECEIVER_PARAMETER_KIND: &str = "receiver_parameter";

/// 型変数のリストと、その 1 つ・境界。
const TYPE_PARAMETER_KIND: &str = "type_parameter";
const TYPE_BOUND_KIND: &str = "type_bound";

/// `throws` 節。
const THROWS_KIND: &str = "throws";

/// 型の構文の種別。
const TYPE_IDENTIFIER_KIND: &str = "type_identifier";
const SCOPED_TYPE_IDENTIFIER_KIND: &str = "scoped_type_identifier";
const GENERIC_TYPE_KIND: &str = "generic_type";
const ARRAY_TYPE_KIND: &str = "array_type";
const ANNOTATED_TYPE_KIND: &str = "annotated_type";
const TYPE_ARGUMENTS_KIND: &str = "type_arguments";
const WILDCARD_KIND: &str = "wildcard";
const SUPER_KIND: &str = "super";

/// プリミティブ型の種別。**この一覧は増えない**（Java のプリミティブ型と `void` で尽きる）。
const PRIMITIVE_TYPE_KINDS: [&str; 4] = [
    "integral_type",
    "floating_point_type",
    "boolean_type",
    "void_type",
];

/// 注釈の種別。型の前・引数の前に付いても、型を変えないので落とす。
const ANNOTATION_KINDS: [&str; 2] = ["marker_annotation", "annotation"];

/// 配列の次元 1 つを開く字句。
const DIMENSION_OPEN_TOKEN: &str = "[";

/// Java のメソッド・コンストラクタ 1 つ分の、単一化の可否を比べられる形。
///
/// メソッド名・引数名・修飾子・注釈を落とし、メソッド自身が束縛した型変数を
/// 出現順に付け替える（引数 → 戻り値 → `throws`、現れなかったものは宣言順）。
/// **クラスが束縛した型変数は付け替えない** — メソッドの綴りからは辿れず、型名として
/// 宣言の場所で比べる（`rules/architecture.md`「外側のスコープが束縛した型変数は外さない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JavaCallable {
    kind: JavaCallableKind,
    /// インスタンスメソッドなら直接囲む型。static メソッドとコンストラクタは `None`。
    ///
    /// **hover の綴りは `static` を綴らない**（`int shop.Total.total(…)` は static メソッド。
    /// #394 で実測）ので、ソースから運ぶ。入れないと、別々のクラスの同じ形のインスタンス
    /// メソッドが重なる（偽陽性。Swift の `enclosing type` と同じ線）。
    receiver: Option<JavaType>,
    /// 引数の型。可変長の引数（`T...`）は、本体の中で見える配列の型（`T[]`）を置く。
    parameters: Vec<JavaType>,
    /// 最後の引数が可変長（`T...`）か。**`T[]` と重ねない** — 呼び出しの書き方が違う。
    variadic: bool,
    /// 戻り値の型。コンストラクタは囲む型。
    value_type: JavaType,
    /// `throws` の例外の型。**並びは落とす**（書く順は自由）が、**型は残す** —
    /// 検査例外は呼び出し側の書き方を変える。
    exceptions: BTreeSet<JavaType>,
    /// メソッド自身が束縛した型変数の数。**使われない型変数も数える**（`<T>f()` の呼び方が変わる）。
    type_parameter_count: usize,
    /// 境界。**型変数の番号と境界 1 つの組の集合**で持つ（`rules/naming.md` の `trait bound`）。
    /// `&` の並びを落とす。
    bounds: BTreeSet<(usize, JavaType)>,
    /// 比較に残る綴りに現れた型名。名前順。**パスで書かれた型はパスごと 1 つの名前。**
    ///
    /// メソッド自身が束縛した型変数とプリミティブ型は入らない。レシーバと構築する型の名前は入る。
    type_names: BTreeSet<String>,
}

/// 呼び出しの仕方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JavaCallableKind {
    /// `f(a)` / `T.f(a)` / `value.f(a)`。
    Method,
    /// `new T(a)`。**メソッドと重ねない** — 戻り値の型が同じメソッドとは呼び方が違う
    /// （Swift の `init` と同じ線）。
    Constructor,
}

/// 型 1 つ分の形。**書かれ方の違い（注釈・C 形式の配列の次元）を落としてある。**
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum JavaType {
    /// プリミティブ型と `void`。綴りで持つ。
    Primitive(String),
    /// 名前で書かれた型。パスの区切りごとに型引数を持つ（`Outer<A>.Inner<B>`）。
    Named(JavaPath),
    /// メソッド自身が束縛した型変数。番号は出現順。
    Parameter(usize),
    /// 配列の 1 次元。
    Array(Box<JavaType>),
}

/// 名前で書かれた型のパス。区切りごとの名前と、その区切りに付いた型引数。
type JavaPath = Vec<(String, Vec<JavaTypeArgument>)>;

/// 型引数 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum JavaTypeArgument {
    Type(JavaType),
    /// `?`。
    Unbounded,
    /// `? extends T`。
    Extends(JavaType),
    /// `? super T`。
    Super(JavaType),
}

/// メンバーを直接囲む型の名前と、そのメンバーが static か。
///
/// **名前だけで持つ。** Java には Swift の制約付き `extension` に当たる形が無く、
/// クラスの型変数がメソッドの綴りに出れば、その型名がクラスの宣言の場所で区別される。
/// **型の宣言の型変数（`class Box<T>` の `<T>`）は持たない**（Swift の `enclosing type` と同じ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaEnclosingType {
    name: String,
    static_member: bool,
}

impl JavaEnclosingType {
    /// 囲む型の名前と、メンバーが static かから作る。
    pub(crate) fn new(name: impl Into<String>, static_member: bool) -> Self {
        Self {
            name: name.into(),
            static_member,
        }
    }

    fn to_type(&self) -> JavaType {
        JavaType::Named(vec![(self.name.clone(), Vec::new())])
    }
}

impl JavaCallable {
    /// hover が返した綴りから読む。宣言として読めなければ `None`。
    ///
    /// `enclosing_type` は、そのメンバーを直接囲む型（`Chunk::java_enclosing_type`）。
    ///
    /// **読めない形は `None`**（偽陰性側）: 修飾した名前の後ろに引数リストが無い綴り、
    /// tree-sitter-java が構文エラーにする綴り、囲む型を名前で言えないメンバー
    /// （匿名クラス・enum 定数の本体）。
    pub(crate) fn from_spelling(
        spelling: &str,
        enclosing_type: Option<&JavaEnclosingType>,
    ) -> Option<Self> {
        let enclosing = enclosing_type?;
        let declaration_text = unqualified_declaration_of(spelling)?;
        let wrapped = format!("{MEMBER_WRAPPER_START}{declaration_text}{MEMBER_WRAPPER_END}");
        let tree = SyntaxTree::from_source(&wrapped, Grammar::Java).ok()?;
        if tree.has_error() {
            return None;
        }
        let declaration = tree
            .named_descendants()
            .into_iter()
            .find(|node| is_callable_kind(node.kind()))?;

        let mut reader = JavaSpelling::new(&wrapped);
        let mut callable = reader.callable_of(declaration)?;
        let mut type_names = reader.type_names;
        match callable.kind {
            JavaCallableKind::Method => {
                if !enclosing.static_member {
                    callable.receiver = Some(enclosing.to_type());
                    type_names.insert(enclosing.name.clone());
                }
            }
            JavaCallableKind::Constructor => {
                callable.value_type = enclosing.to_type();
                type_names.insert(enclosing.name.clone());
            }
        }
        callable.type_names = type_names;
        Some(callable)
    }

    /// 比較に残る綴りに現れた型名。名前順。
    pub(crate) fn type_names(&self) -> &BTreeSet<String> {
        &self.type_names
    }
}

/// 修飾したメソッド名（`shop.Total.total`）を末尾の名前（`total`）にした綴り。
/// 引数リストの開きが無ければ `None`。
///
/// **引数リストは、型引数の `<>` の外にある最初の `(`。** 戻り値の型の型引数
/// （`Map<K, V>`）の中には `(` が現れない。
fn unqualified_declaration_of(spelling: &str) -> Option<String> {
    let mut depth = 0_usize;
    let mut opening = None;
    for (index, character) in spelling.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.checked_sub(1)?,
            '(' if depth == 0 => {
                opening = Some(index);
                break;
            }
            _ => {}
        }
    }
    let opening = opening?;
    let before = spelling.get(..opening)?.trim_end();
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_qualified_name_character(*character))
        .last()
        .map(|(index, _)| index)?;
    let qualified = before.get(start..)?;
    let name = qualified.rsplit('.').next()?;
    if name.is_empty() {
        return None;
    }
    Some(format!(
        "{}{name}{}",
        before.get(..start)?,
        spelling.get(opening..)?
    ))
}

fn is_qualified_name_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$' || character == '.'
}

fn is_callable_kind(kind: &str) -> bool {
    [METHOD_KIND, CONSTRUCTOR_KIND].contains(&kind)
}

/// Java のチャンクのシグネチャに書かれた型名と、その位置。**名前ごとに最初の 1 つ。**
///
/// `function` はチャンクのノード（メソッド・コンストラクタ）、`source` はそれを含むファイル全体。
/// **hover の綴りを読むのと同じ読み手で読む**ので、型名の綴りが揃う。
/// メソッド自身が束縛した型変数・プリミティブ型は集めない。
/// **インスタンスメソッドとコンストラクタは、囲む型の名前も集める** — 位置は型の宣言の名前。
/// [`JavaCallable::from_spelling`] がその名前をレシーバ・構築する型として比較に残す。
///
/// コンパクトコンストラクタ（record の `Point { … }`）は引数リストを持たないので読めず、空。
pub(crate) fn type_references_of(function: Node<'_>, source: &str) -> Vec<TypeReference> {
    let mut reader = JavaSpelling::new(source);
    reader.recording = true;
    let Some(callable) = reader.callable_of(function) else {
        return Vec::new();
    };
    let mut references = reader.references;

    let takes_enclosing = match callable.kind {
        JavaCallableKind::Method => !is_static_member(function, source),
        JavaCallableKind::Constructor => true,
    };
    if !takes_enclosing {
        return references;
    }
    let Some(name) = enclosing_declaration_of(function)
        .and_then(|declaration| declaration.child_by_field_name("name"))
    else {
        return references;
    };
    let Some(spelled) = source.get(name.byte_range()) else {
        return references;
    };
    let recorded = references
        .iter()
        .any(|reference| reference.name() == spelled);
    if !recorded && let Some(position) = source_position_of(name, source) {
        references.push(TypeReference::new(spelled.to_owned(), position));
    }
    references
}

/// そのメンバーを直接囲む型の名前と、static か。名前で言える型に囲まれていなければ `None`。
pub(crate) fn enclosing_type_of(function: Node<'_>, source: &str) -> Option<JavaEnclosingType> {
    let declaration = enclosing_declaration_of(function)?;
    let name = declaration.child_by_field_name("name")?;
    let static_member = function.kind() == METHOD_KIND && is_static_member(function, source);
    Some(JavaEnclosingType::new(
        source.get(name.byte_range())?,
        static_member,
    ))
}

/// メンバーを直接囲む型の宣言。匿名クラス・enum 定数の本体の中なら `None`。
///
/// **最初に当たった型の宣言で止まる。** 局所クラスもメソッドの本体の中の型の宣言なので、
/// 外側のクラスまで上がらない。
fn enclosing_declaration_of(function: Node<'_>) -> Option<Node<'_>> {
    let mut current = function.parent();
    while let Some(node) = current {
        if TYPE_DECLARATION_KINDS.contains(&node.kind()) {
            return Some(node);
        }
        if ANONYMOUS_BODY_OWNER_KINDS.contains(&node.kind()) {
            return None;
        }
        current = node.parent();
    }
    None
}

/// 宣言の修飾子に `static` があるか。
fn is_static_member(declaration: Node<'_>, source: &str) -> bool {
    children_of(declaration)
        .into_iter()
        .filter(|child| child.kind() == MODIFIERS_KIND)
        .flat_map(children_of)
        .any(|modifier| source.get(modifier.byte_range()) == Some(STATIC_TOKEN))
}

fn children_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn named_children_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// 1 つの宣言の綴りを読み進める途中の状態。
struct JavaSpelling<'source> {
    source: &'source str,
    /// メソッド自身が束縛した型変数の名前。宣言された順。
    declared: Vec<String>,
    /// 付け替えた型変数の名前。**番号の順**。
    numbered: Vec<String>,
    type_names: BTreeSet<String>,
    /// ソースを読むときだけ、型名の位置を残す。
    recording: bool,
    references: Vec<TypeReference>,
}

impl<'source> JavaSpelling<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            declared: Vec::new(),
            numbered: Vec::new(),
            type_names: BTreeSet::new(),
            recording: false,
            references: Vec::new(),
        }
    }

    fn text_of(&self, node: Node<'_>) -> Option<&'source str> {
        self.source.get(node.byte_range())
    }

    /// 宣言を読む。読めない形なら `None`。
    ///
    /// コンストラクタの値の型とレシーバは、呼び出し側が囲む型で埋める。
    fn callable_of(&mut self, declaration: Node<'_>) -> Option<JavaCallable> {
        let kind = match declaration.kind() {
            METHOD_KIND => JavaCallableKind::Method,
            CONSTRUCTOR_KIND => JavaCallableKind::Constructor,
            _ => return None,
        };
        let type_parameters = match declaration.child_by_field_name("type_parameters") {
            Some(list) => named_children_of(list)
                .into_iter()
                .filter(|node| node.kind() == TYPE_PARAMETER_KIND)
                .collect(),
            None => Vec::new(),
        };
        for parameter in &type_parameters {
            let name = self.type_parameter_name_of(*parameter)?.to_owned();
            if self.declared.contains(&name) {
                return None;
            }
            self.declared.push(name);
        }

        let (parameters, variadic) =
            self.parameters_of(declaration.child_by_field_name("parameters")?)?;
        let value_type = match kind {
            JavaCallableKind::Method => self.type_of(declaration.child_by_field_name("type")?)?,
            // 呼び出し側が囲む型で埋める
            JavaCallableKind::Constructor => JavaType::Primitive(String::new()),
        };
        let mut exceptions = BTreeSet::new();
        for clause in children_of(declaration)
            .into_iter()
            .filter(|child| child.kind() == THROWS_KIND)
        {
            for exception in named_children_of(clause) {
                exceptions.insert(self.type_of(exception)?);
            }
        }

        // 引数・戻り値・`throws` に現れなかった型変数は、宣言の順に番号を振る
        for name in self.declared.clone() {
            self.parameter_index_of(&name);
        }
        let mut bounds = BTreeSet::new();
        for parameter in &type_parameters {
            let index = self.parameter_index_of(self.type_parameter_name_of(*parameter)?);
            for bound in named_children_of(*parameter)
                .into_iter()
                .filter(|node| node.kind() == TYPE_BOUND_KIND)
            {
                for bound_type in named_children_of(bound) {
                    bounds.insert((index, self.type_of(bound_type)?));
                }
            }
        }

        Some(JavaCallable {
            kind,
            receiver: None,
            parameters,
            variadic,
            value_type,
            exceptions,
            type_parameter_count: self.declared.len(),
            bounds,
            type_names: BTreeSet::new(),
        })
    }

    fn type_parameter_name_of(&self, parameter: Node<'_>) -> Option<&'source str> {
        let name = named_children_of(parameter)
            .into_iter()
            .find(|node| node.kind() == TYPE_IDENTIFIER_KIND)?;
        self.text_of(name)
    }

    /// 型変数の番号。初めて現れたら次の番号を振る。
    fn parameter_index_of(&mut self, name: &str) -> usize {
        if let Some(index) = self.numbered.iter().position(|numbered| numbered == name) {
            return index;
        }
        self.numbered.push(name.to_owned());
        self.numbered.len() - 1
    }

    /// 引数の型の並びと、最後の引数が可変長か。読めない形なら `None`。
    ///
    /// **`Foo this`（receiver parameter）は落とす。** 呼び出しに書く引数ではない。
    fn parameters_of(&mut self, list: Node<'_>) -> Option<(Vec<JavaType>, bool)> {
        let mut parameters = Vec::new();
        let mut variadic = false;
        for parameter in named_children_of(list) {
            if variadic {
                // 可変長の引数の後ろに引数は書けない
                return None;
            }
            match parameter.kind() {
                FORMAL_PARAMETER_KIND => {
                    let mut read = self.type_of(parameter.child_by_field_name("type")?)?;
                    // `int a[]` の C 形式の次元は、`int[] a` と同じ型
                    if let Some(dimensions) = parameter.child_by_field_name("dimensions") {
                        read = arrays_of(read, dimension_count_of(dimensions));
                    }
                    parameters.push(read);
                }
                SPREAD_PARAMETER_KIND => {
                    let element = named_children_of(parameter)
                        .into_iter()
                        .find(|node| is_type_kind(node.kind()))?;
                    parameters.push(JavaType::Array(Box::new(self.type_of(element)?)));
                    variadic = true;
                }
                RECEIVER_PARAMETER_KIND => {}
                _ => return None,
            }
        }
        Some((parameters, variadic))
    }

    /// 型 1 つを読む。未知の型の構文なら `None`。
    fn type_of(&mut self, node: Node<'_>) -> Option<JavaType> {
        let kind = node.kind();
        if PRIMITIVE_TYPE_KINDS.contains(&kind) {
            return Some(JavaType::Primitive(self.text_of(node)?.to_owned()));
        }
        match kind {
            ARRAY_TYPE_KIND => {
                let element = self.type_of(node.child_by_field_name("element")?)?;
                let dimensions = node.child_by_field_name("dimensions")?;
                Some(arrays_of(element, dimension_count_of(dimensions)))
            }
            ANNOTATED_TYPE_KIND => {
                let annotated = named_children_of(node)
                    .into_iter()
                    .rfind(|child| !ANNOTATION_KINDS.contains(&child.kind()))?;
                self.type_of(annotated)
            }
            TYPE_IDENTIFIER_KIND | SCOPED_TYPE_IDENTIFIER_KIND | GENERIC_TYPE_KIND => {
                let (path, position_node) = self.path_of(node)?;
                Some(self.named_type_of(path, position_node))
            }
            _ => None,
        }
    }

    /// 名前で書かれた型のパス（区切りごとの名前と型引数）と、尋ねる位置にする末尾の名前のノード。
    fn path_of<'tree>(&mut self, node: Node<'tree>) -> Option<(JavaPath, Node<'tree>)> {
        match node.kind() {
            TYPE_IDENTIFIER_KIND => {
                Some((vec![(self.text_of(node)?.to_owned(), Vec::new())], node))
            }
            SCOPED_TYPE_IDENTIFIER_KIND => {
                let parts = named_children_of(node)
                    .into_iter()
                    .filter(|child| !ANNOTATION_KINDS.contains(&child.kind()))
                    .collect::<Vec<_>>();
                let [prefix, last] = parts.as_slice() else {
                    return None;
                };
                if last.kind() != TYPE_IDENTIFIER_KIND {
                    return None;
                }
                let (mut path, _) = self.path_of(*prefix)?;
                path.push((self.text_of(*last)?.to_owned(), Vec::new()));
                Some((path, *last))
            }
            GENERIC_TYPE_KIND => {
                let parts = named_children_of(node);
                let [base, arguments] = parts.as_slice() else {
                    return None;
                };
                if arguments.kind() != TYPE_ARGUMENTS_KIND {
                    return None;
                }
                let (mut path, position_node) = self.path_of(*base)?;
                let read = self.type_arguments_of(*arguments)?;
                let (_, last_arguments) = path.last_mut()?;
                *last_arguments = read;
                Some((path, position_node))
            }
            _ => None,
        }
    }

    fn type_arguments_of(&mut self, list: Node<'_>) -> Option<Vec<JavaTypeArgument>> {
        named_children_of(list)
            .into_iter()
            .map(|argument| {
                if argument.kind() == WILDCARD_KIND {
                    return self.wildcard_of(argument);
                }
                Some(JavaTypeArgument::Type(self.type_of(argument)?))
            })
            .collect()
    }

    fn wildcard_of(&mut self, wildcard: Node<'_>) -> Option<JavaTypeArgument> {
        let children = named_children_of(wildcard);
        let lower = children.iter().any(|child| child.kind() == SUPER_KIND);
        let Some(bound) = children
            .into_iter()
            .find(|child| is_type_kind(child.kind()))
        else {
            return Some(JavaTypeArgument::Unbounded);
        };
        let bound = self.type_of(bound)?;
        if lower {
            return Some(JavaTypeArgument::Super(bound));
        }
        Some(JavaTypeArgument::Extends(bound))
    }

    /// 名前で書かれた型。メソッドが束縛した型変数なら番号へ付け替え、そうでなければ型名に残す。
    fn named_type_of(&mut self, path: JavaPath, position_node: Node<'_>) -> JavaType {
        if let [(head, arguments)] = path.as_slice()
            && arguments.is_empty()
            && self.declared.contains(head)
        {
            let head = head.clone();
            return JavaType::Parameter(self.parameter_index_of(&head));
        }

        let name = path
            .iter()
            .map(|(segment, _)| segment.as_str())
            .collect::<Vec<_>>()
            .join(".");
        if self.recording
            && !self.type_names.contains(&name)
            && let Some(position) = source_position_of(position_node, self.source)
        {
            self.references
                .push(TypeReference::new(name.clone(), position));
        }
        self.type_names.insert(name);
        JavaType::Named(path)
    }
}

/// 型の構文のノードか。可変長の引数・ワイルドカードの中で、注釈・修飾子と型を見分ける。
fn is_type_kind(kind: &str) -> bool {
    PRIMITIVE_TYPE_KINDS.contains(&kind)
        || [
            TYPE_IDENTIFIER_KIND,
            SCOPED_TYPE_IDENTIFIER_KIND,
            GENERIC_TYPE_KIND,
            ARRAY_TYPE_KIND,
            ANNOTATED_TYPE_KIND,
        ]
        .contains(&kind)
}

/// `dimensions` のノードが表す次元の数。次元に付いた注釈（`int @A []`）は数えない。
fn dimension_count_of(dimensions: Node<'_>) -> usize {
    children_of(dimensions)
        .into_iter()
        .filter(|child| !child.is_named() && child.kind() == DIMENSION_OPEN_TOKEN)
        .count()
}

fn arrays_of(element: JavaType, dimensions: usize) -> JavaType {
    (0..dimensions).fold(element, |inner, _| JavaType::Array(Box::new(inner)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance_member_of(name: &str) -> JavaEnclosingType {
        JavaEnclosingType::new(name, false)
    }

    fn static_member_of(name: &str) -> JavaEnclosingType {
        JavaEnclosingType::new(name, true)
    }

    fn read(spelling: &str, enclosing: &JavaEnclosingType) -> JavaCallable {
        let Some(callable) = JavaCallable::from_spelling(spelling, Some(enclosing)) else {
            panic!("読める綴り: {spelling}");
        };
        callable
    }

    fn same_shape(one: &str, other: &str) -> bool {
        let enclosing = static_member_of("Total");
        read(one, &enclosing) == read(other, &enclosing)
    }

    #[test]
    fn test_java_parameter_names_are_dropped() {
        assert!(same_shape(
            "int shop.Total.total(int items, double discount)",
            "int shop.Total.subtotal(int values, double rate)",
        ));
    }

    #[test]
    fn test_java_type_parameter_names_are_dropped() {
        assert!(same_shape(
            "<T> T shop.Total.first(List<T> items)",
            "<E> E shop.Total.head(List<E> values)",
        ));
    }

    #[test]
    fn test_java_final_modifier_is_dropped() {
        assert!(same_shape(
            "int shop.Total.total(final int items)",
            "int shop.Total.total(int items)",
        ));
    }

    #[test]
    fn test_java_bound_order_is_dropped() {
        assert!(same_shape(
            "<T extends Priced & Named> T shop.Total.pick(T a)",
            "<T extends Named & Priced> T shop.Total.pick(T a)",
        ));
    }

    #[test]
    fn test_java_annotations_are_dropped() {
        assert!(same_shape(
            "int shop.Total.total(@NonNull List<@NonNull Item> items)",
            "int shop.Total.total(List<Item> items)",
        ));
    }

    #[test]
    fn test_java_different_parameter_types_differ() {
        assert!(!same_shape(
            "int shop.Total.total(int items)",
            "int shop.Total.total(long items)",
        ));
    }

    #[test]
    fn test_java_type_parameters_are_numbered_by_appearance_not_by_declaration() {
        // 宣言の順は違うが、引数に現れる順で付け替えると同じ形になる
        assert!(same_shape(
            "<K, V> V shop.Total.get(K key, V fallback)",
            "<V, K> K shop.Total.get(V key, K fallback)",
        ));
        assert!(!same_shape(
            "<K, V> V shop.Total.get(K key, V fallback)",
            "<K, V> K shop.Total.get(K key, V fallback)",
        ));
    }

    #[test]
    fn test_java_different_bounds_differ() {
        assert!(!same_shape(
            "<T extends Priced> T shop.Total.pick(T a)",
            "<T extends Named> T shop.Total.pick(T a)",
        ));
    }

    #[test]
    fn test_java_variadic_and_array_parameters_differ() {
        assert!(!same_shape(
            "int shop.Total.sum(int... values)",
            "int shop.Total.sum(int[] values)",
        ));
    }

    #[test]
    fn test_java_c_style_array_dimensions_equal_the_type_dimensions() {
        assert!(same_shape(
            "int shop.Total.sum(int values[])",
            "int shop.Total.sum(int[] values)",
        ));
        assert!(!same_shape(
            "int shop.Total.sum(int[][] values)",
            "int shop.Total.sum(int[] values)",
        ));
    }

    #[test]
    fn test_java_wildcards_are_kept() {
        assert!(!same_shape(
            "int shop.Total.total(List<? extends Item> items)",
            "int shop.Total.total(List<? super Item> items)",
        ));
        assert!(!same_shape(
            "int shop.Total.total(List<?> items)",
            "int shop.Total.total(List<Item> items)",
        ));
    }

    #[test]
    fn test_java_thrown_types_are_kept_but_their_order_is_dropped() {
        assert!(same_shape(
            "void shop.Total.load(int a) throws IOException, ParseException",
            "void shop.Total.load(int a) throws ParseException, IOException",
        ));
        assert!(!same_shape(
            "void shop.Total.load(int a) throws IOException",
            "void shop.Total.load(int a)",
        ));
    }

    #[test]
    fn test_java_instance_methods_of_different_types_differ() {
        let spelling = "int shop.Total.total(int items)";
        let total = read(spelling, &instance_member_of("Total"));
        let cart = read(spelling, &instance_member_of("Cart"));

        assert_ne!(total, cart);
        assert!(total.type_names().contains("Total"));
    }

    #[test]
    fn test_java_static_methods_of_different_types_are_the_same_shape() {
        let spelling = "int shop.Total.total(int items)";

        assert_eq!(
            read(spelling, &static_member_of("Total")),
            read(spelling, &static_member_of("Cart"))
        );
        assert!(
            read(spelling, &static_member_of("Total"))
                .type_names()
                .is_empty()
        );
    }

    #[test]
    fn test_java_constructor_and_static_factory_differ() {
        let enclosing = static_member_of("Total");
        let constructor = read("shop.Total.Total(int items)", &instance_member_of("Total"));
        let factory = read("Total shop.Total.of(int items)", &enclosing);

        assert_ne!(constructor, factory);
        assert!(constructor.type_names().contains("Total"));
    }

    #[test]
    fn test_java_type_names_exclude_method_type_parameters_and_primitives() {
        let callable = read(
            "<T extends Priced> java.util.List<T> shop.Total.sorted(T[] items, int limit)",
            &static_member_of("Total"),
        );

        assert_eq!(
            callable.type_names().iter().collect::<Vec<_>>(),
            vec!["Priced", "java.util.List"]
        );
    }

    #[test]
    fn test_java_spelling_without_parameter_list_is_unreadable() {
        assert_eq!(
            JavaCallable::from_spelling("int shop.Total.total", Some(&static_member_of("Total"))),
            None
        );
    }

    #[test]
    fn test_java_member_without_named_enclosing_type_is_unreadable() {
        assert_eq!(
            JavaCallable::from_spelling("int shop.Total.total(int items)", None),
            None
        );
    }

    /// ソースを読んで、最初のメソッド・コンストラクタのノードを渡す。
    fn with_first_member<T>(source: &str, read: impl Fn(Node<'_>) -> T) -> T {
        let tree = SyntaxTree::from_source(source, Grammar::Java).expect("木にできる");
        let member = tree
            .named_descendants()
            .into_iter()
            .find(|node| is_callable_kind(node.kind()))
            .expect("メソッドかコンストラクタがある");
        read(member)
    }

    fn reference_names_of(source: &str) -> Vec<String> {
        with_first_member(source, |member| {
            type_references_of(member, source)
                .iter()
                .map(|reference| reference.name().to_owned())
                .collect()
        })
    }

    #[test]
    fn test_java_source_references_collect_signature_type_names_once() {
        let source = "class Total {\n    \
            <T extends Priced> Map<Item, T> pick(Map<Item, T> items, Item[] more) throws IOException { return null; }\n}\n";

        // 並びは読む順で決まるので見ない。同じ名前が 2 度入らないことと、集める相手を見る
        let mut names = reference_names_of(source);
        names.sort();
        assert_eq!(names, vec!["IOException", "Item", "Map", "Priced", "Total"]);
    }

    #[test]
    fn test_java_source_references_of_a_static_method_leave_out_the_enclosing_type() {
        let source = "class Total {\n    static int total(Item item) { return 0; }\n}\n";

        assert_eq!(reference_names_of(source), vec!["Item"]);
    }

    #[test]
    fn test_java_enclosing_type_of_a_method_in_an_anonymous_class_is_absent() {
        let source = "class Total {\n    Object o = new Object() {\n        int f(int a) { return a; }\n    };\n}\n";

        assert_eq!(
            with_first_member(source, |member| enclosing_type_of(member, source)),
            None
        );
    }

    #[test]
    fn test_java_enclosing_type_says_whether_the_member_is_static() {
        let source = "class Total {\n    public static int total(int a) { return a; }\n}\n";

        assert_eq!(
            with_first_member(source, |member| enclosing_type_of(member, source)),
            Some(JavaEnclosingType::new("Total", true))
        );
    }
}
