//! 型 1 つ分の綴りを、構文木から読んだ構造にする。
//!
//! **綴りの一致で比べると、同じ型が書かれ方の違いで別物になる。** 引数名・タプルのラベル・
//! 型述語の主語は書いた人の都合で、共用体の並びと括弧は同じ型に 2 通りの綴りを与える
//! （**交差型の並びは型の一部**なので落とさない）。
//! 畳んだり切ったりして落とそうとすると、**形の一覧を持つことになる**
//! （`docs/dryguard-plan.md` の Stage 2 が求めるのは綴りではなく型の一致）。
//!
//! **分解するのは、書かれ方の違いを落とすのに要る形だけ。** それ以外は綴りのまま持ち、
//! 綴りで比べる（[`TypeStructure::Spelled`]）。一覧を空にすると全部が綴りのまま比べられる
//! ので、**一覧から漏れた形は偽陰性側へ倒れる**
//! (`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
//!
//! **LSP を知らない。** 綴りを受け取って構造を返すだけなので、`semantics` へ依存しない
//! （`rules/architecture.md`「依存方向のルール」）。型エイリアスを開くのは
//! `semantics` が先に済ませてから渡す。

use std::collections::BTreeSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::syntax::tree::{Grammar, SyntaxTree};
use crate::syntax::type_spelling::{
    Wrapped, names_only_types, substitutable_type_name_spans_of, type_name_spans_of,
};

/// 付け替えた型変数の綴りの前置き。
///
/// `%` は識別子に使えない文字なので、**元の型名と衝突しない**。付け替えた後の綴りを
/// もう一度付け替えてしまうこともない。
const PLACEHOLDER_PREFIX: char = '%';

/// 関数型を表すノードの種別（`(a: string) => void`）。
const FUNCTION_TYPE_KIND: &str = "function_type";

/// 構築型を表すノードの種別（`new (a: string) => R`）。
const CONSTRUCTOR_TYPE_KIND: &str = "constructor_type";

/// 括弧で包んだ型を表すノードの種別。
const PARENTHESIZED_TYPE_KIND: &str = "parenthesized_type";

/// 共用体を表すノードの種別。
const UNION_TYPE_KIND: &str = "union_type";

/// 交差型を表すノードの種別。
const INTERSECTION_TYPE_KIND: &str = "intersection_type";

/// タプルを表すノードの種別。
const TUPLE_TYPE_KIND: &str = "tuple_type";

/// 配列を表すノードの種別（`T[]`）。
const ARRAY_TYPE_KIND: &str = "array_type";

/// 型引数を伴う型名を表すノードの種別（`Array<T>`）。
const GENERIC_TYPE_KIND: &str = "generic_type";

/// 型引数の並びを表すノードの種別（`<T, U>`）。
const TYPE_ARGUMENTS_KIND: &str = "type_arguments";

/// 型名 1 つを表すノードの種別。
const TYPE_IDENTIFIER_KIND: &str = "type_identifier";

/// 修飾された型名を表すノードの種別（`money.Amount`）。
const NESTED_TYPE_IDENTIFIER_KIND: &str = "nested_type_identifier";

/// 組み込みの型を表すノードの種別（`string`）。
const PREDEFINED_TYPE_KIND: &str = "predefined_type";

/// 型変数の宣言の並びを表すノードの種別（`<T extends X>`）。
const TYPE_PARAMETERS_KIND: &str = "type_parameters";

/// 型変数の制約を表すノードの種別（`extends X`）。
const CONSTRAINT_KIND: &str = "constraint";

/// 型変数の既定の型を表すノードの種別（`= D`）。
const DEFAULT_TYPE_KIND: &str = "default_type";

/// 引数リストを表すノードの種別。
const FORMAL_PARAMETERS_KIND: &str = "formal_parameters";

/// 省略できる引数・タプルの要素を表すノードの種別（`a?: T`）。
const OPTIONAL_PARAMETER_KIND: &str = "optional_parameter";

/// 必ず渡す引数・タプルの要素を表すノードの種別（`a: T`）。可変長もここに入る。
const REQUIRED_PARAMETER_KIND: &str = "required_parameter";

/// 型注釈を表すノードの種別（`: T`）。
const TYPE_ANNOTATION_KIND: &str = "type_annotation";

/// 可変長を表すノードの種別（`...rest`）。
const REST_PATTERN_KIND: &str = "rest_pattern";

/// 呼び出し時に渡さない引数の名前・型述語の主語としての `this` を表すノードの種別。
const THIS_KIND: &str = "this";

/// ラベルの無いタプルの、省略できる要素を表すノードの種別（`string?`）。
const OPTIONAL_TYPE_KIND: &str = "optional_type";

/// ラベルの無いタプルの、可変長の要素を表すノードの種別（`...number[]`）。
const REST_TYPE_KIND: &str = "rest_type";

/// 型述語を表すノードの種別（`value is string`）。
const TYPE_PREDICATE_KIND: &str = "type_predicate";

/// 表明を伴う型述語を包むノードの種別（`asserts value is string`）。
const ASSERTS_KIND: &str = "asserts";

/// 実装を持たない構築型を導く修飾子。
const ABSTRACT_MODIFIER: &str = "abstract";

/// 型 1 つ分の綴りを構文木から読んだ形。
///
/// **綴りの上の違いのうち、型の違いでないものは持たない。** 引数名・タプルのラベル・
/// 型述語の主語の綴り・括弧は、読んだ時点で落ちている。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TypeStructure {
    /// 呼べる型（関数型・構築型）。
    Callable(Callable),
    /// 共用体。
    Union(Vec<TypeStructure>),
    /// 交差型。**並びは落とさない**（呼べる型を並べるとオーバーロードの並びになる）。
    Intersection(Vec<TypeStructure>),
    /// タプル。**ラベルは落ちている**（TypeScript ではラベルが型を変えない）。
    Tuple(Vec<TupleElement>),
    /// 配列。
    Array(Box<TypeStructure>),
    /// 型名と、それに渡した型引数。
    Named {
        name: String,
        arguments: Vec<TypeStructure>,
    },
    /// 型述語。**主語は引数の位置で持つ**ので、引数名が違うだけの 2 つが同じ形になる。
    Predicate {
        /// 主語が指す引数の位置。
        subject: usize,
        /// 表明を伴うか（`asserts value is T`）。
        asserted: bool,
        /// 絞り込む先の型。`asserts value` のように書かれていなければ `None`。
        narrowed: Option<Box<TypeStructure>>,
    },
    /// 分解しない型。綴りのまま持ち、綴りで比べる。
    ///
    /// **ここへ落ちても答えは出る。** 同じ綴りなら重なり、違う綴りなら重ならないので、
    /// 倒れる向きは偽陰性になる。
    Spelled(String),
}

/// 呼べる型 1 つ分。
///
/// **`new` を付けて呼ぶ型と、そのまま呼ぶ型は別の型**なので、[`SignatureKind`] を持つ。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Callable {
    kind: SignatureKind,
    /// 型変数。付け替え後は番号順の並び。
    type_parameters: Vec<TypeParameter>,
    /// 引数。名前を落とし、渡し方と型だけが残る。
    parameters: Vec<Parameter>,
    return_type: Box<TypeStructure>,
}

/// 呼び出しの仕方。
///
/// クラスのコンストラクタもチャンクになるので、落とすと
/// `constructor Result(value: string): Result` と `function create(value: string): Result` が
/// 単一化可能になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SignatureKind {
    /// そのまま呼ぶ（`f(a)`）。
    Call,
    /// `new` を付けて呼ぶ（`new C(a)`）。
    Construct,
}

