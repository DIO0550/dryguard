//! sourcekit-lsp の hover が返す関数の綴りを、単一化の可否を比べられる形にする。
//!
//! **TypeScript の `type_structure`・Rust の `rust_callable` とは分ける。** 文法が別で、
//! 構文木のノードの種別で分岐する関数群に言語を混ぜると、片方の形を足すたびに他方へ漏れる。
//!
//! **ソースと hover の綴りを同じ読み手で読む。** 型名の綴り（パスの繋ぎ方）を 2 通りに
//! 書くと、ソースで集めた型名と hover の綴りに残った型名が食い違い、辿った記録が
//! 綴りで引けなくなる（`syntax::rust_callable` の `type_references_of` と同じ形）。
//!
//! **書かれ方の違いを落とす。** sourcekit-lsp 6.4.2 の hover は書かれたとおりに綴る
//! （`Int?` と `Optional<Int>` を寄せない。inline の境界だけ `where` へ寄せる）ので、
//! `T?` / `[T]` / `[K: V]` / `()` と、標準ライブラリを指すと確かめた
//! `Optional` / `Array` / `Dictionary` / `Void` を同じ形にする。
//!
//! **LSP を知らない。** 綴りを受け取って形を返すだけなので、`semantics` へ依存しない
//! （`rules/architecture.md`「依存方向のルール」）。型名ごとに宣言まで辿った答え
//! （[`SwiftTypeResolution`]）は呼び出し側が渡す。

use std::collections::BTreeSet;

use tree_sitter::Node;

use crate::source_position::SourcePosition;
use crate::syntax::tree::{Grammar, SyntaxTree, source_position_of};
use crate::syntax::type_reference::TypeReference;

/// hover の綴りを 1 つのメンバーとして読むための前置き。
///
/// **型の中に包む。** `init` / `deinit` は型の外では宣言として読めず、`class func` の
/// `class` は型の外では修飾子として残らない（tree-sitter-swift 0.7 で確かめた）。
const MEMBER_WRAPPER_START: &str = "class _ {\n";

/// 包みの後ろ。hover の綴りは本体を持たないので、空の本体を足して宣言として読む。
const MEMBER_WRAPPER_END: &str = " {}\n}\n";

/// チャンクになる宣言の種別。`syntax::chunk` の Swift のチャンクと同じ 3 つ。
const FUNCTION_KIND: &str = "function_declaration";
const INITIALIZER_KIND: &str = "init_declaration";
const DEINITIALIZER_KIND: &str = "deinit_declaration";

/// 型の宣言の、tree-sitter-swift のノードの種別。**`struct` / `class` / `actor` / `enum` /
/// `extension` はどれもこの 1 種別になる**（tree-sitter-swift 0.7 で確かめた。宣言の字句は
/// 種別ではなく子として残る）ので、種別を増やさずに全部を読む。
const TYPE_DECLARATION_KIND: &str = "class_declaration";

/// 型の本体。`struct` / `class` / `actor` / `extension` は `class_body`、`enum` は `enum_class_body`。
const TYPE_BODY_KINDS: [&str; 2] = ["class_body", "enum_class_body"];

/// 引数 1 つ。
const PARAMETER_KIND: &str = "parameter";

/// 引数の既定値。tree-sitter-swift では引数の**後ろの兄弟**として現れる。
const DEFAULT_VALUE_FIELD: &str = "default_value";

/// 戻り値の型。
const RETURN_TYPE_FIELD: &str = "return_type";

/// 関数自身が束縛した型変数のリストと、その 1 つ。
const TYPE_PARAMETERS_KIND: &str = "type_parameters";
const TYPE_PARAMETER_KIND: &str = "type_parameter";

/// `where` 節と、その中の述語。
const TYPE_CONSTRAINTS_KIND: &str = "type_constraints";
const INHERITANCE_CONSTRAINT_KIND: &str = "inheritance_constraint";
const EQUALITY_CONSTRAINT_KIND: &str = "equality_constraint";

/// 述語の左辺のフィールド。
const CONSTRAINED_TYPE_FIELD: &str = "constrained_type";

/// 宣言の前に付く修飾子（属性を含む）と、その中の可視性。
const MODIFIERS_KIND: &str = "modifiers";
const VISIBILITY_MODIFIER_KIND: &str = "visibility_modifier";

/// 引数の前に付く `inout` / `@escaping` / `@autoclosure`。
const PARAMETER_MODIFIERS_KIND: &str = "parameter_modifiers";

/// `throws` / `rethrows`。
const THROWS_KIND: &str = "throws";

/// `rethrows` の綴り。
const RETHROWS_TOKEN: &str = "rethrows";

/// 型に付いた属性（`@Sendable` / `@MainActor`）。
const TYPE_MODIFIERS_KIND: &str = "type_modifiers";

/// typed throws（`throws(MyError)`）。**読まない**（下の [`SwiftCallable::from_spelling`]）。
const THROWS_CLAUSE_KIND: &str = "throws_clause";

/// `init!` の `!`。
const BANG_KIND: &str = "bang";

/// 名前の識別子。引数の外部名・内部名、タプルの要素のラベル。
const SIMPLE_IDENTIFIER_KIND: &str = "simple_identifier";

/// 型の名前 1 区切り分。
const TYPE_IDENTIFIER_KIND: &str = "type_identifier";

/// 型引数のリスト。
const TYPE_ARGUMENTS_KIND: &str = "type_arguments";

/// タプル型の要素。
const TUPLE_TYPE_ITEM_KIND: &str = "tuple_type_item";

/// ラベルを書かないことを表す `_`。
const NO_LABEL: &str = "_";

/// `async` の字句。tree-sitter-swift では名前の無いノードとして残る。
const ASYNC_TOKEN: &str = "async";

/// 可変長の引数の字句（`v: Int...`）。
const VARIADIC_TOKEN: &str = "...";

/// 暗黙に unwrap される Optional の字句（`x: Int!`）。
const IMPLICITLY_UNWRAPPED_TOKEN: &str = "!";

/// failable な `init?` の字句。
const FAILABLE_TOKEN: &str = "?";

/// `func` / `init` / `deinit` の字句。ここより前が修飾子。
const DECLARATION_KEYWORDS: [&str; 3] = ["func", "init", "deinit"];

/// メンバーを囲む型を指す名前。
const SELF_TYPE: &str = "Self";

/// 標準ライブラリのモジュール名。`Swift.Int` のように前置きできる。
const STANDARD_LIBRARY_MODULE: &str = "Swift";

/// 綴りに省略形を持つ標準ライブラリの型と、省略形を開いた形。
///
/// **この一覧は増えない。** Swift の型の省略形は `T?` / `T!` / `[T]` / `[K: V]` / `()` で尽きる
/// （`T!` は `Optional` の宣言の書き方で、名前で書く形が無い）。
const OPTIONAL_NAME: &str = "Optional";
const ARRAY_NAME: &str = "Array";
const DICTIONARY_NAME: &str = "Dictionary";
const VOID_NAME: &str = "Void";

/// 省略形を持つ標準ライブラリの型の名前。
///
/// `semantics` が、この名前の型名を標準ライブラリの宣言まで辿れたかを答える。
pub(crate) const STANDARD_SUGAR_NAMES: [&str; 4] =
    [OPTIONAL_NAME, ARRAY_NAME, DICTIONARY_NAME, VOID_NAME];

/// `typealias` の宣言の種別。
const TYPEALIAS_KIND: &str = "typealias_declaration";

/// 型の宣言の字句のフィールド（`struct` / `class` / `extension` など）。
const DECLARATION_KIND_FIELD: &str = "declaration_kind";

/// `typealias` を開いてよい、囲む宣言の字句。**`extension` は入れない** — 拡張した型の
/// 型変数（`extension Array { typealias E = Element }`）を右辺が指しうる。
const OPENABLE_ENCLOSING_KINDS: [&str; 4] = ["struct", "class", "enum", "actor"];

/// `typealias` を開いてよい、宣言の祖先のノードの種別。**許可リスト** — 一覧に無い祖先
/// （`protocol` / 関数の本体など。型変数や関連型を右辺が指しうる）の中の宣言は開かない
/// （偽陰性側。`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。
const OPENABLE_ANCESTOR_KINDS: [&str; 4] = [
    "source_file",
    TYPE_DECLARATION_KIND,
    TYPE_BODY_KINDS[0],
    TYPE_BODY_KINDS[1],
];

/// 型名 1 つを宣言まで辿った答え。[`SwiftCallable::from_spelling`] がその名前の位置に置く形を決める。
///
/// **辿った型名は綴りではなく宣言の場所で比べる**（`rules/naming.md` の `declared type identity`）。
/// `typealias` の右辺の型名は**宣言側のファイルの綴り**で、使用側の同じ綴りが同じ型とは限らない。
/// 右辺と直接書いた型名を同じ形で比べるには、どちらも宣言の場所へ寄せるしかない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwiftTypeResolution {
    /// 宣言まで辿れた型。`identity` は宣言の場所から作った綴り、`standard` はその宣言が
    /// 標準ライブラリにあるか（省略形と重ねてよいか）。
    Declared { identity: String, standard: bool },
    /// 型引数を取らない `typealias` を開いた右辺。
    Opened(SwiftOpenedType),
}

/// `typealias` を開いた右辺の型。右辺の型名は宣言の場所へ置き換えてある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SwiftOpenedType(SwiftType);

impl SwiftOpenedType {
    /// 型を形作るノードの数。展開した右辺の大きさの上限に使う
    /// （`typealias B = (A, A)` の連鎖は段ごとに倍になる）。
    pub(crate) fn size(&self) -> usize {
        self.0.size()
    }
}

/// 宣言側のソースに書かれた、型引数を取らない `typealias` 1 つ。
///
/// **右辺の型名は宣言側の位置から辿る。** 使用側の同じ綴りの型と同じとは限らない
/// （`rules/architecture.md`「Rust の引数のないエイリアスの右辺の型名は…」と同じ線）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SwiftAliasSource {
    right: String,
    references: Vec<TypeReference>,
}