/// 型変数 1 つ分。
///
/// 既定の型を持つのは、**型引数を省いて呼んだときの型がそこで決まる**ため。
/// 落とすと `f<T = string>(): T` と `g<U = number>(): U` が同じ形になるが、
/// どちらも引数無しで呼ぶと戻り値の型が違う。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TypeParameter {
    /// 名前の手前に書かれた修飾子（`const`）。
    ///
    /// **綴りのまま持つ。** 修飾子ごとに意味を読むと、grammar が受け付ける綴りが
    /// 増えるたびに一覧が増える。綴りで比べれば、知らない修飾子は
    /// **付いている側と付いていない側を分ける**（偽陰性側）。
    modifiers: Vec<String>,
    /// 名前。付け替えの後は `%0` などの綴りになる。
    name: String,
    constraint: Option<TypeStructure>,
    default: Option<TypeStructure>,
}

/// 引数 1 つ分。名前を落とし、**渡し方と型だけ**を残した形。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Parameter {
    kind: ParameterKind,
    annotated_type: TypeStructure,
}

/// その引数の渡し方。
///
/// **落とすと渡し方の違う引数が同じ形になる。** `a: string` は必ず渡す引数、
/// `a?: string` は省ける引数、`this: E` は呼び出し時に渡さない引数で、
/// 受け取る値の数がそれぞれ違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ParameterKind {
    /// 必ず渡す。
    Required,
    /// 省略できる（`a?: T`）。
    Optional,
    /// 可変長（`...a: T[]`）。
    Rest,
    /// 呼び出し時に渡さない（`this: T`）。TypeScript が受け取り手の型を書く場所。
    Receiver,
}

/// タプルの要素 1 つ分。**ラベルは落ちている。**
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TupleElement {
    kind: TupleElementKind,
    element: TypeStructure,
}

/// そのタプルの要素の並び方。
///
/// **引数の渡し方（[`ParameterKind`]）と分ける。** タプルには受け取り手（`this`）が無く、
/// 1 つの一覧にすると**タプルでは作れない要素**が型の上で作れてしまう
/// (`rules/coding.md`「不正な状態を型で表現できなくする」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TupleElementKind {
    /// 必ずある。
    Required,
    /// 省略できる（`string?`）。
    Optional,
    /// 可変長（`...number[]`）。
    Rest,
}

impl Callable {
    /// 呼べる型として読める綴りから読む。読めない綴り・呼べる型でない綴りでは `None`。
    ///
    /// **分解しない形へ落ちた綴りも `None`。** `abstract new (a: string) => R` は
    /// 呼べる型だが綴りのまま持つ側なので、シグネチャ全体としては読めなかったことになる。
    ///
    /// `spelling` は型 1 つ分の綴り（`new (a: string) => R` のような構築型も受ける）。
    /// **引数名やラベルはここで落ちるが、型変数の付け替えと並びの固定はまだ行わない**
    /// （[`Callable::normalized`]）。分けてあるのは、**綴りに残る型名を数えるのが
    /// 付け替えの前**だから（付け替え後の `%0` は型として読めない）。
    pub(crate) fn from_spelling(spelling: &str) -> Option<Self> {
        match TypeStructure::from_spelling(spelling)? {
            TypeStructure::Callable(callable) => Some(callable),
            TypeStructure::Union(_)
            | TypeStructure::Intersection(_)
            | TypeStructure::Tuple(_)
            | TypeStructure::Array(_)
            | TypeStructure::Named { .. }
            | TypeStructure::Predicate { .. }
            | TypeStructure::Spelled(_) => None,
        }
    }

    /// 型変数を出現順に付け替え、共用体の並びを固定した形。
    /// 綴りのまま持っている部分を読めなければ `None`。
    ///
    /// **付け替えてから並べ替える。** 逆にすると、付け替え前の名前で整列することになり、
    /// `f<A>(x: A | Middle)` と `g<Z>(x: Z | Middle)` が別の並びになる。
    ///
    /// **番号はシグネチャ全体で 1 つの通し番号。** 入れ子のスコープごとに `%0` から
    /// 振り直すと、`<T>(cb: <U>(u: U) => T) => void` と `<T>(cb: <U>(u: U) => U) => void` が
    /// どちらも「内側 `%0`・戻り値 `%0`」になり、**単一化できない 2 つを重ねてしまう**。
    pub(crate) fn normalized(self) -> Option<Self> {
        let mut scopes = TypeVariableScopes::new();

        Some(self.renamed(&mut scopes)?.sorted())
    }

    /// 綴りに残っている型名。綴りのまま持っている部分を読めなければ `None`。
    ///
    /// **束縛された型変数の名前は数えない。宣言だけでなく使用も。** このシグネチャの
    /// 中でだけ意味を持つので、宣言を辿る相手ではない（`syntax::type_reference` が
    /// 集める側でも外している）。**数えると、集める側が外したぶんが
    /// 「尋ねていない型名」に見える**（`semantics::type_signature`）。
    ///
    /// **外側で束縛された型変数は残る。** そのシグネチャからは辿れないので、
    /// 落とすと**別のファイルの同じ綴りと重なる**（偽陽性）。
    ///
    /// **読めなかったことを空の集合で表さない。** 名前が 1 つも無いことと、
    /// 名前を数えられなかったことは別
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    pub(crate) fn type_names(&self) -> Option<BTreeSet<String>> {
        self.free_type_names_of(&mut Vec::new(), SpelledBinders::Counted)
    }

    /// 綴りのまま持っている部分が、すべて型の名前だけで書かれているか。
    /// 綴りのまま持っている部分を読めなければ `None`。
    ///
    /// **分解できた部分は見ない。** 型が書かれる場所だけを歩いて組み立てた形なので、
    /// そこに残る綴りは型名（[`TypeStructure::Named`]）で、辿れたかどうかは型名として
    /// 答えられる。**型名にならないのに指す先が書いた人の位置で決まる綴り**
    /// （`typeof localValue` / `{ [key]: string }` / `import("./local").T` / `this`）は、
    /// 分解しない側（[`TypeStructure::Spelled`]）にしか現れない。
    ///
    /// **同じシグネチャが束縛した値の名前も外を指す扱いになる。** `(x: string) => typeof x`
    /// の `x` は引数なので指す先はこのシグネチャの中で決まるが、**引数の名前は読んだ時点で
    /// 落ちている**（[`Parameter`]）ので、ここから束縛を引けない。倒れる向きは偽陰性
    /// （測れる答えを 1 つ落とすだけ）なので直していない（Issue #192）。
    pub(crate) fn names_only_types(&self) -> Option<bool> {
        let mut names_only = true;

        for declared in &self.type_parameters {
            for annotated in [&declared.constraint, &declared.default]
                .into_iter()
                .flatten()
            {
                names_only &= annotated.names_only_types()?;
            }
        }
        for parameter in &self.parameters {
            names_only &= parameter.annotated_type.names_only_types()?;
        }
        names_only &= self.return_type.names_only_types()?;

        Some(names_only)
    }

    /// 綴りに残っている型名のうち、**宣言を辿る相手になりうるもの**。
    /// 綴りのまま持っている部分を読めなければ `None`。
    ///
    /// [`Callable::type_names`] との違いは、**綴りのまま持つ部分の中で束縛された名前**
    /// （`infer U` / マップ型の `K in …`）を外すところだけ（[`SpelledBinders`]）。
    pub(crate) fn traceable_type_names(&self) -> Option<BTreeSet<String>> {
        self.free_type_names_of(&mut Vec::new(), SpelledBinders::Skipped)
    }

    /// `scopes` が束縛していない型名。
    ///
    /// 自分の型変数でスコープを 1 つ積んでから降りる（[`Callable::renamed`] と同じ形）。
    fn free_type_names_of(
        &self,
        scopes: &mut Vec<BTreeSet<String>>,
        binders: SpelledBinders,
    ) -> Option<BTreeSet<String>> {
        let mut names = BTreeSet::new();

        scopes.push(
            self.type_parameters
                .iter()
                .map(|declared| declared.name.clone())
                .collect(),
        );

        for declared in &self.type_parameters {
            for annotated in [&declared.constraint, &declared.default]
                .into_iter()
                .flatten()
            {
                names.extend(annotated.free_type_names_of(scopes, binders)?);
            }
        }
        for parameter in &self.parameters {
            names.extend(
                parameter
                    .annotated_type
                    .free_type_names_of(scopes, binders)?,
            );
        }
        names.extend(self.return_type.free_type_names_of(scopes, binders)?);

        scopes.pop();

        Some(names)
    }

    /// 型変数を付け替えた形。
    fn renamed(self, scopes: &mut TypeVariableScopes) -> Option<Self> {
        scopes.opened(&self.type_parameters);

        let parameters = self
            .parameters
            .into_iter()
            .map(|parameter| parameter.renamed(scopes))
            .collect::<Option<Vec<Parameter>>>()?;
        let return_type = Box::new(self.return_type.renamed(scopes)?);

        // 引数にも戻り値にも現れない型変数は、宣言の順で後ろに置く
        scopes.assigned_remaining();

        // 制約と既定の型は出現順に数えない（`<T, U extends T>` の `T` は
        // 引数に現れた順で決まる）が、付け替えの対象にはする
        let mut renamed = Vec::with_capacity(self.type_parameters.len());
        for declared in self.type_parameters {
            let placeholder = scopes.placeholder_of(&declared.name)?;
            renamed.push((placeholder, declared.renamed(placeholder, scopes)?));
        }
        renamed.sort_by_key(|(placeholder, _)| *placeholder);

        scopes.closed();

        Some(Self {
            kind: self.kind,
            type_parameters: renamed
                .into_iter()
                .map(|(_, parameter)| parameter)
                .collect(),
            parameters,
            return_type,
        })
    }

    /// 可換な並びを固定した形。
    fn sorted(self) -> Self {
        Self {
            kind: self.kind,
            type_parameters: self
                .type_parameters
                .into_iter()
                .map(TypeParameter::sorted)
                .collect(),
            parameters: self.parameters.into_iter().map(Parameter::sorted).collect(),
            return_type: Box::new(self.return_type.sorted()),
        }
    }
}

impl TypeParameter {
    /// 名前を付け替え、制約と既定の型の中の型変数も付け替えた形。
    fn renamed(self, placeholder: usize, scopes: &mut TypeVariableScopes) -> Option<Self> {
        let renamed_part = |part: Option<TypeStructure>, scopes: &mut TypeVariableScopes| match part
        {
            Some(structure) => structure.renamed(scopes).map(Some),
            None => Some(None),
        };

        Some(Self {
            modifiers: self.modifiers,
            name: placeholder_spelling(placeholder),
            constraint: renamed_part(self.constraint, scopes)?,
            default: renamed_part(self.default, scopes)?,
        })
    }

    /// 可換な並びを固定した形。
    fn sorted(self) -> Self {
        Self {
            modifiers: self.modifiers,
            name: self.name,
            constraint: self.constraint.map(TypeStructure::sorted),
            default: self.default.map(TypeStructure::sorted),
        }
    }
}

impl Parameter {
    /// 型変数を付け替えた形。**渡し方は付け替えの対象にしない。**
    fn renamed(self, scopes: &mut TypeVariableScopes) -> Option<Self> {
        Some(Self {
            kind: self.kind,
            annotated_type: self.annotated_type.renamed(scopes)?,
        })
    }

    /// 可換な並びを固定した形。
    fn sorted(self) -> Self {
        Self {
            kind: self.kind,
            annotated_type: self.annotated_type.sorted(),
        }
    }
}

impl TypeStructure {
    /// 型 1 つ分として読める綴りから読む。型として読めない綴りでは `None`。
    ///
    /// `spelling` は型 1 つ分の綴り（`type spelling`）。**呼べる型に限らない**ので、
    /// メンバーとしての型（アクセサに返る綴りの `:` の右辺）もここで読む。
    ///
    /// **Why（呼べる型もここを通す）**: 構文木にする手順は綴りが何を表していても同じで、
    /// 呼べる型かどうかは読んだ後の形で分かる（[`Callable::from_spelling`]）。
    /// 2 箇所に置くと、包み方（[`Wrapped`]）を替えたときに片方だけが古くなる。
    pub(crate) fn from_spelling(spelling: &str) -> Option<Self> {
        let wrapped = Wrapped::from_spelling(spelling)?;
        let tree = SyntaxTree::from_source(wrapped.text(), Grammar::TypeScript).ok()?;
        if tree.has_error() {
            return None;
        }

        structured(spanning_node(&tree, wrapped.span())?, wrapped.text())
    }

    /// 型変数を出現順に付け替え、共用体の並びを固定した形。
    /// 綴りのまま持っている部分を読めなければ `None`。
    ///
    /// **外側に型変数の宣言を置かない。** 型 1 つ分の綴りの中で宣言されるのは、
    /// 入れ子の呼べる型が自分で宣言した型変数だけ（[`Callable::renamed`] がその都度
    /// スコープを開く）。ここで宣言されていない名前は、**どこかで宣言された型名**なので
    /// 付け替えない。
    pub(crate) fn normalized(self) -> Option<Self> {
        let mut scopes = TypeVariableScopes::new();

        Some(self.renamed(&mut scopes)?.sorted())
    }

    /// 綴りに残っている型名。綴りのまま持っている部分を読めなければ `None`。
    ///
    /// **入れ子の呼べる型が束縛した型変数は数えない**（[`Callable::type_names`]）。
    pub(crate) fn type_names(&self) -> Option<BTreeSet<String>> {
        self.free_type_names_of(&mut Vec::new(), SpelledBinders::Counted)
    }

    /// 綴りのまま持っている部分が、すべて型の名前だけで書かれているか
    /// （[`Callable::names_only_types`]）。
    pub(crate) fn names_only_types(&self) -> Option<bool> {
        match self {
            Self::Callable(callable) => callable.names_only_types(),
            Self::Union(members) | Self::Intersection(members) => {
                all_names_only_types(members.iter())
            }
            Self::Tuple(elements) => {
                all_names_only_types(elements.iter().map(|element| &element.element))
            }
            Self::Array(element) => element.names_only_types(),
            Self::Named { arguments, .. } => all_names_only_types(arguments.iter()),
            Self::Predicate { narrowed, .. } => {
                all_names_only_types(narrowed.iter().map(Box::as_ref))
            }
            Self::Spelled(spelling) => names_only_types(spelling),
        }
    }

    /// 綴りに残っている型名のうち、**宣言を辿る相手になりうるもの**
    /// （[`Callable::traceable_type_names`]）。
    pub(crate) fn traceable_type_names(&self) -> Option<BTreeSet<String>> {
        self.free_type_names_of(&mut Vec::new(), SpelledBinders::Skipped)
    }