impl SwiftAliasSource {
    /// `source` の中で、名前が `position` にある `typealias` を読む。
    ///
    /// `None`: その位置に `typealias` の名前が無い、型引数を取る、右辺に属性が付く・
    /// 読めない構文、右辺に `Self` がある、**囲む宣言が型変数を持ちうる**（型引数を持つ型・
    /// `extension` / `protocol` / 関数の中）。最後のものは型引数を取らない宣言でも、右辺が囲む型の
    /// 型変数を指すと `Box<Int>.Item` と `Box<String>.Item` が同じ型に開く（偽陽性）。
    pub(crate) fn from_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Swift).ok()?;
        let declaration = tree.named_descendants().into_iter().find(|node| {
            node.kind() == TYPEALIAS_KIND
                && node
                    .child_by_field_name("name")
                    .and_then(|name| source_position_of(name, source))
                    == Some(position)
        })?;
        if declaration.has_error()
            || !children_of_kind(declaration, TYPE_PARAMETERS_KIND).is_empty()
            || !is_openable_context(declaration, source)
        {
            return None;
        }
        let value = alias_value_of(declaration)?;

        let unresolved = |_: &str| None;
        let mut reader = SwiftSpelling::new(source, &unresolved);
        reader.recording = true;
        reader.type_of(value)?;
        if reader.refers_to_self {
            return None;
        }
        Some(Self {
            right: source.get(value.byte_range())?.to_owned(),
            references: reader.references,
        })
    }

    /// 右辺に書かれた型名と、宣言側のソースでの位置。**名前ごとに最初の 1 つ。**
    pub(crate) fn references(&self) -> &[TypeReference] {
        &self.references
    }

    /// 右辺の型名を `resolve` で引いて、右辺を型にする。読めなければ `None`。
    ///
    /// `resolve` は右辺の型名を**宣言側の位置から**辿った答え。省略形を開くかもそれで決まる。
    pub(crate) fn opened_with(
        &self,
        resolve: &dyn Fn(&str) -> Option<SwiftTypeResolution>,
    ) -> Option<SwiftOpenedType> {
        let wrapped = format!("typealias _ = {}\n", self.right);
        let tree = SyntaxTree::from_source(&wrapped, Grammar::Swift).ok()?;
        if tree.has_error() {
            return None;
        }
        let declaration = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == TYPEALIAS_KIND)?;
        let value = alias_value_of(declaration)?;
        let mut reader = SwiftSpelling::new(&wrapped, resolve);
        let opened = reader.type_of(value)?;
        if reader.refers_to_self || reader.has_unopenable_alias {
            return None;
        }
        Some(SwiftOpenedType(opened))
    }
}

/// `typealias` の右辺の型のノード。属性（`@Sendable`）が付く・型が 1 つでなければ `None`。
///
/// **フィールドではなく、名前より後ろの子で探す。** tree-sitter-swift 0.7.4 の `node-types.json` は
/// 右辺を `value` のフィールドと書くが、構文木はそのフィールドを付けない（確かめた）。
fn alias_value_of(declaration: Node<'_>) -> Option<Node<'_>> {
    let name = declaration.child_by_field_name("name")?;
    let mut cursor = declaration.walk();
    let after_name: Vec<Node<'_>> = declaration
        .named_children(&mut cursor)
        .filter(|child| child.start_byte() >= name.end_byte())
        .collect();
    match after_name.as_slice() {
        [value] if is_type_kind(value.kind()) => Some(*value),
        _ => None,
    }
}

/// その `typealias` の祖先が、型変数を持ちえない宣言だけか。
fn is_openable_context(declaration: Node<'_>, source: &str) -> bool {
    let mut current = declaration.parent();
    while let Some(ancestor) = current {
        if !OPENABLE_ANCESTOR_KINDS.contains(&ancestor.kind()) {
            return false;
        }
        if ancestor.kind() == TYPE_DECLARATION_KIND {
            let kind = ancestor
                .child_by_field_name(DECLARATION_KIND_FIELD)
                .and_then(|kind| source.get(kind.byte_range()));
            let generic = !children_of_kind(ancestor, TYPE_PARAMETERS_KIND).is_empty();
            let openable = kind.is_some_and(|kind| OPENABLE_ENCLOSING_KINDS.contains(&kind));
            if generic || !openable {
                return false;
            }
        }
        current = ancestor.parent();
    }
    true
}

/// Swift の関数・`init`・`deinit` 1 つ分の、単一化の可否を比べられる形。
///
/// 関数名・引数の内部名・可視性を落とし、型変数を出現順に付け替える
/// （引数 → 戻り値 → 境界の順）。
///
/// **引数のラベルは残す。** `func move(to x: Int)` の `to` は呼び出しの綴り
/// （`move(to: 1)`）の一部で、ラベルだけ違う 2 つは呼び出し側で差し替えられない。
/// 内部名は本体の中でだけ意味を持つので落とす（`rules/naming.md`「`signature text` と
/// `type signature` を混ぜない」）。
#[derive(Debug, Clone)]
pub(crate) struct SwiftCallable {
    kind: SwiftCallableKind,
    /// `static` / `class` / `mutating` / `@MainActor` など。**並びは落とす**（書く順は自由）。
    ///
    /// **可視性だけは落とす。** 呼び出せる範囲を変えるが、呼び出しの形は変えない
    /// （Rust の `pub` を落とすのと同じ）。
    modifiers: BTreeSet<String>,
    parameters: Vec<SwiftParameter>,
    effects: SwiftEffects,
    /// 戻り値の型。関数が省けば `()`、`init` は囲む型（failable なら Optional）。
    value_type: SwiftType,
    /// 型のメンバーなら、直接囲む型（`init` は戻り値の側に入れるので `None`）。
    ///
    /// **hover の綴りにレシーバは出ない**（`func spend(_ amount: Int) throws -> Int`）。
    /// 入れないと、別々の型の同じ形のメソッドと、メソッドと自由関数が重なる（偽陽性）。
    /// Rust のメソッドが `self` を impl の対象型へ置き換えるのと同じ線。
    receiver: Option<SwiftType>,
    /// 直接囲む宣言の `where` 節。**関数自身の境界（`constraints`）と混ぜない** — 左辺は
    /// レシーバの型変数（とその関連型）で、辿らずにレシーバからの相対の綴りで持つ。
    /// 綴りで比べてよいのは、レシーバを宣言まで辿ってあり、同じ宣言なら同じ綴りが同じ型変数を指すから。
    ///
    /// **hover は `where` を綴らない**（sourcekit-lsp 6.4.2 で実測）ので、ソースの宣言から運ぶ。
    /// 入れないと、`Element == Int` と `Element == Double` の拡張のメソッドが重なる（偽陽性）。
    receiver_constraints: BTreeSet<ReceiverConstraint>,
    /// 関数が束縛した型変数の数。**使われない型変数も数える**（`f<T>()` の呼び方が変わる）。
    type_parameter_count: usize,
    /// 境界。**左辺・関係・右辺の組の集合**で持つ（`rules/naming.md` の `trait bound`）。
    ///
    /// inline（`<T: P>`）と `where` の書き分け・`P & Q` の分け方は、どれも同じ要求の
    /// 書き分けなので、組に割ってから集合にする。
    constraints: BTreeSet<SwiftConstraint>,
    /// 比較に残る綴りに現れた型名。名前順。**パスで書かれた型はパスごと 1 つの名前。**
    /// 辿った記録・開けなかった理由を引くのに使い、**比較には使わない**（形は宣言の場所で持つ。
    /// [`PartialEq`] の実装）。
    ///
    /// 束縛した型変数・標準ライブラリを指すと確かめた省略形の名前・`Self` は入らない。
    type_names: BTreeSet<String>,
    /// `Self` が残るか。囲む型で決まるので、綴りだけで比べない。
    refers_to_self: bool,
    /// 開いた `typealias` を、型引数の付いたパスに当てることになったか。
    /// 型引数を取らない宣言にだけ右辺を持たせるので、当たれば型引数を落とすことになる。
    has_unopenable_alias: bool,
}

/// **比べるのは形だけ。** 型名の綴り（`type_names`）と、測れない側へ倒すための印は比べない。
/// 辿った型名は宣言の場所で形に入っているので、`Inner` と `Holder.Inner`、`typealias` の名前と
/// その右辺のように、**同じ型を別の綴りで書いた 2 つ**を綴りで分けない。
impl PartialEq for SwiftCallable {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.modifiers == other.modifiers
            && self.parameters == other.parameters
            && self.effects == other.effects
            && self.value_type == other.value_type
            && self.receiver == other.receiver
            && self.receiver_constraints == other.receiver_constraints
            && self.type_parameter_count == other.type_parameter_count
            && self.constraints == other.constraints
    }
}

impl Eq for SwiftCallable {}

/// 呼び出しの仕方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwiftCallableKind {
    /// `f(a)` / `T.f(a)` / `value.f(a)`。
    Function,
    /// `T(a)`。**関数と重ねない** — 戻り値の型が同じ関数とは呼び方が違う
    /// （TypeScript の `SignatureKind::Construct` と同じ線）。
    Initializer,
    /// 呼べない。比べても、構造が同じなら同じ答えになるだけ。
    Deinitializer,
}

/// 引数 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SwiftParameter {
    /// 呼び出しに書くラベル。`_` なら `None`。
    label: Option<String>,
    /// `inout` / `@escaping` / `@autoclosure`。並びは落とす。
    modifiers: BTreeSet<String>,
    /// `v: Int...`。
    variadic: bool,
    /// 既定値を書いたか。**値は落とす**（型ではない）が、**有無は残す** — 既定値のある
    /// 引数は呼び出しで省けるので、省いた呼び出しを持つ側は差し替えられない。
    defaulted: bool,
    value: SwiftType,
}

/// 効果。
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
struct SwiftEffects {
    asynchronous: bool,
    /// 投げなければ `None`。
    throwing: Option<Throwing>,
}

/// 投げ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Throwing {
    /// `throws`。
    Throws,
    /// `rethrows`。引数のクロージャが投げたときだけ投げるので、`throws` と重ねない。
    Rethrows,
}

/// メンバーを直接囲む宣言の、型と `where` 節のソースの綴り
/// （`Wallet` / `Shop.Wallet` / `Array<Int>` / `[Int]` / `Array where Element == Int`）。
///
/// **名前だけで持たない。** `extension Array where Element == Int` のメンバーのレシーバは
/// `Array<Int>` で、名前（`Array`）だけで持つと `Element == Double` の拡張のメンバーと重なる
/// （偽陽性）。読むのは hover の綴りと同じ読み手（[`SwiftCallable::from_spelling`]）で、
/// 型名の綴りが揃う（Rust の `impl header` と同じ形）。
///
/// **型の宣言の型変数（`struct Box<T>` の `<T>`）は持たない。** レシーバは名前（`Box`）で比べる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwiftEnclosingType {
    header: String,
}

impl SwiftEnclosingType {
    /// 型と `where` 節の綴りから作る。読めるかは [`SwiftCallable::from_spelling`] が確かめる。
    pub(crate) fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
        }
    }
}

/// 囲む宣言の `where` の述語 1 つ。左辺はレシーバからの相対のパス（`Element` / `Element.Index`）。
type ReceiverConstraint = (Vec<String>, ConstraintRelation, SwiftType);

/// 囲む宣言を、レシーバとして読んだ形。
struct EnclosingReceiver {
    value: SwiftType,
    constraints: BTreeSet<ReceiverConstraint>,
    type_names: BTreeSet<String>,
    refers_to_self: bool,
    has_unopenable_alias: bool,
}

impl EnclosingReceiver {
    /// 囲む型の綴りを `extension` の宣言として読む。読めなければ `None`。
    fn from_header(
        header: &str,
        resolve: &dyn Fn(&str) -> Option<SwiftTypeResolution>,
    ) -> Option<Self> {
        let wrapped = format!("extension {header} {{}}\n");
        let tree = SyntaxTree::from_source(&wrapped, Grammar::Swift).ok()?;
        if tree.has_error() {
            return None;
        }
        let declaration = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == TYPE_DECLARATION_KIND)?;
        let mut reader = SwiftSpelling::new(&wrapped, resolve);
        let value = reader.receiver_type_of(declaration)?;
        let constraints = reader.receiver_constraints_of(declaration)?;
        Some(Self {
            value,
            constraints,
            type_names: reader.type_names,
            refers_to_self: reader.refers_to_self,
            has_unopenable_alias: reader.has_unopenable_alias,
        })
    }
}