    /// `scopes` が束縛していない型名。
    fn free_type_names_of(
        &self,
        scopes: &mut Vec<BTreeSet<String>>,
        binders: SpelledBinders,
    ) -> Option<BTreeSet<String>> {
        let mut names = BTreeSet::new();

        match self {
            Self::Callable(callable) => {
                names.extend(callable.free_type_names_of(scopes, binders)?);
            }
            Self::Union(members) | Self::Intersection(members) => {
                for member in members {
                    names.extend(member.free_type_names_of(scopes, binders)?);
                }
            }
            Self::Tuple(elements) => {
                for element in elements {
                    names.extend(element.element.free_type_names_of(scopes, binders)?);
                }
            }
            Self::Array(element) => names.extend(element.free_type_names_of(scopes, binders)?),
            Self::Named { name, arguments } => {
                if !is_bound_in(scopes, name) {
                    names.insert(name.clone());
                }
                for argument in arguments {
                    names.extend(argument.free_type_names_of(scopes, binders)?);
                }
            }
            Self::Predicate { narrowed, .. } => {
                if let Some(narrowed) = narrowed {
                    names.extend(narrowed.free_type_names_of(scopes, binders)?);
                }
            }
            Self::Spelled(spelling) => {
                for span in binders.spans_of(spelling)? {
                    let name = spelling.get(span)?;
                    if is_bound_in(scopes, name) {
                        continue;
                    }
                    names.insert(name.to_owned());
                }
            }
        }

        Some(names)
    }

    /// 型変数を付け替えた形。綴りのまま持っている部分を読めなければ `None`。
    fn renamed(self, scopes: &mut TypeVariableScopes) -> Option<Self> {
        Some(match self {
            Self::Callable(callable) => Self::Callable(callable.renamed(scopes)?),
            Self::Union(members) => Self::Union(renamed_members(members, scopes)?),
            Self::Intersection(members) => Self::Intersection(renamed_members(members, scopes)?),
            Self::Tuple(elements) => Self::Tuple(
                elements
                    .into_iter()
                    .map(|element| {
                        Some(TupleElement {
                            kind: element.kind,
                            element: element.element.renamed(scopes)?,
                        })
                    })
                    .collect::<Option<Vec<TupleElement>>>()?,
            ),
            Self::Array(element) => Self::Array(Box::new(element.renamed(scopes)?)),
            Self::Named { name, arguments } => {
                // 型名を先に見る。型引数より手前に書かれているので、出現順もそちらが先
                let renamed = match scopes.placeholder_of(&name) {
                    Some(placeholder) => placeholder_spelling(placeholder),
                    None => name,
                };

                Self::Named {
                    name: renamed,
                    arguments: renamed_members(arguments, scopes)?,
                }
            }
            Self::Predicate {
                subject,
                asserted,
                narrowed,
            } => Self::Predicate {
                subject,
                asserted,
                narrowed: match narrowed {
                    Some(narrowed) => Some(Box::new(narrowed.renamed(scopes)?)),
                    None => None,
                },
            },
            Self::Spelled(spelling) => Self::Spelled(renamed_spelling(&spelling, scopes)?),
        })
    }

    /// 可換な並びを固定した形。
    ///
    /// **並べ替えるのは共用体だけ。** タプルの要素と型引数の並びは型の一部で、
    /// **交差型の並びも型の一部**（下記）。
    ///
    /// **Why not（交差型も並べ替える）**: 交差型が並べる呼べる型は
    /// オーバーロードの並びそのもので、TypeScript は書かれた順に突き合わせる
    /// （`rules/naming.md`「`overload set` の並びを落とさない」）。`tsc 5.9.3` で確かめると、
    /// `Wide & Narrow` と `Narrow & Wide` は同じ引数に対して別の型を返す。
    /// 並べ替えると**単一化できない 2 つを重ねてしまう**（偽陽性）。
    ///
    /// **Why not（呼べる型を含む交差型だけ並べ替えない）**: 呼び出しの形を持つかは
    /// 綴りのまま持つ側（オブジェクト型の呼び出しシグネチャ）や、開けなかった型名からは
    /// 決められない。一覧に無い形が偽陽性へ倒れるので、並べ替えないほうへ寄せる
    /// （`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。
    fn sorted(self) -> Self {
        match self {
            Self::Callable(callable) => Self::Callable(callable.sorted()),
            Self::Union(members) => Self::Union(sorted_members(members)),
            Self::Intersection(members) => {
                Self::Intersection(members.into_iter().map(Self::sorted).collect())
            }
            Self::Tuple(elements) => Self::Tuple(
                elements
                    .into_iter()
                    .map(|element| TupleElement {
                        kind: element.kind,
                        element: element.element.sorted(),
                    })
                    .collect(),
            ),
            Self::Array(element) => Self::Array(Box::new(element.sorted())),
            Self::Named { name, arguments } => Self::Named {
                name,
                arguments: arguments.into_iter().map(Self::sorted).collect(),
            },
            Self::Predicate {
                subject,
                asserted,
                narrowed,
            } => Self::Predicate {
                subject,
                asserted,
                narrowed: narrowed.map(|narrowed| Box::new(narrowed.sorted())),
            },
            Self::Spelled(spelling) => Self::Spelled(spelling),
        }
    }
}

/// 並べた型を、それぞれ付け替えた形。
/// どれも型の名前だけで書かれているか。**1 つでも読めなければ `None`。**
///
/// 1 つも無ければ `true`（型名でない綴りが残っていない）。
fn all_names_only_types<'a>(structures: impl Iterator<Item = &'a TypeStructure>) -> Option<bool> {
    let mut names_only = true;

    for structure in structures {
        names_only &= structure.names_only_types()?;
    }

    Some(names_only)
}

fn renamed_members(
    members: Vec<TypeStructure>,
    scopes: &mut TypeVariableScopes,
) -> Option<Vec<TypeStructure>> {
    members
        .into_iter()
        .map(|member| member.renamed(scopes))
        .collect()
}

/// 共用体の相手を、それぞれ並びを固定したうえで整列した形。
///
/// **交差型には使わない**（[`TypeStructure::sorted`]）。
fn sorted_members(members: Vec<TypeStructure>) -> Vec<TypeStructure> {
    let mut sorted: Vec<TypeStructure> = members.into_iter().map(TypeStructure::sorted).collect();
    sorted.sort();

    sorted
}

/// 綴りのまま持っている型の中の型変数を付け替えた綴り。型として読めなければ `None`。
///
/// **どこが型名かは構文木が決める**（`syntax::type_spelling`）。綴りを識別子の単位で歩くと、
/// 型変数と同じ綴りのメンバー名や `typeof` の後ろの値の名前まで付け替わる。
fn renamed_spelling(spelling: &str, scopes: &mut TypeVariableScopes) -> Option<String> {
    let mut renamed = String::with_capacity(spelling.len());
    let mut copied = 0;

    for span in substitutable_type_name_spans_of(spelling)? {
        let name = spelling.get(span.clone())?;
        let Some(placeholder) = scopes.placeholder_of(name) else {
            continue;
        };

        renamed.push_str(spelling.get(copied..span.start)?);
        renamed.push_str(&placeholder_spelling(placeholder));
        copied = span.end;
    }
    renamed.push_str(spelling.get(copied..)?);

    Some(renamed)
}

/// 綴りのまま持つ部分の中で束縛された名前（`infer U` / マップ型の `K in …`）を、
/// 返す型名に数えるかどうか。
///
/// **数えるかどうかで倒れる向きが逆になるので、消費する側ごとに選ぶ**
/// （`rules/coding.md`「同じ一覧を、安全な倒れ方が違う 2 箇所で使い回さない」）。
/// 宣言を突き合わせる側は外しすぎると**隠された外側の名前まで落ちて別の記号が重なる**
/// （偽陽性）ので数え、宣言を辿る相手を見る側は数えると**辿る相手が居ない名前を
/// 「尋ねていない」と答える**（偽陰性）ので外す。
#[derive(Clone, Copy)]
enum SpelledBinders {
    /// 型名として数える。
    Counted,
    /// 数えない。
    Skipped,
}

impl SpelledBinders {
    /// その綴りの中で、型名が書かれている範囲。
    fn spans_of(self, spelling: &str) -> Option<Vec<Range<usize>>> {
        match self {
            Self::Counted => type_name_spans_of(spelling),
            Self::Skipped => substitutable_type_name_spans_of(spelling),
        }
    }
}

/// その名前が、積んであるどれかのスコープで型変数として束縛されているか。
///
/// **内側から探す必要は無い。** 知りたいのは束縛されているかどうかだけで、
/// どのスコープが隠しているかは要らない（付け替える [`TypeVariableScopes`] とはそこが違う）。
fn is_bound_in(scopes: &[BTreeSet<String>], name: &str) -> bool {
    scopes.iter().any(|scope| scope.contains(name))
}

/// 付け替え後の型変数の綴り。
fn placeholder_spelling(placeholder: usize) -> String {
    format!("{PLACEHOLDER_PREFIX}{placeholder}")
}

/// 型変数の名前と、付け替え後の番号の対応。**スコープごとに積む。**
///
/// 内側のスコープが同じ名前を宣言していれば、そちらが外側を隠す。
/// 番号はシグネチャ全体で 1 つの通し番号で、**スコープをまたいでも重ならない**。
struct TypeVariableScopes {
    scopes: Vec<Vec<TypeVariableBinding>>,
    /// 次に割り当てる番号。
    next: usize,
}

/// 型変数 1 つ分の束縛。番号は、**その名前が最初に現れたときに決まる**。
struct TypeVariableBinding {
    name: String,
    placeholder: Option<usize>,
}

impl TypeVariableScopes {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            next: 0,
        }
    }

    /// その型変数の宣言でスコープを 1 つ開く。
    fn opened(&mut self, declared: &[TypeParameter]) {
        self.scopes.push(
            declared
                .iter()
                .map(|declared| TypeVariableBinding {
                    name: declared.name.clone(),
                    placeholder: None,
                })
                .collect(),
        );
    }

    /// いちばん内側のスコープを閉じる。
    fn closed(&mut self) {
        self.scopes.pop();
    }

    /// その名前に割り当てた番号。型変数でなければ `None`。
    ///
    /// **まだ割り当てていなければ、ここで割り当てる。** 呼ばれる順が出現順なので、
    /// 割り当てた番号がそのまま出現順になる。
    fn placeholder_of(&mut self, name: &str) -> Option<usize> {
        let found = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(scope, bindings)| {
                bindings
                    .iter()
                    .position(|binding| binding.name == name)
                    .map(|binding| (scope, binding))
            })?;
        let (scope, binding) = found;

        let binding = self.scopes.get_mut(scope)?.get_mut(binding)?;
        if let Some(placeholder) = binding.placeholder {
            return Some(placeholder);
        }

        binding.placeholder = Some(self.next);
        self.next += 1;

        binding.placeholder
    }

    /// いちばん内側のスコープで、まだ割り当てていない型変数へ宣言の順に割り当てる。
    ///
    /// 引数にも戻り値にも現れない型変数がここに来る。
    fn assigned_remaining(&mut self) {
        let Some(bindings) = self.scopes.last_mut() else {
            return;
        };

        for binding in bindings {
            if binding.placeholder.is_some() {
                continue;
            }
            binding.placeholder = Some(self.next);
            self.next += 1;
        }
    }
}

/// その範囲をちょうど覆うノード。無ければ `None`。
///
/// 同じ範囲のノードが縦に重なることがある（`null` は `literal_type` と同じ範囲）ので、
/// **前順で最初に見つかる外側のほう**を採る。
fn spanning_node<'tree>(tree: &'tree SyntaxTree<'_>, span: Range<usize>) -> Option<Node<'tree>> {
    tree.named_descendants()
        .into_iter()
        .find(|node| node.byte_range() == span)
}

/// そのノードが覆う綴り。
fn sliced<'text>(text: &'text str, node: Node<'_>) -> Option<&'text str> {
    text.get(node.byte_range())
}

/// そのノードの最初の名前付きの子。
fn first_named_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor).next()
}

/// そのノードの、その種別の名前付きの子。
fn named_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// そのノードが、その綴りの字句を直下に持つか。
fn holds_token(node: Node<'_>, text: &str, token: &str) -> bool {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .any(|child| !child.is_named() && sliced(text, child) == Some(token))
}

/// そのノードの直下にある、名前付きでない字句の綴り。
fn tokens_of(node: Node<'_>, text: &str) -> Vec<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .filter(|child| !child.is_named())
        .filter_map(|child| sliced(text, child))
        .map(str::to_owned)
        .collect()
}

/// そのノードを型の構造として読む。読めない綴りでは `None`。
fn structured(node: Node<'_>, text: &str) -> Option<TypeStructure> {
    match node.kind() {
        // 括弧は型を変えない。剥がしておかないと、包み方の違いで同じ型が別物になる
        PARENTHESIZED_TYPE_KIND => structured(first_named_child(node)?, text),
        FUNCTION_TYPE_KIND | CONSTRUCTOR_TYPE_KIND => callable_structure(node, text),
        UNION_TYPE_KIND => Some(TypeStructure::Union(flattened_union(structured_children(
            node, text,
        )?))),
        INTERSECTION_TYPE_KIND => Some(TypeStructure::Intersection(flattened_intersection(
            structured_children(node, text)?,
        ))),
        TUPLE_TYPE_KIND => Some(TypeStructure::Tuple(tuple_elements_of(node, text)?)),
        ARRAY_TYPE_KIND => Some(TypeStructure::Array(Box::new(structured(
            first_named_child(node)?,
            text,
        )?))),
        TYPE_IDENTIFIER_KIND | NESTED_TYPE_IDENTIFIER_KIND | PREDEFINED_TYPE_KIND => {
            Some(TypeStructure::Named {
                name: sliced(text, node)?.to_owned(),
                arguments: Vec::new(),
            })
        }
        GENERIC_TYPE_KIND => generic_structure(node, text),
        _ => Some(TypeStructure::Spelled(sliced(text, node)?.to_owned())),
    }
}

/// 共用体の中の共用体を、1 つの並びへ均した形。
///
/// **`|` は結合の仕方で型を変えない。** `A | (B | C)` と `(A | B) | C` は同じ型だが、
/// 入れ子のまま持つと**開いた綴りと書き下した綴りが別物になる**
/// （`type Scalar = string | number` を `Scalar | null` の位置へ開くと
/// `(string | number) | null` になり、`string | number | null` と重ならない）。
///
/// 並べ替え（[`sorted_members`]）では均せない。**入れ子の深さは並びではない。**
fn flattened_union(members: Vec<TypeStructure>) -> Vec<TypeStructure> {
    members
        .into_iter()
        .flat_map(|member| match member {
            TypeStructure::Union(nested) => nested,
            member => vec![member],
        })
        .collect()
}

/// 交差型の中の交差型を、1 つの並びへ均した形。[`flattened_union`] と同じ理由。
///
/// **均しても書かれた順は変わらない。** 交差型の並びはオーバーロードの並びなので、
/// 入れ子を開くだけにして並べ替えない（[`TypeStructure::sorted`]）。
fn flattened_intersection(members: Vec<TypeStructure>) -> Vec<TypeStructure> {
    members
        .into_iter()
        .flat_map(|member| match member {
            TypeStructure::Intersection(nested) => nested,
            member => vec![member],
        })
        .collect()
}

/// そのノードの名前付きの子を、それぞれ型の構造として読む。
fn structured_children(node: Node<'_>, text: &str) -> Option<Vec<TypeStructure>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .map(|child| structured(child, text))
        .collect()
}