/// 境界 1 つの関係。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ConstraintRelation {
    /// `T: P`。
    Conforms,
    /// `T == U`。
    Equals,
}

/// 境界 1 つ。左辺・関係・右辺。
type SwiftConstraint = (SwiftType, ConstraintRelation, SwiftType);

/// 型 1 つ分の形。**書かれ方の違い（省略形・冗長な括弧）を落としてある。**
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SwiftType {
    /// 名前で書かれた型。パスの区切りごとに型引数を持つ（`Outer<Int>.Inner`）。
    Named(Vec<(String, Vec<SwiftType>)>),
    /// 関数が束縛した型変数。番号は出現順。
    Parameter(usize),
    /// 型変数の関連型（`T.Element`）。
    Member(usize, Vec<String>),
    /// `T?` / `Optional<T>`。
    Optional(Box<SwiftType>),
    /// `T!`。**`T?` と重ねない** — 受け取る側で暗黙に unwrap されるので、呼び出し側の書き方が変わる。
    ImplicitlyUnwrapped(Box<SwiftType>),
    /// `[T]` / `Array<T>`。
    Array(Box<SwiftType>),
    /// `[K: V]` / `Dictionary<K, V>`。
    Dictionary(Box<SwiftType>, Box<SwiftType>),
    /// タプル。**ラベルは残す**（`(a: Int, b: Int)` の要素は `.a` で読む）。空は `()` = `Void`。
    Tuple(Vec<(Option<String>, SwiftType)>),
    /// 関数型。**引数の名前は落とす**（関数型はラベルを持てない）。
    Function {
        parameters: Vec<(BTreeSet<String>, bool, SwiftType)>,
        effects: SwiftEffects,
        value: Box<SwiftType>,
    },
    /// `some P`。
    Opaque(Box<SwiftType>),
    /// `any P`。
    Existential(Box<SwiftType>),
    /// `P & Q`。**並びは落とす**。
    Composition(BTreeSet<SwiftType>),
}

impl SwiftType {
    /// この型を形作るノードの数（自分を含む）。
    fn size(&self) -> usize {
        let inner: usize = match self {
            Self::Named(path) => path
                .iter()
                .flat_map(|(_, arguments)| arguments)
                .map(Self::size)
                .sum(),
            Self::Parameter(_) | Self::Member(..) => 0,
            Self::Optional(wrapped)
            | Self::ImplicitlyUnwrapped(wrapped)
            | Self::Array(wrapped)
            | Self::Opaque(wrapped)
            | Self::Existential(wrapped) => wrapped.size(),
            Self::Dictionary(key, value) => key.size() + value.size(),
            Self::Tuple(elements) => elements.iter().map(|(_, element)| element.size()).sum(),
            Self::Function {
                parameters, value, ..
            } => {
                parameters
                    .iter()
                    .map(|(_, _, parameter)| parameter.size())
                    .sum::<usize>()
                    + value.size()
            }
            Self::Composition(members) => members.iter().map(Self::size).sum(),
        };
        inner + 1
    }
}

impl SwiftCallable {
    /// hover が返した綴りから読む。宣言として読めなければ `None`。
    ///
    /// `resolve` は型名ごとに、宣言まで辿った答えを返す。辿れた名前は宣言の場所の綴りに、
    /// 開いた `typealias` は右辺に置き換える。省略形を持つ名前（[`STANDARD_SUGAR_NAMES`]）は
    /// **標準ライブラリの宣言を指すと確かめられたとき**だけ省略形と重ねる。確かめられなければ、
    /// その名前は利用者の型として型名に残る（`Optional` を宣言し直したモジュールもありうる）。
    /// `enclosing_type` は、そのメンバーを直接囲む型（`Chunk::swift_enclosing_type`）。
    /// 型の外の関数では `None`。
    ///
    /// **読めない形は `None`**（偽陰性側）: typed throws（`throws(E)`）、未知の型の構文、
    /// 囲む型の分からない `init` / `deinit`、囲む型の綴りを読めないメンバー。
    pub(crate) fn from_spelling(
        spelling: &str,
        resolve: &dyn Fn(&str) -> Option<SwiftTypeResolution>,
        enclosing_type: Option<&SwiftEnclosingType>,
    ) -> Option<Self> {
        let enclosing = match enclosing_type {
            None => None,
            Some(enclosing) => Some(EnclosingReceiver::from_header(&enclosing.header, resolve)?),
        };
        let wrapped = format!("{MEMBER_WRAPPER_START}{spelling}{MEMBER_WRAPPER_END}");
        let tree = SyntaxTree::from_source(&wrapped, Grammar::Swift).ok()?;
        if tree.has_error() {
            return None;
        }
        let declaration = tree
            .named_descendants()
            .into_iter()
            .find(|node| is_callable_kind(node.kind()))?;

        let mut reader = SwiftSpelling::new(&wrapped, resolve);
        let mut callable = reader.callable_of(declaration)?;
        let mut type_names = reader.type_names;
        let mut refers_to_self = reader.refers_to_self;
        let mut has_unopenable_alias = reader.has_unopenable_alias;
        let enclosing = enclosing.map(|receiver| {
            type_names.extend(receiver.type_names);
            refers_to_self |= receiver.refers_to_self;
            has_unopenable_alias |= receiver.has_unopenable_alias;
            callable.receiver_constraints = receiver.constraints;
            receiver.value
        });
        match callable.kind {
            SwiftCallableKind::Function => callable.receiver = enclosing,
            SwiftCallableKind::Deinitializer => callable.receiver = Some(enclosing?),
            SwiftCallableKind::Initializer => {
                let initialized = enclosing?;
                callable.value_type = match failability_of(declaration) {
                    Failability::Never => initialized,
                    Failability::Optional => SwiftType::Optional(Box::new(initialized)),
                    Failability::ImplicitlyUnwrapped => {
                        SwiftType::ImplicitlyUnwrapped(Box::new(initialized))
                    }
                };
            }
        }
        callable.type_names = type_names;
        callable.refers_to_self = refers_to_self;
        callable.has_unopenable_alias = has_unopenable_alias;
        Some(callable)
    }

    /// 比較に残る綴りに現れた型名。名前順。
    pub(crate) fn type_names(&self) -> &BTreeSet<String> {
        &self.type_names
    }

    /// `Self` が残るか。
    pub(crate) fn refers_to_self(&self) -> bool {
        self.refers_to_self
    }

    /// 開いた `typealias` を、型引数の付いたパスに当てることになったか。
    pub(crate) fn has_unopenable_alias(&self) -> bool {
        self.has_unopenable_alias
    }
}

/// `init` が失敗しうるか。
enum Failability {
    Never,
    Optional,
    ImplicitlyUnwrapped,
}

fn failability_of(declaration: Node<'_>) -> Failability {
    let mut cursor = declaration.walk();
    for child in declaration.children(&mut cursor) {
        if child.kind() == BANG_KIND {
            return Failability::ImplicitlyUnwrapped;
        }
        if !child.is_named() && child.kind() == FAILABLE_TOKEN {
            return Failability::Optional;
        }
        if child.kind() == PARAMETER_KIND {
            break;
        }
    }
    Failability::Never
}

fn is_callable_kind(kind: &str) -> bool {
    [FUNCTION_KIND, INITIALIZER_KIND, DEINITIALIZER_KIND].contains(&kind)
}

/// Swift のチャンクのシグネチャに書かれた型名と、その位置。**名前ごとに最初の 1 つ。**
///
/// `function` はチャンクのノード（関数・`init`・`deinit`）、`source` はそれを含むファイル全体。
/// **hover の綴りを読むのと同じ読み手で読む**ので、型名の綴りが揃う。
/// 関数自身が束縛した型変数・`Self` は集めない（辿る相手が居ない / 囲む型で決まる）。
/// **標準ライブラリの省略形の名前（`Optional` など）も集める** — 標準ライブラリを
/// 指すかは辿ってみるまで分からない。
pub(crate) fn type_references_of(function: Node<'_>, source: &str) -> Vec<TypeReference> {
    let unresolved = |_: &str| None;
    let mut reader = SwiftSpelling::new(source, &unresolved);
    reader.recording = true;
    if reader.callable_of(function).is_none() {
        return Vec::new();
    }
    reader.references
}

/// そのメンバーを直接囲む型の、型と `where` 節の綴り。型のメンバーでなければ `None`。
///
/// 型引数を書いた `extension`（`Array<Int>` / `[Int]`）は型引数ごと、`where` の付いた宣言は
/// `where` 節ごと運ぶ。
pub(crate) fn enclosing_type_of(function: Node<'_>, source: &str) -> Option<SwiftEnclosingType> {
    let declaration = enclosing_declaration_of(function)?;
    let name = declaration.child_by_field_name("name")?;
    let mut header = source.get(name.byte_range())?.to_owned();
    for clause in children_of_kind(declaration, TYPE_CONSTRAINTS_KIND) {
        header.push(' ');
        header.push_str(source.get(clause.byte_range())?);
    }
    Some(SwiftEnclosingType::new(header))
}

/// メンバーを直接囲む型の宣言（`extension` を含む）。
///
/// 関数の本体の中の関数は、型の本体の直下に無いので囲む型を持たない。
fn enclosing_declaration_of(function: Node<'_>) -> Option<Node<'_>> {
    let body = function.parent()?;
    if !TYPE_BODY_KINDS.contains(&body.kind()) {
        return None;
    }
    body.parent()
}

/// そのメンバーを直接囲む型の綴りに書かれた型名と、その位置。**名前ごとに最初の 1 つ。**
/// 型のメンバーでなければ空。
///
/// **レシーバの名前に加えて、型引数と `where` の右辺の型名も集める。** どれも
/// [`SwiftCallable::from_spelling`] が比較に残す型名になる。`where` の左辺はレシーバの
/// 型変数で、集めない（sourcekit-lsp 6.4.2 の definition は宣言ではない位置を返す）。
///
/// **`extension` も囲む型に数える。** 名前は `extension` に書かれたパスの綴りで、
/// 型の宣言のほうは名前の識別子 1 つ。どちらの位置から辿っても型の宣言へ届く。
pub(crate) fn enclosing_type_references_of(function: Node<'_>, source: &str) -> Vec<TypeReference> {
    let Some(declaration) = enclosing_declaration_of(function) else {
        return Vec::new();
    };
    let unresolved = |_: &str| None;
    let mut reader = SwiftSpelling::new(source, &unresolved);
    reader.recording = true;
    let read = reader
        .receiver_type_of(declaration)
        .and_then(|_| reader.receiver_constraints_of(declaration));
    if read.is_none() {
        return Vec::new();
    }
    reader.references
}

/// 1 つの宣言の綴りを読み進める途中の状態。
struct SwiftSpelling<'source> {
    source: &'source str,
    resolve: &'source dyn Fn(&str) -> Option<SwiftTypeResolution>,
    /// 関数が束縛した型変数の名前。宣言された順。
    declared: Vec<String>,
    /// 付け替えた型変数の名前。**番号の順**。
    numbered: Vec<String>,
    type_names: BTreeSet<String>,
    refers_to_self: bool,
    has_unopenable_alias: bool,
    /// ソースを読むときだけ、型名の位置を残す。
    recording: bool,
    references: Vec<TypeReference>,
}