/// 型引数を伴う型名を読む。
fn generic_structure(node: Node<'_>, text: &str) -> Option<TypeStructure> {
    let name = first_named_child(node)?;
    let arguments = match named_child_of_kind(node, TYPE_ARGUMENTS_KIND) {
        Some(arguments) => structured_children(arguments, text)?,
        None => Vec::new(),
    };

    Some(TypeStructure::Named {
        name: sliced(text, name)?.to_owned(),
        arguments,
    })
}

/// 呼べる型を読む。
///
/// **一覧に無い修飾子が付いた構築型は綴りのまま持つ。** `abstract new` を落とすと
/// `new` との違いが消え、**代入できない 2 つを重ねてしまう**（偽陽性）。
fn callable_structure(node: Node<'_>, text: &str) -> Option<TypeStructure> {
    let kind = match node.kind() {
        CONSTRUCTOR_TYPE_KIND => SignatureKind::Construct,
        _ => SignatureKind::Call,
    };
    if holds_token(node, text, ABSTRACT_MODIFIER) {
        return Some(TypeStructure::Spelled(sliced(text, node)?.to_owned()));
    }

    let mut type_parameters = Vec::new();
    let mut written = ParameterList::default();
    let mut returned = None;

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            TYPE_PARAMETERS_KIND => type_parameters = declared_type_parameters_of(child, text)?,
            FORMAL_PARAMETERS_KIND => written = ParameterList::from_node(child, text)?,
            _ => returned = Some(child),
        }
    }

    let return_type = returned_structure(returned?, text, &written.names)?;

    Some(TypeStructure::Callable(Callable {
        kind,
        type_parameters,
        parameters: written.parameters,
        return_type: Box::new(return_type),
    }))
}

/// 引数リストから読んだ、渡し方と型の並び。
///
/// **書かれた名前も一緒に持つ。** 型述語の主語がどの引数を指しているかを決めるのに要る
/// （[`returned_structure`]）。名前そのものは [`Parameter`] に残らない。
#[derive(Default)]
struct ParameterList {
    names: Vec<String>,
    parameters: Vec<Parameter>,
}

impl ParameterList {
    /// 引数リストのノードから読む。型注釈の無い引数があれば `None`。
    fn from_node(node: Node<'_>, text: &str) -> Option<Self> {
        let mut names = Vec::new();
        let mut parameters = Vec::new();

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let written = first_named_child(child)?;
            let rest = written.kind() == REST_PATTERN_KIND;
            let name = match rest {
                true => first_named_child(written)?,
                false => written,
            };
            let annotation = named_child_of_kind(child, TYPE_ANNOTATION_KIND)?;

            names.push(sliced(text, name)?.to_owned());
            parameters.push(Parameter {
                kind: ParameterKind::of(child.kind(), name.kind(), rest),
                annotated_type: structured(first_named_child(annotation)?, text)?,
            });
        }

        Some(Self { names, parameters })
    }
}

impl ParameterKind {
    /// 引数のノードの種別・名前のノードの種別・可変長かどうかから決める。
    fn of(parameter: &str, name: &str, rest: bool) -> Self {
        if rest {
            return Self::Rest;
        }
        if name == THIS_KIND {
            return Self::Receiver;
        }
        if parameter == OPTIONAL_PARAMETER_KIND {
            return Self::Optional;
        }
        Self::Required
    }
}

/// タプルの要素を読む。
fn tuple_elements_of(node: Node<'_>, text: &str) -> Option<Vec<TupleElement>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .map(|child| match child.kind() {
            OPTIONAL_TYPE_KIND => Some(TupleElement {
                kind: TupleElementKind::Optional,
                element: structured(first_named_child(child)?, text)?,
            }),
            REST_TYPE_KIND => Some(TupleElement {
                kind: TupleElementKind::Rest,
                element: structured(first_named_child(child)?, text)?,
            }),
            REQUIRED_PARAMETER_KIND | OPTIONAL_PARAMETER_KIND => labelled_element_of(child, text),
            _ => Some(TupleElement {
                kind: TupleElementKind::Required,
                element: structured(child, text)?,
            }),
        })
        .collect()
}

/// ラベルの付いたタプルの要素を読む。**ラベルは落とす**（型を変えない）。
fn labelled_element_of(node: Node<'_>, text: &str) -> Option<TupleElement> {
    let written = first_named_child(node)?;
    let rest = written.kind() == REST_PATTERN_KIND;
    let annotation = named_child_of_kind(node, TYPE_ANNOTATION_KIND)?;

    let kind = match (rest, node.kind()) {
        (true, _) => TupleElementKind::Rest,
        (false, OPTIONAL_PARAMETER_KIND) => TupleElementKind::Optional,
        (false, _) => TupleElementKind::Required,
    };

    Some(TupleElement {
        kind,
        element: structured(first_named_child(annotation)?, text)?,
    })
}

/// 型変数の宣言を読む。
fn declared_type_parameters_of(node: Node<'_>, text: &str) -> Option<Vec<TypeParameter>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .map(|child| {
            let annotated = |kind: &str| match named_child_of_kind(child, kind) {
                Some(node) => structured(first_named_child(node)?, text).map(Some),
                None => Some(None),
            };

            Some(TypeParameter {
                modifiers: tokens_of(child, text),
                name: sliced(text, first_named_child(child)?)?.to_owned(),
                constraint: annotated(CONSTRAINT_KIND)?,
                default: annotated(DEFAULT_TYPE_KIND)?,
            })
        })
        .collect()
}