impl<'source> SwiftSpelling<'source> {
    fn new(
        source: &'source str,
        resolve: &'source dyn Fn(&str) -> Option<SwiftTypeResolution>,
    ) -> Self {
        Self {
            source,
            resolve,
            declared: Vec::new(),
            numbered: Vec::new(),
            type_names: BTreeSet::new(),
            refers_to_self: false,
            has_unopenable_alias: false,
            recording: false,
            references: Vec::new(),
        }
    }

    fn text(&self, node: Node<'_>) -> Option<&'source str> {
        self.source.get(node.byte_range())
    }

    fn callable_of(&mut self, declaration: Node<'_>) -> Option<SwiftCallable> {
        let kind = match declaration.kind() {
            FUNCTION_KIND => SwiftCallableKind::Function,
            INITIALIZER_KIND => SwiftCallableKind::Initializer,
            DEINITIALIZER_KIND => SwiftCallableKind::Deinitializer,
            _ => return None,
        };

        let children = all_children_of(declaration);
        if children
            .iter()
            .any(|child| child.kind() == THROWS_CLAUSE_KIND)
        {
            return None;
        }

        self.declared = children
            .iter()
            .filter(|child| child.kind() == TYPE_PARAMETERS_KIND)
            .flat_map(|list| children_of_kind(*list, TYPE_PARAMETER_KIND))
            .filter_map(|parameter| {
                children_of_kind(parameter, TYPE_IDENTIFIER_KIND)
                    .first()
                    .and_then(|name| self.text(*name))
                    .map(str::to_owned)
            })
            .collect();

        let modifiers = self.modifiers_of(&children)?;
        let effects = effects_of(&children, self.source);

        let mut parameters = Vec::new();
        for child in &children {
            if child.kind() == PARAMETER_KIND {
                parameters.push(self.parameter_of(*child)?);
            } else if is_default_value(declaration, *child)
                && let Some(last) = parameters.last_mut()
            {
                last.defaulted = true;
            }
        }

        let value_type = self.value_type_of(declaration)?;

        let mut constraints = BTreeSet::new();
        for list in children
            .iter()
            .filter(|child| child.kind() == TYPE_PARAMETERS_KIND)
        {
            for parameter in children_of_kind(*list, TYPE_PARAMETER_KIND) {
                self.push_inline_constraints(parameter, &mut constraints)?;
            }
        }
        for list in children
            .iter()
            .filter(|child| child.kind() == TYPE_CONSTRAINTS_KIND)
        {
            for predicate in descendants_of_kinds(
                *list,
                &[INHERITANCE_CONSTRAINT_KIND, EQUALITY_CONSTRAINT_KIND],
            ) {
                self.push_constraint(predicate, &mut constraints)?;
            }
        }

        // 使われない型変数も数える（`f<T>()` の呼び方が変わる）
        let type_parameter_count = self.declared.len();

        Some(SwiftCallable {
            kind,
            modifiers,
            parameters,
            effects,
            value_type,
            receiver: None,
            receiver_constraints: BTreeSet::new(),
            type_parameter_count,
            constraints,
            type_names: BTreeSet::new(),
            refers_to_self: false,
            has_unopenable_alias: false,
        })
    }

    /// 戻り値の型。省けば `()`。
    ///
    /// **`-> Int!` の `!` は戻り値のフィールドに付く別の子になる**（tree-sitter-swift 0.7）。
    /// 先頭の 1 つだけを読むと `-> Int` と重なる。
    /// **型に付いた属性（`-> @Sendable () -> Void`）は読まない**（`None`）。
    fn value_type_of(&mut self, declaration: Node<'_>) -> Option<SwiftType> {
        let Some(written) = declaration.child_by_field_name(RETURN_TYPE_FIELD) else {
            return Some(SwiftType::Tuple(Vec::new()));
        };
        if written.kind() == TYPE_MODIFIERS_KIND {
            return None;
        }
        let value = self.type_of(written)?;
        let unwrapped = written.next_sibling().is_some_and(|next| {
            !next.is_named() && self.text(next) == Some(IMPLICITLY_UNWRAPPED_TOKEN)
        });
        if unwrapped {
            return Some(SwiftType::ImplicitlyUnwrapped(Box::new(value)));
        }
        Some(value)
    }

    /// 宣言の字句（`func` / `init` / `deinit`）より前の修飾子。可視性は落とす。
    fn modifiers_of(&self, children: &[Node<'_>]) -> Option<BTreeSet<String>> {
        let mut modifiers = BTreeSet::new();
        for child in children {
            let text = self.text(*child)?;
            if !child.is_named() && DECLARATION_KEYWORDS.contains(&text) {
                break;
            }
            if child.kind() == MODIFIERS_KIND {
                let mut cursor = child.walk();
                for modifier in child.named_children(&mut cursor) {
                    if modifier.kind() == VISIBILITY_MODIFIER_KIND {
                        continue;
                    }
                    modifiers.insert(collapsed(self.text(modifier)?));
                }
            } else if !child.is_named() {
                // `class func` の `class` は名前の無い字句として残る
                modifiers.insert(text.to_owned());
            }
        }
        Some(modifiers)
    }

    fn parameter_of(&mut self, parameter: Node<'_>) -> Option<SwiftParameter> {
        let children = all_children_of(parameter);
        let names: Vec<&str> = children
            .iter()
            .filter(|child| child.kind() == SIMPLE_IDENTIFIER_KIND)
            .map(|child| self.text(*child))
            .collect::<Option<_>>()?;
        // 外部名を書けば 1 つ目、書かなければ内部名がそのままラベル
        let label = match names.first() {
            Some(&NO_LABEL) | None => None,
            Some(label) => Some((*label).to_owned()),
        };
        let modifiers = self.parameter_modifiers_of(&children)?;
        let written = children
            .iter()
            .rev()
            .find(|child| child.is_named() && is_type_kind(child.kind()))?;
        let mut value = self.type_of(*written)?;

        let tokens: Vec<&str> = children
            .iter()
            .filter(|child| !child.is_named())
            .filter_map(|child| self.text(*child))
            .collect();
        if tokens.contains(&IMPLICITLY_UNWRAPPED_TOKEN) {
            value = SwiftType::ImplicitlyUnwrapped(Box::new(value));
        }

        Some(SwiftParameter {
            label,
            modifiers,
            variadic: tokens.contains(&VARIADIC_TOKEN),
            defaulted: false,
            value,
        })
    }

    /// 引数・関数型の引数に付いた修飾子と属性。
    ///
    /// **`type_modifiers` も読む。** tree-sitter-swift は `@MainActor` / `@Sendable` と、
    /// それらの後ろに書いた `@escaping` を `parameter_modifiers` ではなく `type_modifiers` に置く。
    /// 読み落とすと `@MainActor @escaping () -> Void` が `() -> Void` と重なる。
    fn parameter_modifiers_of(&self, children: &[Node<'_>]) -> Option<BTreeSet<String>> {
        let mut modifiers = BTreeSet::new();
        for list in children.iter().filter(|child| {
            child.kind() == PARAMETER_MODIFIERS_KIND || child.kind() == TYPE_MODIFIERS_KIND
        }) {
            let mut cursor = list.walk();
            for modifier in list.named_children(&mut cursor) {
                modifiers.insert(collapsed(self.text(modifier)?));
            }
        }
        Some(modifiers)
    }

    /// `<T: P & Q>` の inline の境界。
    fn push_inline_constraints(
        &mut self,
        parameter: Node<'_>,
        constraints: &mut BTreeSet<SwiftConstraint>,
    ) -> Option<()> {
        let name = children_of_kind(parameter, TYPE_IDENTIFIER_KIND)
            .first()
            .and_then(|name| self.text(*name))?;
        let mut cursor = parameter.walk();
        let bounds: Vec<Node<'_>> = parameter
            .named_children(&mut cursor)
            .filter(|child| is_type_kind(child.kind()))
            .collect();
        for bound in bounds {
            let left = self.parameter_type_of(name);
            let right = self.type_of(bound)?;
            push_split_constraint(constraints, left, ConstraintRelation::Conforms, right);
        }
        Some(())
    }

    /// `where` 節の述語 1 つ。
    fn push_constraint(
        &mut self,
        predicate: Node<'_>,
        constraints: &mut BTreeSet<SwiftConstraint>,
    ) -> Option<()> {
        let (path, relation, right) = self.predicate_parts_of(predicate)?;
        let left = self.path_type_of(
            path.into_iter()
                .map(|segment| (segment, Vec::new()))
                .collect(),
            None,
        );
        push_split_constraint(constraints, left, relation, right);
        Some(())
    }

    /// 囲む宣言の名前のノードを、レシーバの型として読む。
    ///
    /// 型の宣言の名前は識別子 1 つ（`struct Wallet`）、`extension` の名前は型
    /// （`Shop.Wallet` / `Array<Int>` / `[Int]`）。
    fn receiver_type_of(&mut self, declaration: Node<'_>) -> Option<SwiftType> {
        let name = declaration.child_by_field_name("name")?;
        if name.kind() == TYPE_IDENTIFIER_KIND {
            let path = vec![(self.text(name)?.to_owned(), Vec::new())];
            return Some(self.path_type_of(path, Some(name)));
        }
        self.type_of(name)
    }

    /// 囲む宣言の `where` 節の述語。読めない述語があれば `None`。
    fn receiver_constraints_of(
        &mut self,
        declaration: Node<'_>,
    ) -> Option<BTreeSet<ReceiverConstraint>> {
        let mut constraints = BTreeSet::new();
        for list in children_of_kind(declaration, TYPE_CONSTRAINTS_KIND) {
            for predicate in descendants_of_kinds(
                list,
                &[INHERITANCE_CONSTRAINT_KIND, EQUALITY_CONSTRAINT_KIND],
            ) {
                let (left, relation, right) = self.predicate_parts_of(predicate)?;
                push_split_constraint(&mut constraints, left, relation, right);
            }
        }
        Some(constraints)
    }

    /// 述語 1 つの左辺のパス（`Element.Index` なら 2 区切り）・関係・右辺の型。
    fn predicate_parts_of(
        &mut self,
        predicate: Node<'_>,
    ) -> Option<(Vec<String>, ConstraintRelation, SwiftType)> {
        let relation = match predicate.kind() {
            INHERITANCE_CONSTRAINT_KIND => ConstraintRelation::Conforms,
            _ => ConstraintRelation::Equals,
        };
        let constrained = predicate.child_by_field_name(CONSTRAINED_TYPE_FIELD)?;
        let mut cursor = predicate.walk();
        let right_node = predicate
            .named_children(&mut cursor)
            .filter(|child| child.id() != constrained.id())
            .find(|child| is_type_kind(child.kind()))?;
        let right = self.type_of(right_node)?;
        let left = self
            .text(constrained)?
            .split('.')
            .map(|segment| segment.trim().to_owned())
            .collect();
        Some((left, relation, right))
    }

    /// 型 1 つ分のノードを読む。読めない構文なら `None`。
    fn type_of(&mut self, node: Node<'_>) -> Option<SwiftType> {
        match node.kind() {
            "user_type" => self.user_type_of(node),
            "optional_type" => {
                let wrapped = first_type_child_of(node)?;
                Some(SwiftType::Optional(Box::new(self.type_of(wrapped)?)))
            }
            "array_type" => {
                let element = first_type_child_of(node)?;
                Some(SwiftType::Array(Box::new(self.type_of(element)?)))
            }
            "dictionary_type" => {
                let types = type_children_of(node);
                let [key, value] = types.as_slice() else {
                    return None;
                };
                Some(SwiftType::Dictionary(
                    Box::new(self.type_of(*key)?),
                    Box::new(self.type_of(*value)?),
                ))
            }
            "tuple_type" => self.tuple_type_of(node),
            "function_type" => self.function_type_of(node),
            "opaque_type" => Some(SwiftType::Opaque(Box::new(
                self.type_of(first_type_child_of(node)?)?,
            ))),
            "existential_type" => Some(SwiftType::Existential(Box::new(
                self.type_of(first_type_child_of(node)?)?,
            ))),
            "protocol_composition_type" => {
                let mut members = BTreeSet::new();
                for member in type_children_of(node) {
                    members.insert(self.type_of(member)?);
                }
                Some(SwiftType::Composition(members))
            }
            _ => None,
        }
    }

    fn user_type_of(&mut self, node: Node<'_>) -> Option<SwiftType> {
        let mut path: Vec<(String, Vec<SwiftType>)> = Vec::new();
        let mut last_identifier = None;
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        for child in children {
            match child.kind() {
                TYPE_IDENTIFIER_KIND => {
                    path.push((self.text(child)?.to_owned(), Vec::new()));
                    last_identifier = Some(child);
                }
                TYPE_ARGUMENTS_KIND => {
                    let mut arguments = Vec::new();
                    for argument in type_children_of(child) {
                        arguments.push(self.type_of(argument)?);
                    }
                    path.last_mut()?.1 = arguments;
                }
                _ => return None,
            }
        }
        Some(self.path_type_of(path, last_identifier))
    }

    /// 名前のパスを型にする。束縛した型変数・`Self`・標準ライブラリの省略形を見分ける。
    ///
    /// `position_node` は型名を尋ねる位置（パスの末尾の識別子）。ソースを読むときに残す。
    fn path_type_of(
        &mut self,
        path: Vec<(String, Vec<SwiftType>)>,
        position_node: Option<Node<'_>>,
    ) -> SwiftType {
        let Some(((head, head_arguments), rest)) = path.split_first() else {
            return SwiftType::Tuple(Vec::new());
        };
        if head == SELF_TYPE {
            self.refers_to_self = true;
            return SwiftType::Named(path);
        }
        if head_arguments.is_empty() && self.declared.contains(head) {
            let index = self.parameter_index_of(head);
            if rest.is_empty() {
                return SwiftType::Parameter(index);
            }
            return SwiftType::Member(index, rest.iter().map(|(name, _)| name.clone()).collect());
        }

        let name = path
            .iter()
            .map(|(segment, _)| segment.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let resolution = (self.resolve)(&name);
        if let Some(opened) = Self::standard_sugar_of(resolution.as_ref(), &path) {
            return opened;
        }

        if self.recording
            && !self.type_names.contains(&name)
            && let Some(position) =
                position_node.and_then(|node| source_position_of(node, self.source))
        {
            self.references
                .push(TypeReference::new(name.clone(), position));
        }
        self.type_names.insert(name);
        match resolution {
            None => SwiftType::Named(path),
            Some(SwiftTypeResolution::Declared { identity, .. }) => {
                SwiftType::Named(declared_path_of(identity, path))
            }
            Some(SwiftTypeResolution::Opened(opened)) => {
                if path.iter().any(|(_, arguments)| !arguments.is_empty()) {
                    self.has_unopenable_alias = true;
                    return SwiftType::Named(path);
                }
                opened.0
            }
        }
    }

    /// 標準ライブラリの省略形を持つ型を、省略形を開いた形にする。当たらなければ `None`。
    fn standard_sugar_of(
        resolution: Option<&SwiftTypeResolution>,
        path: &[(String, Vec<SwiftType>)],
    ) -> Option<SwiftType> {
        let (last, arguments) = path.last()?;
        let qualified = match path.len() {
            1 => true,
            2 => path.first().is_some_and(|(module, module_arguments)| {
                module == STANDARD_LIBRARY_MODULE && module_arguments.is_empty()
            }),
            _ => false,
        };
        let sugar = qualified && STANDARD_SUGAR_NAMES.contains(&last.as_str());
        let standard = matches!(
            resolution,
            Some(SwiftTypeResolution::Declared { standard: true, .. })
        );
        if !sugar || !standard {
            return None;
        }
        match (last.as_str(), arguments.as_slice()) {
            (OPTIONAL_NAME, [wrapped]) => Some(SwiftType::Optional(Box::new(wrapped.clone()))),
            (ARRAY_NAME, [element]) => Some(SwiftType::Array(Box::new(element.clone()))),
            (DICTIONARY_NAME, [key, value]) => Some(SwiftType::Dictionary(
                Box::new(key.clone()),
                Box::new(value.clone()),
            )),
            (VOID_NAME, []) => Some(SwiftType::Tuple(Vec::new())),
            _ => None,
        }
    }

    fn parameter_type_of(&mut self, name: &str) -> SwiftType {
        SwiftType::Parameter(self.parameter_index_of(name))
    }

    /// 型変数の番号。初めて現れた名前には次の番号を振る。
    fn parameter_index_of(&mut self, name: &str) -> usize {
        if let Some(index) = self.numbered.iter().position(|numbered| numbered == name) {
            return index;
        }
        self.numbered.push(name.to_owned());
        self.numbered.len() - 1
    }

    fn tuple_type_of(&mut self, node: Node<'_>) -> Option<SwiftType> {
        let mut elements = Vec::new();
        let mut cursor = node.walk();
        let items: Vec<Node<'_>> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == TUPLE_TYPE_ITEM_KIND)
            .collect();
        for item in items {
            let children = all_children_of(item);
            let label = children
                .iter()
                .find(|child| child.kind() == SIMPLE_IDENTIFIER_KIND)
                .and_then(|child| self.text(*child))
                .map(str::to_owned);
            let written = children
                .iter()
                .rev()
                .find(|child| child.is_named() && is_type_kind(child.kind()))?;
            elements.push((label, self.type_of(*written)?));
        }
        // 括弧 1 つだけの `(T)` は `T` と同じ型
        if let [(None, single)] = elements.as_slice() {
            return Some(single.clone());
        }
        Some(SwiftType::Tuple(elements))
    }

    fn function_type_of(&mut self, node: Node<'_>) -> Option<SwiftType> {
        let children = all_children_of(node);
        // 戻り値の型に付いた属性（`() -> @Sendable () -> Void`）は読まない
        if children
            .iter()
            .any(|child| child.kind() == TYPE_MODIFIERS_KIND)
        {
            return None;
        }
        let parameter_list = node.child_by_field_name("params")?;
        let mut parameters = Vec::new();
        let mut cursor = parameter_list.walk();
        let items: Vec<Node<'_>> = parameter_list
            .named_children(&mut cursor)
            .filter(|child| child.kind() == TUPLE_TYPE_ITEM_KIND)
            .collect();
        for item in items {
            let item_children = all_children_of(item);
            let modifiers = self.parameter_modifiers_of(&item_children)?;
            let written = item_children
                .iter()
                .rev()
                .find(|child| child.is_named() && is_type_kind(child.kind()))?;
            let variadic = item_children
                .iter()
                .any(|child| !child.is_named() && self.text(*child) == Some(VARIADIC_TOKEN));
            parameters.push((modifiers, variadic, self.type_of(*written)?));
        }
        let value = children
            .iter()
            .rev()
            .find(|child| {
                child.is_named() && is_type_kind(child.kind()) && child.id() != parameter_list.id()
            })
            .copied()?;
        let value = self.type_of(value)?;
        Some(SwiftType::Function {
            parameters,
            effects: effects_of(&children, self.source),
            value: Box::new(value),
        })
    }
}

/// 名前のパスの末尾の名前を、宣言の場所の綴りに置き換える。**前の区切りは綴りのまま残す。**
///
/// **Why**: 尋ねるのは末尾の名前の位置で、宣言の場所が決まるのは末尾だけ。前の区切りを落とすと、
/// 末尾がプロトコルの `associatedtype` を指す `IntBag.Element` と `StrBag.Element` が同じ形になる
/// （偽陽性）。
///
/// **Why not（区切りを落として `Inner` と `Holder.Inner` を重ねる）**: 上の偽陽性と引き換えになる。
/// 前の区切りが残っても、同じ綴りで書いた 2 つは今までどおり重なる（偽陰性は綴りの違いだけ）。
fn declared_path_of(
    identity: String,
    mut path: Vec<(String, Vec<SwiftType>)>,
) -> Vec<(String, Vec<SwiftType>)> {
    if let Some((last, _)) = path.last_mut() {
        *last = identity;
    }
    path
}

/// 境界の右辺が `P & Q` なら、組に割ってから入れる。
///
/// 左辺は関数の境界なら型（[`SwiftConstraint`]）、囲む宣言の `where` ならレシーバからの
/// 相対のパス（[`ReceiverConstraint`]）。
fn push_split_constraint<Left: Clone + Ord>(
    constraints: &mut BTreeSet<(Left, ConstraintRelation, SwiftType)>,
    left: Left,
    relation: ConstraintRelation,
    right: SwiftType,
) {
    match (relation, right) {
        (ConstraintRelation::Conforms, SwiftType::Composition(members)) => {
            for member in members {
                constraints.insert((left.clone(), relation, member));
            }
        }
        (relation, right) => {
            constraints.insert((left, relation, right));
        }
    }
}

/// 宣言の効果。`async` は名前の無い字句、`throws` / `rethrows` は `throws` のノード。
fn effects_of(children: &[Node<'_>], source: &str) -> SwiftEffects {
    let text = |node: &Node<'_>| source.get(node.byte_range());
    SwiftEffects {
        asynchronous: children
            .iter()
            .any(|child| !child.is_named() && text(child) == Some(ASYNC_TOKEN)),
        throwing: children
            .iter()
            .find(|child| child.kind() == THROWS_KIND)
            .map(|throws| match text(throws) {
                Some(RETHROWS_TOKEN) => Throwing::Rethrows,
                _ => Throwing::Throws,
            }),
    }
}

/// そのノードが、宣言の `default_value` のフィールドに入った子か。
fn is_default_value(declaration: Node<'_>, child: Node<'_>) -> bool {
    let mut cursor = declaration.walk();
    declaration
        .children_by_field_name(DEFAULT_VALUE_FIELD, &mut cursor)
        .any(|value| value.id() == child.id())
}

/// 型を表すノードの種別か。[`SwiftSpelling::type_of`] が読む種別と同じ一覧。
///
/// **一覧に無い種別は型として拾わない**ので、それしか型を持たない引数は読めずに `None`
/// へ倒れる（偽陰性側。`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。
fn is_type_kind(kind: &str) -> bool {
    [
        "user_type",
        "optional_type",
        "array_type",
        "dictionary_type",
        "tuple_type",
        "function_type",
        "opaque_type",
        "existential_type",
        "protocol_composition_type",
    ]
    .contains(&kind)
}

fn first_type_child_of(node: Node<'_>) -> Option<Node<'_>> {
    type_children_of(node).into_iter().next()
}

fn type_children_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| is_type_kind(child.kind()))
        .collect()
}

fn all_children_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn children_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() == kind)
        .collect()
}

fn descendants_of_kinds<'tree>(node: Node<'tree>, kinds: &[&str]) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        if kinds.contains(&current.kind()) {
            found.push(current);
            continue;
        }
        let mut cursor = current.walk();
        let children: Vec<Node<'_>> = current.named_children(&mut cursor).collect();
        pending.extend(children.into_iter().rev());
    }
    found
}

/// 空白の連なりを 1 つに畳んだ綴り（`@available(iOS 13, *)` のような属性の引数の書き分け）。
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// どの型名も標準ライブラリの宣言まで辿れた、とする答え。宣言の場所の綴りは名前のまま。
    fn standard(name: &str) -> Option<SwiftTypeResolution> {
        Some(SwiftTypeResolution::Declared {
            identity: name.to_owned(),
            standard: true,
        })
    }

    /// どの型名も利用者の宣言まで辿れた、とする答え。
    fn user_declared(name: &str) -> Option<SwiftTypeResolution> {
        Some(SwiftTypeResolution::Declared {
            identity: name.to_owned(),
            standard: false,
        })
    }

    fn callable_of(spelling: &str) -> SwiftCallable {
        SwiftCallable::from_spelling(spelling, &standard, None).expect("読める綴り")
    }

    fn method_of(spelling: &str, enclosing: &str) -> SwiftCallable {
        SwiftCallable::from_spelling(
            spelling,
            &standard,
            Some(&SwiftEnclosingType::new(enclosing)),
        )
        .expect("読める綴り")
    }

    #[test]
    fn test_from_spelling_drops_internal_parameter_names() {
        assert_eq!(
            callable_of("func move(to x: Int) -> Int"),
            callable_of("func shift(to y: Int) -> Int")
        );
    }

    #[test]
    fn test_from_spelling_keeps_argument_labels() {
        assert_ne!(
            callable_of("func move(to x: Int) -> Int"),
            callable_of("func move(from x: Int) -> Int")
        );
    }

    #[test]
    fn test_from_spelling_reads_an_internal_name_without_external_name_as_the_label() {
        // `x: Int` のラベルは `x`。`x y: Int` と同じ呼び方（`f(x: 1)`）
        assert_eq!(
            callable_of("func f(x: Int)"),
            callable_of("func f(x y: Int)")
        );
        assert_ne!(
            callable_of("func f(x: Int)"),
            callable_of("func f(_ x: Int)")
        );
    }

    #[test]
    fn test_from_spelling_renames_type_parameters_by_appearance() {
        assert_eq!(
            callable_of("func first<T>(of x: T) -> T"),
            callable_of("func head<Element>(of y: Element) -> Element")
        );
    }

    #[test]
    fn test_from_spelling_keeps_which_parameter_the_type_variable_is_returned_from() {
        assert_ne!(
            callable_of("func pick<T, U>(a: T, b: U) -> T"),
            callable_of("func pick<T, U>(a: T, b: U) -> U")
        );
    }

    #[test]
    fn test_from_spelling_drops_redundant_parentheses() {
        assert_eq!(
            callable_of("func f(x: (Int)) -> ((Int))"),
            callable_of("func f(x: Int) -> Int")
        );
    }

    #[test]
    fn test_from_spelling_does_not_fold_a_tuple_parameter_of_a_function_type() {
        // `((Int, Int)) -> Void` はタプル 1 つを受け取り、`(Int, Int) -> Void` は 2 つを受け取る
        assert_ne!(
            callable_of("func f(g: ((Int, Int)) -> Int)"),
            callable_of("func f(g: (Int, Int) -> Int)")
        );
    }

    #[test]
    fn test_from_spelling_reads_standard_void_as_the_empty_tuple() {
        assert_eq!(
            callable_of("func f(g: (Int) -> Void)"),
            callable_of("func f(g: (Int) -> ())")
        );
    }

    #[test]
    fn test_from_spelling_keeps_void_declared_by_the_user_as_a_type_name() {
        let declared = |spelling| {
            SwiftCallable::from_spelling(spelling, &user_declared, None).expect("読める綴り")
        };

        let void = declared("func f(g: (Int) -> Void)");

        assert_ne!(void, declared("func f(g: (Int) -> ())"));
        assert!(void.type_names().contains("Void"));
    }

    #[test]
    fn test_from_spelling_reads_the_omitted_return_type_as_the_empty_tuple() {
        assert_eq!(callable_of("func f()"), callable_of("func f() -> ()"));
    }

    #[test]
    fn test_from_spelling_reads_optional_sugar_as_standard_optional() {
        assert_eq!(
            callable_of("func f(x: Int?) -> Optional<Int>"),
            callable_of("func f(x: Optional<Int>) -> Int?")
        );
        assert_eq!(
            callable_of("func f(x: Int?)"),
            callable_of("func f(x: Swift.Optional<Int>)")
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_a_user_optional_as_the_sugar() {
        let declared =
            SwiftCallable::from_spelling("func f(x: Optional<Int>)", &user_declared, None)
                .expect("読める綴り");

        assert_ne!(declared, callable_of("func f(x: Int?)"));
        assert!(declared.type_names().contains("Optional"));
    }

    #[test]
    fn test_from_spelling_reads_array_and_dictionary_sugar_as_standard_types() {
        assert_eq!(
            callable_of("func f(xs: [Int], d: [String : Int]) -> Array<Int>"),
            callable_of("func f(xs: Array<Int>, d: Dictionary<String, Int>) -> [Int]")
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_implicitly_unwrapped_as_optional() {
        assert_ne!(
            callable_of("func f(x: Int!)"),
            callable_of("func f(x: Int?)")
        );
        assert_ne!(
            callable_of("func f(x: Int!)"),
            callable_of("func f(x: Int)")
        );
    }

    #[test]
    fn test_from_spelling_keeps_effects() {
        let throwing = callable_of("func f() throws -> Int");

        assert_ne!(throwing, callable_of("func f() -> Int"));
        assert_ne!(throwing, callable_of("func f() async throws -> Int"));
        assert_ne!(throwing, callable_of("func f() rethrows -> Int"));
        assert_ne!(
            callable_of("func f(g: () async -> Int)"),
            callable_of("func f(g: () -> Int)")
        );
    }

    #[test]
    fn test_from_spelling_keeps_inout_variadic_and_escaping() {
        let plain = callable_of("func f(x: Int, g: () -> Int)");

        assert_ne!(plain, callable_of("func f(x: inout Int, g: () -> Int)"));
        assert_ne!(plain, callable_of("func f(x: Int..., g: () -> Int)"));
        assert_ne!(plain, callable_of("func f(x: Int, g: @escaping () -> Int)"));
    }

    #[test]
    fn test_from_spelling_keeps_attributes_written_on_a_function_type() {
        // tree-sitter-swift は `@MainActor` / `@Sendable` を `type_modifiers` に、先頭の
        // `@escaping` を `parameter_modifiers` に置く。どちらも呼び出しの約束を変える
        let plain = callable_of("func f(y: () -> Void)");

        assert_ne!(
            callable_of("func f(y: @MainActor @escaping () -> Void)"),
            plain
        );
        assert_ne!(
            callable_of("func f(y: @MainActor @escaping () -> Void)"),
            callable_of("func f(y: @escaping () -> Void)")
        );
        assert_ne!(
            callable_of("func f(y: @escaping @Sendable () -> Void)"),
            callable_of("func f(y: @escaping () -> Void)")
        );
        assert_ne!(
            callable_of("func f(w: (@Sendable () -> Void) -> Int)"),
            callable_of("func f(w: (() -> Void) -> Int)")
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_an_implicitly_unwrapped_return_as_plain() {
        assert_ne!(
            callable_of("func f() -> Int!"),
            callable_of("func f() -> Int")
        );
    }

    #[test]
    fn test_from_spelling_keeps_whether_a_parameter_has_a_default_value() {
        assert_eq!(
            callable_of("func f(x: Int = 3)"),
            callable_of("func f(x: Int = 4)")
        );
        assert_ne!(
            callable_of("func f(x: Int = 3)"),
            callable_of("func f(x: Int)")
        );
    }

    #[test]
    fn test_from_spelling_reads_inline_and_where_bounds_as_the_same_constraints() {
        let inline = callable_of("func f<T: Equatable & Hashable>(x: T)");

        assert_eq!(
            inline,
            callable_of("func f<T>(x: T) where T : Hashable, T : Equatable")
        );
        assert_ne!(inline, callable_of("func f<T>(x: T) where T : Equatable"));
    }

    #[test]
    fn test_from_spelling_counts_unused_type_parameters() {
        assert_ne!(
            callable_of("func f<T>(x: Int)"),
            callable_of("func f(x: Int)")
        );
    }

    #[test]
    fn test_from_spelling_drops_visibility_but_keeps_other_modifiers() {
        assert_eq!(
            method_of("public func f(x: Int)", "Wallet"),
            method_of("func f(x: Int)", "Wallet")
        );
        assert_ne!(
            method_of("static func f(x: Int)", "Wallet"),
            method_of("func f(x: Int)", "Wallet")
        );
        assert_ne!(
            method_of("class func f(x: Int)", "Wallet"),
            method_of("func f(x: Int)", "Wallet")
        );
    }

    #[test]
    fn test_from_spelling_keeps_the_receiver_of_a_method() {
        let method = method_of("func spend(_ amount: Int) -> Int", "Order");

        assert_ne!(
            method,
            method_of("func spend(_ amount: Int) -> Int", "Invoice")
        );
        assert_ne!(method, callable_of("func spend(_ amount: Int) -> Int"));
        assert!(method.type_names().contains("Order"));
    }

    #[test]
    fn test_from_spelling_reads_an_initializer_as_constructing_the_enclosing_type() {
        let initializer = method_of("init(base: Int)", "Wallet");

        assert_ne!(initializer, method_of("init(base: Int)", "Purse"));
        assert_ne!(initializer, method_of("init?(base: Int)", "Wallet"));
        assert_ne!(
            method_of("init?(base: Int)", "Wallet"),
            method_of("init!(base: Int)", "Wallet")
        );
        // 戻り値の型が同じ static 関数とは呼び方が違う
        assert_ne!(
            initializer,
            method_of("static func init(base: Int) -> Wallet", "Wallet")
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_an_initializer_without_its_enclosing_type() {
        assert_eq!(
            SwiftCallable::from_spelling("init(base: Int)", &standard, None),
            None
        );
    }

    #[test]
    fn test_from_spelling_tells_apart_members_of_extensions_with_different_equality_constraints() {
        // `extension Array where Element == Int` のレシーバは `Array<Int>`。hover は `where` を
        // 綴らないので、名前だけでは `Element == Double` の拡張のメソッドと重なる
        assert_ne!(
            method_of("func total() -> Int", "Array where Element == Int"),
            method_of("func total() -> Int", "Array where Element == Double")
        );
        assert_ne!(
            method_of("init(base: Int)", "Array where Element == Int"),
            method_of("init(base: Int)", "Array where Element == Double")
        );
        // 型名の集合（`Array` と `Int`）が揃っても、制約の形で分ける
        assert_ne!(
            method_of("func total() -> Int", "Array where Element == Int"),
            method_of("func total() -> Int", "Array where Element == [Int]")
        );
    }

    #[test]
    fn test_from_spelling_unifies_members_of_extensions_with_the_same_constraints() {
        assert_eq!(
            method_of(
                "func total(by scale: Int) -> Int",
                "Array where Element == Int"
            ),
            method_of(
                "func sum(by factor: Int) -> Int",
                "Array where Element == Int"
            )
        );
    }

    #[test]
    fn test_from_spelling_tells_apart_constrained_and_unconstrained_extensions() {
        // 制約の無い拡張のメソッドはどの `Array` でも呼べるので、差し替えられる範囲が違う
        assert_ne!(
            method_of("func total() -> Int", "Array where Element == Int"),
            method_of("func total() -> Int", "Array")
        );
    }

    #[test]
    fn test_from_spelling_tells_apart_extensions_with_different_type_arguments() {
        assert_ne!(
            method_of("func total() -> Int", "Array<Int>"),
            method_of("func total() -> Int", "Array<Double>")
        );
        assert_ne!(
            method_of("func total() -> Int", "Array<Int>"),
            method_of("func total() -> Int", "Array<[Int]>")
        );
    }

    #[test]
    fn test_from_spelling_unifies_bound_extensions_written_with_and_without_sugar() {
        assert_eq!(
            method_of("func total() -> Int", "Array<Int>"),
            method_of("func total() -> Int", "[Int]")
        );
        assert_eq!(
            method_of("func total() -> Int", "Optional<Int>"),
            method_of("func total() -> Int", "Int?")
        );
        // 省略形で書いた拡張のメソッドは、自由関数と重ならない
        assert_ne!(
            method_of("func total() -> Int", "[Int]"),
            callable_of("func total() -> Int")
        );
        // `Array` を宣言し直したモジュールでは、`[Int]` と同じ型とは限らない
        let user_array = SwiftCallable::from_spelling(
            "func total() -> Int",
            &user_declared,
            Some(&SwiftEnclosingType::new("Array<Int>")),
        )
        .expect("読める綴り");
        assert_ne!(user_array, method_of("func total() -> Int", "[Int]"));
    }

    #[test]
    fn test_from_spelling_drops_how_receiver_constraints_are_split() {
        assert_eq!(
            method_of(
                "func total() -> Int",
                "Array where Element: Equatable, Element: Hashable"
            ),
            method_of(
                "func total() -> Int",
                "Array where Element: Hashable & Equatable"
            )
        );
    }

    #[test]
    fn test_from_spelling_keeps_the_left_side_of_receiver_constraints_out_of_type_names() {
        // 左辺はレシーバの型変数で、辿っても宣言の位置が返らない。右辺は型名として辿る
        assert_eq!(
            method_of("func total() -> Int", "Array where Element == Money").type_names(),
            &BTreeSet::from(["Array", "Int", "Money"].map(str::to_owned))
        );
    }

    #[test]
    fn test_from_spelling_reads_receiver_constraints_on_an_associated_type_path() {
        // 左辺のパスは区切りごとに持つ。`Element.Index` と `Element` は別の制約
        assert_ne!(
            method_of("func total() -> Int", "Array where Element.Index == Int"),
            method_of("func total() -> Int", "Array where Element == Int")
        );
    }

    #[test]
    fn test_from_spelling_keeps_how_the_left_side_of_receiver_constraints_is_spelled() {
        // 左辺は辿らずに綴りで比べるので、`Self.Element` と `Element` は重ならない（偽陰性側）
        assert_ne!(
            method_of(
                "func total() -> Int",
                "Collection where Self.Element == Int"
            ),
            method_of("func total() -> Int", "Collection where Element == Int")
        );
        // 左辺の `Self` はレシーバ自身で、書かれた場所で決まる綴りに数えない
        assert!(
            !method_of("func total() -> Int", "Collection where Self: Equatable").refers_to_self()
        );
    }

    #[test]
    fn test_from_spelling_refers_to_self_on_the_right_side_of_receiver_constraints() {
        // 右辺の `Self` はプロトコルの拡張では採用する型ごとに違う
        assert!(
            method_of("func total() -> Int", "Collection where Element == Self").refers_to_self()
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_a_member_under_a_suppressed_constraint() {
        // `~Copyable` は型として読まない（偽陰性側）
        assert_eq!(
            SwiftCallable::from_spelling(
                "func total() -> Int",
                &standard,
                Some(&SwiftEnclosingType::new("Array where Element: ~Copyable"))
            ),
            None
        );
    }

    #[test]
    fn test_from_spelling_reads_members_of_a_constrained_type_declaration_and_a_nested_extension() {
        // どちらも main では型シグネチャを読まなかった形
        assert_ne!(
            method_of("func total() -> Int", "Box where T: Equatable"),
            method_of("func total() -> Int", "Box where T: Hashable")
        );
        assert_ne!(
            method_of("func total() -> Int", "Outer<Int>.Inner"),
            method_of("func total() -> Int", "Outer<[Int]>.Inner")
        );
        assert_eq!(
            method_of("func total() -> Int", "Outer<Int>.Inner").type_names(),
            &BTreeSet::from(["Int", "Outer.Inner"].map(str::to_owned))
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_a_member_of_an_unreadable_enclosing_type() {
        assert_eq!(
            SwiftCallable::from_spelling(
                "func total() -> Int",
                &standard,
                Some(&SwiftEnclosingType::new("Array where"))
            ),
            None
        );
    }

    #[test]
    fn test_from_spelling_does_not_read_typed_throws() {
        assert_eq!(
            SwiftCallable::from_spelling("func f() throws(Failure) -> Int", &standard, None),
            None
        );
        assert!(SwiftCallable::from_spelling("func f() throws -> Int", &standard, None).is_some());
    }

    #[test]
    fn test_from_spelling_marks_self_as_referring_to_self() {
        assert!(method_of("func copy() -> Self", "Wallet").refers_to_self());
        assert!(!method_of("func copy() -> Wallet", "Wallet").refers_to_self());
    }

    #[test]
    fn test_type_names_of_a_callable_exclude_bound_type_variables_and_sugar() {
        let callable = callable_of("func f<T>(x: T, y: Amount?) -> Optional<Swift.Int>");

        assert_eq!(
            callable.type_names(),
            &BTreeSet::from(["Amount".to_owned(), "Swift.Int".to_owned()])
        );
    }

    fn references_of(source: &str) -> Vec<(String, SourcePositionText)> {
        let tree = SyntaxTree::from_source(source, Grammar::Swift).expect("Swift の木にできる");
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| is_callable_kind(node.kind()))
            .expect("関数がある");
        type_references_of(function, source)
            .iter()
            .map(|reference| {
                (
                    reference.name().to_owned(),
                    format!("{:?}", reference.position()),
                )
            })
            .collect()
    }

    type SourcePositionText = String;

    #[test]
    fn test_type_references_of_a_source_function_skip_its_own_type_variables() {
        let source =
            "func f<T: Equatable>(x: T, y: Box<Amount>) -> Int? where T: Hashable {\n    0\n}\n";

        let names: BTreeSet<String> = references_of(source)
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            BTreeSet::from(["Amount", "Box", "Equatable", "Hashable", "Int"].map(str::to_owned))
        );
    }

    #[test]
    fn test_type_references_of_a_source_function_point_at_the_last_path_segment() {
        let source = "func f(x: Swift.Int) {}\n";

        let references = references_of(source);

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].0, "Swift.Int");
        let expected = format!(
            "{:?}",
            crate::source_position::SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::new(1).expect("正の行番号"),
                "func f(x: Swift."
            )
        );
        assert_eq!(references[0].1, expected);
    }

    #[test]
    fn test_type_references_of_a_source_function_keep_standard_sugar_names() {
        // 標準ライブラリを指すかは辿ってみるまで分からないので、`Optional` も尋ねる
        let names: BTreeSet<String> = references_of("func f(x: Optional<Int>) {}\n")
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            BTreeSet::from(["Int", "Optional"].map(str::to_owned))
        );
    }

    /// 囲む型の綴りから集めた型名。名前順（集める順は型名の入れ子で決まり、意味を持たない）。
    fn enclosing_names_of(source: &str, kind: &str) -> Vec<String> {
        let tree = SyntaxTree::from_source(source, Grammar::Swift).expect("Swift の木にできる");
        let member = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == kind)
            .expect("メンバーがある");
        enclosing_type_references_of(member, source)
            .into_iter()
            .map(|reference| reference.name().to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[test]
    fn test_enclosing_type_references_of_reads_struct_extension_and_enum() {
        assert_eq!(
            enclosing_names_of(
                "struct Wallet {\n    init(base: Int) {}\n}\n",
                INITIALIZER_KIND
            ),
            ["Wallet"]
        );
        assert_eq!(
            enclosing_names_of(
                "extension Shop.Wallet {\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Shop.Wallet"]
        );
        assert_eq!(
            enclosing_names_of(
                "enum Coin {\n    case a\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Coin"]
        );
    }

    #[test]
    fn test_enclosing_type_references_of_reads_type_arguments_and_the_right_side_of_constraints() {
        // 左辺の `Element` はレシーバの型変数で、辿らない
        assert_eq!(
            enclosing_names_of(
                "extension Array where Element == Money {\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Array", "Money"]
        );
        assert_eq!(
            enclosing_names_of(
                "extension Array<Int> {\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Array", "Int"]
        );
        assert_eq!(
            enclosing_names_of("extension [Int] {\n    func f() {}\n}\n", FUNCTION_KIND),
            ["Int"]
        );
        assert_eq!(
            enclosing_names_of(
                "struct Box<T> where T: Equatable {\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Box", "Equatable"]
        );
        assert_eq!(
            enclosing_names_of(
                "extension Outer<Int>.Inner {\n    func f() {}\n}\n",
                FUNCTION_KIND
            ),
            ["Int", "Outer.Inner"]
        );
    }

    #[test]
    fn test_enclosing_type_of_keeps_type_arguments_and_where_clause() {
        let enclosing_of = |source: &str| {
            let tree = SyntaxTree::from_source(source, Grammar::Swift).expect("Swift の木にできる");
            let member = tree
                .named_descendants()
                .into_iter()
                .find(|node| node.kind() == FUNCTION_KIND)
                .expect("メンバーがある");
            enclosing_type_of(member, source)
        };

        assert_eq!(
            enclosing_of("extension Array where Element == Int {\n    func f() {}\n}\n"),
            Some(SwiftEnclosingType::new("Array where Element == Int"))
        );
        assert_eq!(
            enclosing_of("extension Array<Int> {\n    func f() {}\n}\n"),
            Some(SwiftEnclosingType::new("Array<Int>"))
        );
        assert_eq!(
            enclosing_of("extension Array {\n    func f() {}\n}\n"),
            Some(SwiftEnclosingType::new("Array"))
        );
        // 型引数を省略形で書いた拡張も、囲む型を持つ（自由関数と同じレシーバ無しにしない）
        assert_eq!(
            enclosing_of("extension [Int] {\n    func f() {}\n}\n"),
            Some(SwiftEnclosingType::new("[Int]"))
        );
        assert_eq!(
            enclosing_of("extension [String: Int]? {\n    func f() {}\n}\n"),
            Some(SwiftEnclosingType::new("[String: Int]?"))
        );
    }

    #[test]
    fn test_enclosing_type_references_of_a_free_or_local_function() {
        assert!(enclosing_names_of("func f() {}\n", FUNCTION_KIND).is_empty());
        assert_eq!(
            enclosing_names_of(
                "struct W {\n    func outer() {\n        func inner() {}\n    }\n}\n",
                FUNCTION_KIND
            ),
            ["W"]
        );
    }

    /// `source` の `line` 行目で、`preceding` の直後に名前がある `typealias`。
    fn alias_at(source: &str, line: usize, preceding: &str) -> Option<SwiftAliasSource> {
        let position = SourcePosition::from_preceding_text(
            crate::line_number::LineNumber::new(line).expect("正の行番号"),
            preceding,
        );
        SwiftAliasSource::from_source(source, position)
    }

    /// 型名ごとの答えを表から引く。表に無い名前は辿れなかったことにする。
    fn resolving<'a>(
        table: &'a [(&'a str, SwiftTypeResolution)],
    ) -> impl Fn(&str) -> Option<SwiftTypeResolution> + 'a {
        move |name: &str| {
            table
                .iter()
                .find(|(written, _)| *written == name)
                .map(|(_, resolution)| resolution.clone())
        }
    }

    /// 宣言の場所 `identity` まで辿れた、利用者の型。
    fn declared_at(identity: &str) -> SwiftTypeResolution {
        SwiftTypeResolution::Declared {
            identity: identity.to_owned(),
            standard: false,
        }
    }

    /// 標準ライブラリの宣言 `identity` まで辿れた型。
    fn standard_at(identity: &str) -> SwiftTypeResolution {
        SwiftTypeResolution::Declared {
            identity: identity.to_owned(),
            standard: true,
        }
    }

    /// `typealias _ = right` を、右辺の型名を `table` で引いて開いた答え。
    fn opened(right: &str, table: &[(&str, SwiftTypeResolution)]) -> SwiftTypeResolution {
        let source = format!("typealias Alias = {right}\n");
        let alias = alias_at(&source, 1, "typealias ").expect("読める typealias");
        SwiftTypeResolution::Opened(alias.opened_with(&resolving(table)).expect("開ける右辺"))
    }

    fn resolved_callable_of(
        spelling: &str,
        table: &[(&str, SwiftTypeResolution)],
    ) -> SwiftCallable {
        SwiftCallable::from_spelling(spelling, &resolving(table), None).expect("読める綴り")
    }

    #[test]
    fn test_alias_source_records_right_side_names_at_the_declaring_positions() {
        let source = "struct Shop {}\ntypealias Ledger = [Shop.Money: Rate]\n";

        let alias = alias_at(source, 2, "typealias ").expect("読める typealias");

        let names: Vec<(&str, SourcePosition)> = alias
            .references()
            .iter()
            .map(|reference| (reference.name(), reference.position()))
            .collect();
        let at = |preceding: &str| {
            SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::new(2).expect("正の行番号"),
                preceding,
            )
        };
        // 位置はパスの末尾の名前（`Money`）。使用側と同じく、そこへ definition を尋ねる
        assert_eq!(
            names,
            [
                ("Shop.Money", at("typealias Ledger = [Shop.")),
                ("Rate", at("typealias Ledger = [Shop.Money: ")),
            ]
        );
    }

    #[test]
    fn test_alias_source_reads_only_the_declaration_named_at_the_position() {
        let source = "typealias Money = Int\ntypealias Rate = Double\n";

        assert!(alias_at(source, 2, "typealias ").is_some());
        // 名前ではなく `=` の後ろを指しても読まない
        assert!(alias_at(source, 2, "typealias Rate = ").is_none());
    }

    #[test]
    fn test_alias_source_reads_a_declaration_nested_in_a_non_generic_type() {
        let source = "struct Holder {\n    typealias Inner = Int\n}\n";

        assert!(alias_at(source, 2, "    typealias ").is_some());
    }

    #[test]
    fn test_alias_source_does_not_read_declarations_that_may_name_type_variables() {
        // 型引数を取る宣言は #527 で開く
        assert!(alias_at("typealias Pair<T> = (T, T)\n", 1, "typealias ").is_none());
        // 囲む型の型変数を指しうる: `Box<Int>.Item` と `Box<String>.Item` が同じ型に開くと偽陽性
        assert!(
            alias_at(
                "struct Box<T> {\n    typealias Item = T\n}\n",
                2,
                "    typealias "
            )
            .is_none()
        );
        assert!(
            alias_at(
                "struct Box<T> {\n    struct Inner {\n        typealias Item = Int\n    }\n}\n",
                3,
                "        typealias "
            )
            .is_none()
        );
        assert!(
            alias_at(
                "extension Array {\n    typealias Item = Element\n}\n",
                2,
                "    typealias "
            )
            .is_none()
        );
        assert!(
            alias_at(
                "protocol Shelf {\n    typealias Item = Int\n}\n",
                2,
                "    typealias "
            )
            .is_none()
        );
        assert!(
            alias_at(
                "func f<T>(x: T) {\n    typealias Item = T\n}\n",
                2,
                "    typealias "
            )
            .is_none()
        );
        // 対照: 型引数を持たない型の中は開く
        assert!(
            alias_at(
                "struct Box {\n    typealias Item = Int\n}\n",
                2,
                "    typealias "
            )
            .is_some()
        );
    }

    #[test]
    fn test_alias_source_does_not_read_a_right_side_naming_self_or_with_attributes() {
        assert!(
            alias_at(
                "class Node {\n    typealias Me = Self\n}\n",
                2,
                "    typealias "
            )
            .is_none()
        );
        assert!(
            alias_at(
                "typealias Handler = @Sendable () -> Void\n",
                1,
                "typealias "
            )
            .is_none()
        );
        assert!(alias_at("typealias Handler = () -> Void\n", 1, "typealias ").is_some());
    }

    #[test]
    fn test_from_spelling_reads_an_opened_alias_as_its_right_side() {
        let int = standard_at("@Int");
        let money = opened("Int", &[("Int", int.clone())]);

        let through_alias = resolved_callable_of(
            "func f(x: Money) -> Int",
            &[("Money", money), ("Int", int.clone())],
        );
        let direct = resolved_callable_of("func g(x: Int) -> Int", &[("Int", int)]);

        assert_eq!(through_alias, direct);
    }

    #[test]
    fn test_from_spelling_compares_right_side_names_by_their_declarations() {
        // 右辺の `Money` は宣言側のファイルの `Money`。使用側の同じ綴りとは別の型
        let bank_money = declared_at("@Bank.Money");
        let shop_money = declared_at("@Shop.Money");
        let ledger = opened("[Money]", &[("Money", bank_money.clone())]);
        let array = standard_at("@Array");

        let through_alias = resolved_callable_of("func f(x: Ledger)", &[("Ledger", ledger)]);
        let same_declaration = resolved_callable_of("func g(x: [Money])", &[("Money", bank_money)]);
        let other_declaration = resolved_callable_of(
            "func g(x: Array<Money>)",
            &[("Money", shop_money), ("Array", array)],
        );

        assert_eq!(through_alias, same_declaration);
        assert_ne!(through_alias, other_declaration);
    }

    #[test]
    fn test_from_spelling_keeps_the_qualifying_segments_of_a_declared_path() {
        // 末尾がプロトコルの `associatedtype` を指すと、宣言の場所は 2 つで同じになりうる
        let element = declared_at("@Sequence.Element");

        let int_bag = resolved_callable_of(
            "func f(x: IntBag.Element)",
            &[("IntBag.Element", element.clone())],
        );
        let text_bag = resolved_callable_of(
            "func g(x: TextBag.Element)",
            &[("TextBag.Element", element.clone())],
        );

        assert_ne!(int_bag, text_bag);
        // 対照: 同じ綴りで同じ宣言なら重なる
        assert_eq!(
            int_bag,
            resolved_callable_of("func h(x: IntBag.Element)", &[("IntBag.Element", element)])
        );
    }

    #[test]
    fn test_from_spelling_compares_a_single_name_by_its_declaration() {
        let inner = declared_at("@Holder.Inner");

        let one = resolved_callable_of("func f(x: Inner)", &[("Inner", inner.clone())]);

        assert_eq!(
            one,
            resolved_callable_of("func g(x: Inner)", &[("Inner", inner)])
        );
        // 対照: 別の宣言なら綴りが同じでも重ならない
        assert_ne!(
            one,
            resolved_callable_of(
                "func h(x: Inner)",
                &[("Inner", declared_at("@Other.Inner"))]
            )
        );
    }

    #[test]
    fn test_from_spelling_reads_standard_sugar_in_an_opened_right_side() {
        let int = standard_at("@Int");
        let optional = standard_at("@Optional");
        let maybe = opened(
            "Optional<Int>",
            &[("Optional", optional), ("Int", int.clone())],
        );

        let through_alias = resolved_callable_of("func f(x: Maybe)", &[("Maybe", maybe)]);
        let sugar = resolved_callable_of("func g(x: Int?)", &[("Int", int)]);

        assert_eq!(through_alias, sugar);
    }

    #[test]
    fn test_from_spelling_marks_an_opened_alias_with_type_arguments_unopenable() {
        let int = standard_at("@Int");
        let money = opened("Int", &[("Int", int.clone())]);

        let with_arguments = resolved_callable_of(
            "func f(x: Money<Int>)",
            &[("Money", money.clone()), ("Int", int)],
        );
        let without = resolved_callable_of("func f(x: Money)", &[("Money", money)]);

        assert!(with_arguments.has_unopenable_alias());
        assert!(!without.has_unopenable_alias());
    }

    #[test]
    fn test_opened_type_size_counts_every_node_of_the_right_side() {
        let int = standard_at("@Int");
        let Some(SwiftTypeResolution::Opened(single)) =
            Some(opened("Int", &[("Int", int.clone())]))
        else {
            panic!("開いた右辺");
        };
        let Some(SwiftTypeResolution::Opened(pair)) = Some(opened("(Int, Int)", &[("Int", int)]))
        else {
            panic!("開いた右辺");
        };

        assert_eq!(single.size(), 1);
        assert_eq!(pair.size(), 3);
    }
}