/// 戻り値の位置に書かれたものを読む。型述語なら主語を引数の位置へ直す。
///
/// **引数を指していない主語（`this is Foo`）は綴りのまま持つ。** 位置に直せないので、
/// 直せたことにすると**書かれていない対応**を作ることになる。綴りで比べれば
/// 倒れる向きは偽陰性になる。
fn returned_structure(node: Node<'_>, text: &str, names: &[String]) -> Option<TypeStructure> {
    let (asserted, predicate) = match node.kind() {
        ASSERTS_KIND => (true, first_named_child(node)?),
        _ => (false, node),
    };
    let names_a_predicate = asserted || predicate.kind() == TYPE_PREDICATE_KIND;
    if !names_a_predicate {
        return structured(node, text);
    }

    // `asserts value` は主語だけを持ち、絞り込む先を書かない
    let (subject, narrowed) = match predicate.kind() {
        TYPE_PREDICATE_KIND => {
            let mut cursor = predicate.walk();
            let mut children = predicate.named_children(&mut cursor);
            (children.next()?, children.next())
        }
        _ => (predicate, None),
    };

    let written = sliced(text, subject)?;
    let Some(subject) = names.iter().position(|name| name == written) else {
        return Some(TypeStructure::Spelled(sliced(text, node)?.to_owned()));
    };

    Some(TypeStructure::Predicate {
        subject,
        asserted,
        narrowed: match narrowed {
            Some(narrowed) => Some(Box::new(structured(narrowed, text)?)),
            None => None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストが渡す綴りは呼べる型として読み取れる前提で、正規化まで済ませる。
    fn normalized(spelling: &str) -> Callable {
        Callable::from_spelling(spelling)
            .expect("テストが渡す綴りは呼べる型として読み取れる")
            .normalized()
            .expect("テストが渡す綴りは正規化できる")
    }

    /// 2 つの綴りが同じ構造になるか。
    fn same_structure(one: &str, other: &str) -> bool {
        normalized(one) == normalized(other)
    }

    /// テストが渡す綴りは型 1 つ分として読み取れる前提で、正規化まで済ませる。
    fn normalized_type(spelling: &str) -> TypeStructure {
        TypeStructure::from_spelling(spelling)
            .expect("テストが渡す綴りは型 1 つ分として読み取れる")
            .normalized()
            .expect("テストが渡す綴りは正規化できる")
    }

    #[test]
    fn test_a_callable_type_differing_only_in_parameter_names_reads_as_the_same_structure() {
        assert!(same_structure(
            "(reason: string) => void",
            "(message: string) => void"
        ));
    }

    #[test]
    fn test_a_callable_type_differing_in_a_parameter_type_reads_as_a_different_structure() {
        // 対照。名前を落としても型の違いは残る
        assert!(!same_structure(
            "(reason: string) => void",
            "(reason: number) => void"
        ));
    }

    #[test]
    fn test_a_parameter_missing_its_type_annotation_cannot_be_read() {
        assert_eq!(Callable::from_spelling("(a) => void"), None);
    }

    #[test]
    fn test_a_spelling_that_is_not_a_callable_type_cannot_be_read() {
        assert_eq!(Callable::from_spelling("string | number"), None);
    }

    #[test]
    fn test_a_spelling_that_is_not_a_callable_type_still_reads_as_a_type_structure() {
        // 対照は 1 つ上のテスト。呼べる型でない綴りも型 1 つ分としては読めるので、
        // **書かれ方の違い（共用体の並び）はここでも落ちる**
        assert_eq!(
            normalized_type("string | number"),
            normalized_type("number | string")
        );
    }

    #[test]
    fn test_a_type_name_written_alone_is_not_renamed_like_a_type_variable() {
        // 型 1 つ分の綴りの外側に型変数の宣言は無い。付け替えると、
        // **どこかで宣言された別々の型名が同じ `%0` になる**
        assert_ne!(normalized_type("Amount"), normalized_type("Total"));
    }

    #[test]
    fn test_a_type_variable_declared_inside_a_standalone_type_is_renamed() {
        // 対照は 1 つ上のテスト。入れ子の呼べる型が自分で宣言した名前は付け替わる
        assert_eq!(
            normalized_type("(<T>(value: T) => T)[]"),
            normalized_type("(<U>(other: U) => U)[]")
        );
    }

    #[test]
    fn test_a_standalone_type_that_does_not_read_as_a_type_cannot_be_read() {
        assert_eq!(TypeStructure::from_spelling("(a: ) => void"), None);
    }

    #[test]
    fn test_a_spelling_that_does_not_read_as_a_type_cannot_be_read() {
        assert_eq!(Callable::from_spelling("(a: ) => void"), None);
    }

    #[test]
    fn test_a_callable_type_wrapped_in_parentheses_reads_as_the_structure_it_wraps() {
        // 括弧は型を変えない。剥がさないと包み方の違いで同じ型が別物になる
        assert!(same_structure(
            "(cb: ((reason: string) => void)) => void",
            "(fn: (message: string) => void) => void"
        ));
    }

    #[test]
    fn test_a_callable_type_inside_a_union_drops_its_parameter_names() {
        assert!(same_structure(
            "(cb: ((reason: string) => void) | null) => void",
            "(handler: ((message: string) => void) | null) => void"
        ));
    }

    #[test]
    fn test_a_union_written_in_the_other_order_reads_as_the_same_structure() {
        assert!(same_structure(
            "(x: string | number) => void",
            "(x: number | string) => void"
        ));
    }

    #[test]
    fn test_a_union_holding_another_member_reads_as_a_different_structure() {
        // 対照。並べ替えても members が違えば重ならない
        assert!(!same_structure(
            "(x: string | number) => void",
            "(x: string | boolean) => void"
        ));
    }

    #[test]
    fn test_a_union_nested_on_the_left_reads_as_the_flat_structure() {
        assert!(same_structure(
            "(x: (string | number) | null) => void",
            "(x: string | number | null) => void"
        ));
    }

    #[test]
    fn test_a_union_nested_on_the_right_reads_as_the_flat_structure() {
        // 並べ替えでは均せない向き。`null | (string | number)` は入れ子の深さが違う
        assert!(same_structure(
            "(x: null | (string | number)) => void",
            "(x: null | string | number) => void"
        ));
    }

    #[test]
    fn test_an_intersection_written_in_the_other_order_reads_as_a_different_structure() {
        // 交差型が並べる呼べる型はオーバーロードの並びそのもの。
        // `((a: string) => string) & ((a: number) => number)` を逆順にすると、
        // 同じ引数を渡した呼び出しが別のシグネチャへ解決する
        assert!(!same_structure(
            "(x: ((a: string) => string) & ((a: number) => number)) => void",
            "(x: ((a: number) => number) & ((a: string) => string)) => void"
        ));
    }

    #[test]
    fn test_an_intersection_written_in_the_same_order_reads_as_the_same_structure() {
        // 対照。並びが同じなら、中の引数名は落ちる
        assert!(same_structure(
            "(x: Left & ((a: string) => void)) => void",
            "(y: Left & ((b: string) => void)) => void"
        ));
    }

    #[test]
    fn test_a_labelled_tuple_reads_as_the_same_structure_as_the_unlabelled_one() {
        assert!(same_structure(
            "(pair: [left: string, right: number]) => void",
            "(pair: [string, number]) => void"
        ));
    }

    #[test]
    fn test_a_tuple_written_in_the_other_order_reads_as_a_different_structure() {
        // 対照。タプルの並びは型の一部なので、共用体のようには並べ替えない
        assert!(!same_structure(
            "(pair: [string, number]) => void",
            "(pair: [number, string]) => void"
        ));
    }

    #[test]
    fn test_a_labelled_optional_tuple_element_keeps_being_optional() {
        assert!(same_structure(
            "(pair: [left?: string]) => void",
            "(pair: [string?]) => void"
        ));
    }

    #[test]
    fn test_a_labelled_rest_tuple_element_keeps_being_variadic() {
        assert!(same_structure(
            "(pair: [...rest: string[]]) => void",
            "(pair: [...string[]]) => void"
        ));
    }

    #[test]
    fn test_an_optional_tuple_element_reads_as_a_different_structure_from_a_required_one() {
        // 対照。ラベルを落としても省けるかどうかは残る
        assert!(!same_structure(
            "(pair: [left?: string]) => void",
            "(pair: [left: string]) => void"
        ));
    }

    #[test]
    fn test_a_type_predicate_naming_another_parameter_reads_as_the_same_structure() {
        assert!(same_structure(
            "(value: unknown) => value is string",
            "(input: unknown) => input is string"
        ));
    }

    #[test]
    fn test_a_type_predicate_naming_a_different_parameter_reads_as_a_different_structure() {
        // 対照。主語を位置で持つので、どの引数を絞り込むかの違いは残る
        assert!(!same_structure(
            "(first: unknown, second: unknown) => first is string",
            "(first: unknown, second: unknown) => second is string"
        ));
    }

    #[test]
    fn test_an_asserting_type_predicate_reads_as_a_different_structure_from_a_plain_one() {
        assert!(!same_structure(
            "(value: unknown) => asserts value is string",
            "(value: unknown) => value is string"
        ));
    }

    #[test]
    fn test_a_type_predicate_naming_no_parameter_is_compared_by_its_spelling() {
        // 位置に直せない主語は綴りのまま。同じ綴りなら重なる
        assert!(same_structure(
            "(x: number) => this is Shape",
            "(y: number) => this is Shape"
        ));
    }

    #[test]
    fn test_a_type_predicate_naming_no_parameter_keeps_what_it_narrows_to() {
        // 対照。綴りのまま比べるので、絞り込む先が違えば重ならない
        assert!(!same_structure(
            "(x: number) => this is Shape",
            "(x: number) => this is Circle"
        ));
    }

    #[test]
    fn test_a_type_variable_declared_with_a_modifier_is_renamed_like_a_plain_one() {
        assert!(same_structure(
            "<const T>(x: T) => T",
            "<const U>(x: U) => U"
        ));
    }

    #[test]
    fn test_a_type_variable_declared_with_a_modifier_reads_as_a_different_structure_from_a_plain_one()
     {
        // 対照。修飾子は綴りのまま持つので、付いている側と付いていない側は分かれる
        assert!(!same_structure("<const T>(x: T) => T", "<T>(x: T) => T"));
    }

    #[test]
    fn test_a_type_variable_declared_by_a_nested_callable_type_is_renamed() {
        assert!(same_structure(
            "(cb: <T>(value: T) => T) => void",
            "(fn: <U>(input: U) => U) => void"
        ));
    }

    #[test]
    fn test_a_nested_type_variable_is_not_confused_with_the_enclosing_one() {
        // 内側で `%0` を振り直すと、この 2 つがどちらも「内側 %0・戻り値 %0」になる
        assert!(!same_structure(
            "<T>(cb: <U>(u: U) => T) => void",
            "<T>(cb: <U>(u: U) => U) => void"
        ));
    }

    #[test]
    fn test_type_variables_are_renamed_in_the_order_they_occur() {
        assert!(same_structure(
            "<T, U>(a: U, b: T) => void",
            "<A, B>(a: A, b: B) => void"
        ));
    }

    #[test]
    fn test_a_type_variable_occurring_in_a_union_is_renamed_before_the_members_are_sorted() {
        // 並べ替えを先にすると、付け替え前の名前で整列して別の並びになる
        assert!(same_structure(
            "<A>(x: A | Middle) => void",
            "<Z>(x: Z | Middle) => void"
        ));
    }

    #[test]
    fn test_a_type_variable_inside_an_array_type_is_renamed() {
        assert!(same_structure("<T>(x: T[]) => void", "<U>(x: U[]) => void"));
    }

    #[test]
    fn test_a_callable_type_passed_as_a_type_argument_drops_its_parameter_names() {
        assert!(same_structure(
            "(x: Array<(a: string) => void>) => void",
            "(y: Array<(b: string) => void>) => void"
        ));
    }

    #[test]
    fn test_a_constructor_type_reads_as_a_different_structure_from_a_function_type() {
        assert!(!same_structure(
            "new (a: string) => Shape",
            "(a: string) => Shape"
        ));
    }

    #[test]
    fn test_an_abstract_constructor_type_reads_as_a_different_structure_from_a_plain_one() {
        // 一覧に無い修飾子を落とすと、代入できない 2 つが重なる
        assert!(!same_structure(
            "(make: abstract new (a: string) => Shape) => void",
            "(make: new (a: string) => Shape) => void"
        ));
    }

    #[test]
    fn test_a_type_that_is_not_taken_apart_is_compared_by_its_spelling() {
        assert!(same_structure(
            "(x: { a: string }) => void",
            "(y: { a: string }) => void"
        ));
    }

    #[test]
    fn test_a_type_that_is_not_taken_apart_keeps_the_difference_in_its_spelling() {
        // 対照。綴りで比べるので、分解しない形でも違いは残る
        assert!(!same_structure(
            "(x: { a: string }) => void",
            "(x: { b: string }) => void"
        ));
    }

    #[test]
    fn test_a_type_variable_inside_a_type_that_is_not_taken_apart_is_renamed() {
        assert!(same_structure(
            "<T>(x: { value: T }) => void",
            "<U>(x: { value: U }) => void"
        ));
    }

    #[test]
    fn test_a_member_name_matching_a_type_variable_is_not_renamed() {
        // 付け替えがプロパティ名まで届くと、この 2 つが重なる（偽陽性）
        assert!(!same_structure(
            "<T>(x: { T: string; value: T }) => void",
            "<U>(x: { U: string; value: U }) => void"
        ));
    }

    #[test]
    fn test_a_value_name_after_typeof_matching_a_type_variable_is_not_renamed() {
        // `typeof T` の `T` は値の名前。TypeScript は型と値で名前空間が別なので、
        // 付け替えがそこまで届くと、この 2 つが重なる（偽陽性）
        assert!(!same_structure(
            "<T>(x: T, y: typeof T) => void",
            "<U>(x: U, y: typeof U) => void"
        ));
    }

    #[test]
    fn test_a_callable_type_keeping_a_value_name_does_not_name_only_types() {
        // `typeof localValue` はどのファイルで書かれたかで指す先が変わるのに、
        // 型名のノードにならないので辿る位置を作れない
        let read = Callable::from_spelling("() => typeof localValue")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(read.names_only_types(), Some(false));
    }

    #[test]
    fn test_a_callable_type_burying_a_value_name_in_a_type_argument_does_not_name_only_types() {
        // 分解できた形の中にも綴りのまま持つ部分は残る。外側だけを見ると取りこぼす
        let read = Callable::from_spelling("() => Array<typeof localValue>")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(read.names_only_types(), Some(false));
    }

    #[test]
    fn test_a_callable_type_written_with_names_alone_names_only_types() {
        // 対照。綴りのまま持つ部分があっても、型の名前だけで書かれていれば
        // どこで書かれていても同じ型を指す
        let read = Callable::from_spelling("(a: { id: string }) => Local")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(read.names_only_types(), Some(true));
    }

    #[test]
    fn test_the_traceable_type_names_of_a_callable_type_leave_out_a_name_bound_by_infer() {
        // 条件型は綴りのまま持つので、`infer U` の `U` は綴りの中で束縛されている。
        // 辿る相手が居ないのは型変数と同じで、数えると**注釈を書いてある
        // ジェネリック関数まで測れない側へ落ちる**
        let read = Callable::from_spelling("<T>(x: T) => T extends Promise<infer U> ? U : never")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.traceable_type_names(),
            Some(["Promise"].into_iter().map(str::to_owned).collect())
        );
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_hold_a_name_bound_by_infer() {
        // 対照は上のテスト。**宣言を突き合わせる側は外さない。** 外しすぎると、
        // 同じ綴りの束縛に隠された外側の名前まで落ちて別の記号が重なる（偽陽性）
        let read = Callable::from_spelling("<T>(x: T) => T extends Promise<infer U> ? U : never")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.type_names(),
            Some(["Promise", "U"].into_iter().map(str::to_owned).collect())
        );
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_leave_out_the_type_variables_it_uses() {
        // 束縛された型変数は宣言を辿る相手ではない。残すと、尋ねていない型名と
        // 見分けが付かなくなる（`semantics::type_signature`）
        let read = Callable::from_spelling("<T>(x: T) => T")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(read.type_names(), Some(BTreeSet::new()));
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_leave_out_the_type_variables_of_a_nested_callable() {
        // キーワードの型は残る（落とすのは `semantics::type_signature` の側）
        let read = Callable::from_spelling("<T>(cb: <U>(u: U) => T) => void")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.type_names(),
            Some(["void"].into_iter().map(str::to_owned).collect())
        );
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_keep_a_type_variable_bound_outside_it() {
        // 外側の関数が束縛した型変数は、このシグネチャからは辿れない。落とすと
        // 別のファイルの同じ綴りと重なる（偽陽性）
        let read = Callable::from_spelling("(x: number) => Boxed<R>")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.type_names(),
            Some(
                ["Boxed", "R", "number"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_leave_out_the_declared_type_variables() {
        let read = Callable::from_spelling("<T extends Amount>(x: Boxed) => Total")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.type_names(),
            Some(
                ["Amount", "Boxed", "Total"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
    }

    #[test]
    fn test_the_type_names_of_a_callable_type_hold_a_name_written_inside_a_spelled_type() {
        let read = Callable::from_spelling("(x: { value: Amount }) => Total")
            .expect("テストが渡す綴りは呼べる型として読み取れる");

        assert_eq!(
            read.type_names(),
            Some(["Amount", "Total"].into_iter().map(str::to_owned).collect())
        );
    }
}
