//! hover が返した綴りを、単一化の可否を比べられる形へ直す。
//!
//! 綴りそのまま（`signature text`）と、正規化した形（`type signature`）を分けている
//! (`rules/naming.md`「このツールの語彙を固定する」)。**綴りのまま比べると、
//! 引数名や型変数名が違うだけのペアが別物になる。**
//!
//! **綴りを直す部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! サーバに尋ねるのは [`type_signature_outcome_of`] だけで、そこは
//! `tests/semantics.rs` が実サーバで見る。

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::lsp::{ClientError, HoverOutcome, Session, SourceDocument};
use crate::semantics::resolved_type::ResolvedTypes;
use crate::source_position::SourcePosition;

/// 付け替えた型変数の綴りの前置き。
///
/// `%` は識別子に使えない文字なので、**元の型名と衝突しない**。付け替えた後の綴りを
/// もう一度付け替えてしまうこともない。
const PLACEHOLDER_PREFIX: char = '%';

/// 型変数の制約を導く語。前後の空白ごと見て、`extendsFoo` のような名前と分ける。
const CONSTRAINT_KEYWORD: &str = " extends ";

/// 型変数の既定の型を導く印。
///
/// 前後の空白ごと見るのは、関数型の `=>` と分けるため（そちらは `=` の後ろが `>`）。
const DEFAULT_MARKER: &str = " = ";

/// 呼び出し時に渡さない引数（TypeScript の `this` 引数）の名前。
const RECEIVER_PARAMETER: &str = "this";

/// 構築シグネチャの宣言形を導く語（`constructor Result(value: string): Result`）。
const CONSTRUCTOR_KEYWORD: &str = "constructor";

/// 構築シグネチャの値形を導く語（`new (value: string) => Result`）。
const NEW_KEYWORD: &str = "new";

/// 括弧で括られていなければ、周りの印に負ける演算子。
///
/// 関数型の `=>` は、深さ 0 の `=` として現れる（`>` は矢印の一部なので
/// [`SignatureScan`] が閉じ括弧として数えない）。
const UNGROUPED_TYPE_OPERATORS: [char; 3] = ['|', '&', '='];

/// 型の手前に来る区切り。
///
/// `>` は関数型の矢印（`(a: T) => U` の `U` の手前）、`[` はタプルの始まり。
/// **型名の直後の `[` は配列の印**なので、後ろの一覧（[`TYPE_BOUNDARY_AFTER`]）には入れない。
const TYPE_BOUNDARY_BEFORE: [char; 6] = [':', ',', '(', '<', '>', '['];

/// 型の後ろに来る区切り。
const TYPE_BOUNDARY_AFTER: [char; 6] = [',', ')', '>', ';', '}', ']'];

/// メンバーや引数の名前の後ろに続く印。
///
/// 型の綴りでは、名前の後ろにだけこれらが続く。`(` と `<` はメソッドの名前
/// （`{ ID(): X }` / `{ ID<T>(): X }`）、`?` の付く形は省略できるメンバー。
///
/// **`?` だけでは足りない。** 条件型（`T extends U ? X : Y`）の `?` と見分けが付かない。
///
/// **`<` が型名の後ろに来るのは総称型（`Box<User>`）だけ**で、そのときは型引数を取る
/// エイリアスなので開く対象に入っていない（[`ResolvedTypes`] へ入らない）。
const MEMBER_MARKERS: [&str; 5] = [":", "?:", "(", "?(", "<"];

/// 後ろに値の名前を取る演算子。
///
/// `typeof x` の `x` は**値の名前**で、型の名前ではない。同じ綴りの型エイリアスがあると
/// 差し替えが `typeof string` という綴りを作る（TypeScript では型と値が別の名前空間なので、
/// 同じ綴りが両方にありうる）。
const VALUE_OPERATOR: &str = "typeof";

/// どこで書かれても同じ型を指す綴り。
///
/// TypeScript の文法が持つ組み込みの型で、**宣言を辿らずに意味が決まる**。
/// tree-sitter が `predefined_type` として扱う一覧と同じもの。
const PREDEFINED_TYPES: [&str; 12] = [
    "any",
    "bigint",
    "boolean",
    "never",
    "null",
    "number",
    "object",
    "string",
    "symbol",
    "undefined",
    "unknown",
    "void",
];

/// 型の綴りの中で、型名ではなく型の作り方を表す語。
///
/// **一覧から漏れた語は型名として扱われる。** 漏れると差し込みを見送るだけなので、
/// 倒れる向きは偽陰性になる（[`is_site_independent`] がこの向きを保つ）。
const TYPE_OPERATORS: [&str; 5] = ["keyof", "readonly", "infer", "extends", VALUE_OPERATOR];

/// 識別子として読まれるリテラル型。
///
/// リテラル型のうち、**綴りが識別子の形をしているのはこの 2 つだけ**。文字列リテラル型
/// (`"a"`) と数値リテラル型 (`42`) は識別子として読まれないので、ここに並べる相手にならない
/// （[`is_site_independent`] は識別子だけを見る）。
const BOOLEAN_LITERALS: [&str; 2] = ["true", "false"];

/// 綴りを検証するときに、メンバーや引数の名前と見なす印。
///
/// [`MEMBER_MARKERS`] から `<` を外したもの。**差し込む側と検証する側で `<` の意味が違う**。
/// 差し込む先の綴りに `X<…>` が現れるのはメソッド名だけだが（型引数を取るエイリアスは
/// 開く対象に入らない）、**検証する相手は開いた後の綴り**で、そこには総称型の参照
/// (`Local<string>`) が現れる。それは宣言を辿る相手なので、見逃すと**別々のモジュールの
/// `Local<string>` が同じ綴りとして重なる**。
///
/// **Why not（型引数の中身を読んで見分ける）**: メソッド名か総称型かは `<` の後ろが
/// 型変数の宣言かどうかで決まり、読むには型そのものの構文解析が要る。
/// 取りこぼす側（差し込みを見送る = 偽陰性）に倒した。
const VETTED_MEMBER_MARKERS: [&str; 4] = [":", "?:", "(", "?("];

/// サーバに型シグネチャを尋ねた結果。
///
/// **「取れなかった」を 1 つにまとめない。** どれなのかで**利用者が次に試すことが違う**
/// （サーバを替える / そのチャンクを諦める / dryguard 側の穴）
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSignatureOutcome {
    /// 綴りを正規化できた。
    Normalized(TypeSignature),
    /// サーバがその位置に型を持たなかった。
    NoTypeThere,
    /// hover の応答を `lsp` が読めなかった。
    UnreadableHover,
    /// 綴りは返ったが、[`TypeSignature::from_signature_text`] が読み解けなかった。
    UnreadableSignature,
    /// サーバが hover を提供していない。
    HoverNotProvided,
    /// サーバが typeDefinition を提供していないので、型名を 1 つも開けなかった。
    ///
    /// **綴りのまま比べた結果を出さない。** 開けていれば重なったかもしれないので、
    /// 「単一化不能」として出すと確かめられなかったことを答えにしてしまう。
    TypeDefinitionNotProvided,
    /// 宣言の場所は返ったが、`lsp` がパスとして読めなかった。
    ///
    /// **サーバは宣言を持っている。** 読めないのはこちら側の穴なので、宣言が無いのとは
    /// 分けて出す。
    UnreadableTypeDefinition,
}

/// その位置にある名前の型を尋ねて、正規化した形にする。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`position` は
/// `Chunk::name_position` が指す識別子の位置。`resolved` は
/// `semantics::resolved_type` が集めた型エイリアスの右辺。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
/// **答えが無い / 読めないは `Err` にしない**（会話は成立しているので、
/// シグナルが取れなかっただけ）。
pub fn type_signature_outcome_of(
    session: &mut Session,
    document: &SourceDocument,
    position: SourcePosition,
    resolved: &ResolvedTypes,
) -> Result<TypeSignatureOutcome, ClientError> {
    let outcome = match session.hover(document, position)? {
        HoverOutcome::Answered(signature_text) => normalized_outcome_of(&signature_text, resolved),
        HoverOutcome::NoAnswer => TypeSignatureOutcome::NoTypeThere,
        HoverOutcome::Unreadable => TypeSignatureOutcome::UnreadableHover,
        HoverOutcome::NotSupported => TypeSignatureOutcome::HoverNotProvided,
    };

    Ok(outcome)
}

/// 綴りを正規化した結果。読み解けなければ、その旨。
fn normalized_outcome_of(signature_text: &str, resolved: &ResolvedTypes) -> TypeSignatureOutcome {
    let Some(signature) = TypeSignature::from_signature_text(signature_text, resolved) else {
        return TypeSignatureOutcome::UnreadableSignature;
    };

    TypeSignatureOutcome::Normalized(signature)
}

/// 単一化の可否を比べられる形に直した型シグネチャ。
///
/// 引数名を落とし、型変数を出現順に付け替えてある。**同じ形になった 2 つは
/// 単一化できる**（[`TypeSignature::is_unifiable_with`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSignature {
    /// 呼び出しの仕方。
    kind: SignatureKind,
    /// 型変数。付け替え後の並び。
    type_parameters: Vec<TypeParameter>,
    /// 引数。名前を落とし、渡し方と型だけが残る。
    parameters: Vec<Parameter>,
    /// 戻り値の型。
    return_type: String,
}

impl TypeSignature {
    /// hover が返した綴りから組み立てる。
    ///
    /// `text` は正規化前の綴り（`lsp` の `Session::hover` が返すもの）。サーバごとに
    /// 宣言形（`function decl(a: string): number`）と値形
    /// （`const arrow: (a: string) => number`）の 2 通りがあり、どちらも同じ形へ直す。
    /// `resolved` は型エイリアスの右辺で、綴りを読む前に差し込む。
    ///
    /// 引数リストと戻り値の型を読み取れない綴りでは `None`。**空の引数リストで
    /// 埋めない**ので、後段は「引数が無い関数」と「読めなかった」を区別できる
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    ///
    /// **差し込むのは読む前。** シグネチャ全体がエイリアスに置き換わる形
    /// （`const aliased: Handler`）は引数リストを持たないので、読んだ後に差し込む形では
    /// **この綴りが入口に入れない**（`None` になる）。
    pub fn from_signature_text(text: &str, resolved: &ResolvedTypes) -> Option<Self> {
        let flattened = flattened(&substituted(text, resolved));
        let split = SplitSignature::from_signature_text(&flattened)?;

        let return_type = normalized_type(split.return_type()?)?;
        let parameters = split.parameters()?;
        let declared = split.declared_type_parameters();

        let placeholders = Placeholders::of(
            &declared,
            parameters
                .iter()
                .map(Parameter::annotated_type)
                .chain(std::iter::once(return_type.as_str())),
        );

        Some(Self {
            kind: split.kind(),
            type_parameters: placeholders.type_parameters(&declared),
            parameters: parameters
                .iter()
                .map(|parameter| parameter.renamed(&placeholders))
                .collect(),
            return_type: placeholders.renamed(&return_type),
        })
    }

    /// 2 つの型シグネチャが同じ型構造に重なるか。
    ///
    /// 引数名と型変数名の違いは正規化の時点で消えているので、ここでは形が同じかを見る。
    /// 呼び出しの仕方・引数の渡し方と型・制約・型変数の既定の型が違えば重ならない。
    pub fn is_unifiable_with(&self, other: &Self) -> bool {
        self == other
    }
}

/// 呼び出しの仕方。
///
/// **`new` を付けて呼ぶ型と、そのまま呼ぶ型は別の型**なので、同じ形にしない。
/// クラスのコンストラクタもチャンクになる（tree-sitter では `method_definition`）ので、
/// 落とすと `constructor Result(value: string): Result` と
/// `function create(value: string): Result` が単一化可能になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureKind {
    /// そのまま呼ぶ（`f(a)`）。
    Call,
    /// `new` を付けて呼ぶ（`new C(a)`）。
    Construct,
}

impl SignatureKind {
    /// 引数リストの手前の綴りから読む。
    ///
    /// hover は構築シグネチャを、宣言形なら `constructor Result(value: string): Result`、
    /// 値形なら `new (value: string) => Result` と返す（実測）。
    ///
    /// **Why（接頭辞を見てよい理由）**: ここで見分けたいのは「`new` が要るか」の 2 択で、
    /// 綴りはこの 2 つで尽きる。`(method)` / `(property)` のように**この先増える一覧では
    /// ない**ので、[`SplitSignature`] が接頭辞を列挙しない判断とは別の話になる。
    fn from_prefix(prefix: &str) -> Self {
        let prefix = prefix.trim();
        let declared = prefix.split_whitespace().next() == Some(CONSTRUCTOR_KEYWORD);
        let annotated = prefix.split_whitespace().next_back() == Some(NEW_KEYWORD);

        if declared || annotated {
            return Self::Construct;
        }
        Self::Call
    }
}

/// 引数 1 つ分。名前を落とし、**渡し方と型だけ**を残した形。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Parameter {
    kind: ParameterKind,
    annotated_type: String,
}

impl Parameter {
    /// 引数リストの中の 1 つ分から読む。型注釈が無ければ `None`。
    fn from_text(parameter: &str) -> Option<Self> {
        let parameter = parameter.trim();
        let (rest, named) = match parameter.strip_prefix(ParameterKind::REST_MARKER) {
            Some(named) => (true, named),
            None => (false, parameter),
        };

        // 分割は深さ 0 の `:` で行う。分割代入の引数（`{ a: b }: Shape`）は
        // 名前の側にも `:` を持つ。
        let separator = SignatureScan::new(named).top_level_index_of(':')?;
        let name = named.get(..separator)?.trim_end();
        let annotated = named.get(separator + 1..)?.trim();

        if annotated.is_empty() {
            return None;
        }

        Some(Self {
            kind: ParameterKind::of(name, rest),
            annotated_type: normalized_type(annotated)?,
        })
    }

    /// この引数の型。
    fn annotated_type(&self) -> &str {
        &self.annotated_type
    }

    /// 型変数を付け替えた引数。**渡し方は付け替えの対象にしない。**
    fn renamed(&self, placeholders: &Placeholders) -> Self {
        Self {
            kind: self.kind,
            annotated_type: placeholders.renamed(&self.annotated_type),
        }
    }
}

impl fmt::Display for Parameter {
    /// 型 1 つ分の綴りとして書き出す。入れ子の関数型を組み立て直すのに使う。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.kind.marker(), self.annotated_type)
    }
}

/// その引数の渡し方。
///
/// **落とすと渡し方の違う引数が同じ形になる。** `a: string` は必ず渡す引数、
/// `a?: string` は省ける引数、`this: E` は呼び出し時に渡さない引数で、
/// 受け取る値の数がそれぞれ違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParameterKind {
    /// 必ず渡す。
    Required,
    /// 省略できる（`a?: T`）。
    Optional,
    /// 可変長（`...a: T[]`）。
    Rest,
    /// 呼び出し時に渡さない（`this: T`）。TypeScript が受け取り手の型を書く場所。
    Receiver,
}

impl ParameterKind {
    /// 可変長を表す綴り。
    const REST_MARKER: &'static str = "...";

    /// 名前の綴りと、可変長かどうかから決める。
    ///
    /// `name` は `:` の手前（`a` / `a?` / `this` / `{ a, b }`）。
    fn of(name: &str, rest: bool) -> Self {
        if rest {
            return Self::Rest;
        }
        if name == RECEIVER_PARAMETER {
            return Self::Receiver;
        }
        if name.ends_with('?') {
            return Self::Optional;
        }
        Self::Required
    }

    /// 型の綴りに前置きする印。入れ子の関数型を書き出すときに使う。
    fn marker(self) -> &'static str {
        match self {
            Self::Required => "",
            Self::Optional => "?",
            Self::Rest => Self::REST_MARKER,
            // `this:` で始まる型は書けないので、本物の型と衝突しない。
            Self::Receiver => "this:",
        }
    }
}

/// 空白の連なりを 1 つに畳んだ綴り。**引用符の中は畳まない。**
///
/// TS のサーバはオブジェクト型リテラルを複数行に展開して返す
/// （`<T extends {\n    id: string;\n}, U>`）。改行と字下げが残ったままだと、
/// 同じ型が書かれ方の違いで別物になる。
///
/// **Why not（一律に畳む）**: 文字列リテラル型の中の空白は型の一部で、`"a b"` と
/// `"a  b"` は別の型。一律に畳むと**別の型が同じ形になり、単一化できると答えてしまう。**
fn flattened(text: &str) -> String {
    let mut flattened = String::new();
    let mut quote = QuoteState::new();
    let mut after_whitespace = false;

    for character in text.chars() {
        if quote.is_inside(character) {
            flattened.push(character);
            continue;
        }

        if character.is_whitespace() {
            after_whitespace = true;
            continue;
        }

        // 先頭の空白は落とす。末尾の空白は次の文字が来ないので、そもそも書き出されない。
        if after_whitespace && !flattened.is_empty() {
            flattened.push(' ');
        }
        after_whitespace = false;
        flattened.push(character);
    }

    flattened
}

/// 型名を、解決後の綴りへ差し替えた文字列。
///
/// 差し替えは**識別子の単位**で行う。部分一致で差し替えると、`Amount` が
/// `AmountRate` の中まで書き換える。
///
/// **引用符の中は差し替えない。** 文字列リテラル型 `"Amount"` を書き換えると
/// 別の型になる（[`flattened`] が引用符の中の空白を畳まないのと同じ理由）。
fn substituted(text: &str, resolved: &ResolvedTypes) -> String {
    let mut substituted = String::new();
    let mut identifier = String::new();
    let mut quote = QuoteState::new();
    let mut preceded = Preceded::Nothing;

    for (index, character) in text.char_indices() {
        let inside_quotes = quote.is_inside(character);
        if !inside_quotes && is_identifier_character(character) {
            identifier.push(character);
            continue;
        }

        let placement = Placement {
            preceded: preceded.clone(),
            following: text.get(index..).unwrap_or_default(),
        };
        push_resolved(&mut substituted, &identifier, resolved, &placement);

        // 識別子が終わったことを覚えてから、区切りの文字で上書きする。空白は覚えない
        // （`keyof Maybe` の `Maybe` から見た直前は、空白ではなく `keyof` そのもの）。
        if !identifier.is_empty() {
            preceded = Preceded::Word(identifier.clone());
        }
        identifier.clear();
        substituted.push(character);

        if !character.is_whitespace() {
            preceded = Preceded::Separator(character);
        }
    }
    let placement = Placement {
        preceded,
        following: "",
    };
    push_resolved(&mut substituted, &identifier, resolved, &placement);

    substituted
}

/// 綴りの中で、その名前が置かれている位置の前後。
///
/// **型名なのかも、括弧が要るかも、前後で決まる。** 2 つを別々に持ち回すと、
/// 片方だけを見て判断する枝が生えやすい。
struct Placement<'text> {
    /// その名前の直前にあったもの。
    preceded: Preceded,
    /// その名前の後ろに続く綴り。綴りの末尾なら空。
    following: &'text str,
}

/// 名前の直前にあったもの。
///
/// **区切りの文字と識別子を分けて持つ。** 識別子を「最後の 1 文字」に潰すと、
/// `keyof` と `typeof` を見分けられない（どちらも `f` で終わる）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Preceded {
    /// 綴りの先頭。
    Nothing,
    /// 区切りの文字。
    Separator(char),
    /// 識別子。前置きの型演算子（`keyof`）や、値を取る `typeof` がこれ。
    Word(String),
}

impl Placement<'_> {
    /// その名前が型ではなく、メンバーや引数の名前か。
    ///
    /// 型の綴りでは、名前の後ろにだけ [`MEMBER_MARKERS`] が続く。`{ ID: ID }` の左側、
    /// `{ ID(): ID }` のメソッド名、`(value: T)` の `value` がこれで、**差し替えると
    /// 型でないものを型で置き換える**ことになる。
    fn names_a_member(&self) -> bool {
        let following = self.following.trim_start();

        MEMBER_MARKERS
            .iter()
            .any(|marker| following.starts_with(marker))
    }

    /// その名前が型ではなく、値を指しているか。
    ///
    /// `typeof x` の `x` がこれ。**TypeScript は型と値で名前空間が別**なので、
    /// 同じ綴りの型エイリアスがあっても、ここを差し替えると型でないものを置き換えることになる。
    fn names_a_value(&self) -> bool {
        matches!(&self.preceded, Preceded::Word(word) if word == VALUE_OPERATOR)
    }

    /// 前後が型の区切りで、括弧を足さなくても 1 つのまとまりとして読めるか。
    ///
    /// 直前が識別子で終わっていれば挟まれていない。前置きの型演算子（`keyof` /
    /// `readonly` / `infer`）がこれで、**`keyof A | B` は `(keyof A) | B` と読まれる**。
    fn is_bounded(&self) -> bool {
        let bounded_before = match &self.preceded {
            Preceded::Nothing => true,
            Preceded::Separator(character) => TYPE_BOUNDARY_BEFORE.contains(character),
            Preceded::Word(_) => false,
        };
        let bounded_after = self
            .following
            .trim_start()
            .chars()
            .next()
            .is_none_or(|character| TYPE_BOUNDARY_AFTER.contains(&character));

        bounded_before && bounded_after
    }
}

/// 識別子 1 つ分を、解決後の綴り（解決できていなければそのまま）で書き足す。
fn push_resolved(
    target: &mut String,
    identifier: &str,
    resolved: &ResolvedTypes,
    placement: &Placement<'_>,
) {
    let names_a_type = !placement.names_a_member() && !placement.names_a_value();
    let opened = resolved
        .resolved_of(identifier)
        .filter(|_| names_a_type)
        .filter(|spelling| is_site_independent(spelling));
    let Some(spelling) = opened else {
        target.push_str(identifier);
        return;
    };

    target.push_str(&grouped_spelling_of(spelling, placement));
}

/// その綴りが、どこに書かれていても同じ型を指すか。
///
/// 開いた綴りに**別の場所で宣言された名前が残る**ことがある。サーバは型エイリアスを
/// 辿って展開するが、`interface` / `class` はそこで止まり、`type Boxed = Local` の
/// ように名前のまま返る（typescript-language-server 6.0.0 で実測）。
///
/// **残った名前は、その宣言のあるファイルでしか意味が決まらない。** 別々のモジュールが
/// それぞれの `Local` を宣言していると、どちらも `Local` に開かれて**別の型が
/// 単一化可能と出る**（偽陽性）。`Local[]` でも `{ x: Local }` でも同じことが起きるので、
/// 綴りの形ではなく**名前が残っているかどうか**で見る。
///
/// 名前として数えないのは 2 つ。メンバーや引数の名前（後ろに [`MEMBER_MARKERS`] が続く）と、
/// [`PREDEFINED_TYPES`] / [`TYPE_OPERATORS`] に載っている語。
///
/// **Why not（残った名前もその宣言まで辿る）**: 辿るには宣言のあるファイルを
/// 構文木にするところから始まり、綴りではなく位置で差し込む形になる（Issue #133）。
fn is_site_independent(spelling: &str) -> bool {
    let mut identifier = String::new();
    let mut quote = QuoteState::new();

    for (index, character) in spelling.char_indices() {
        let inside_quotes = quote.is_inside(character);
        if !inside_quotes && is_identifier_character(character) {
            identifier.push(character);
            continue;
        }

        let following = spelling.get(index..).unwrap_or_default();
        if names_a_declared_type(&identifier, following) {
            return false;
        }
        identifier.clear();
    }

    !names_a_declared_type(&identifier, "")
}

/// その語が、どこかで宣言された型の名前か。空の語と、数字で始まる語は名前ではない。
///
/// `following` はその語の後ろに続く綴り。メンバーや引数の名前を見分けるのに使う。
fn names_a_declared_type(identifier: &str, following: &str) -> bool {
    if identifier.is_empty() || identifier.starts_with(|first: char| first.is_ascii_digit()) {
        return false;
    }
    let names_the_language = PREDEFINED_TYPES.contains(&identifier)
        || TYPE_OPERATORS.contains(&identifier)
        || BOOLEAN_LITERALS.contains(&identifier);
    if names_the_language {
        return false;
    }

    let after = following.trim_start();
    !VETTED_MEMBER_MARKERS
        .iter()
        .any(|marker| after.starts_with(marker))
}

/// その位置に置くときの型の綴り。括弧が要るなら括ってから返す。
///
/// **Why**: `type Maybe = string | undefined` を `Maybe[]` の位置へそのまま差し込むと
/// `string | undefined[]` になり、`(string | undefined)[]` とは別の型を指す。
/// 呼び出し可能なエイリアスでは**倒れる向きが偽陽性**になり、`Handler | null` が
/// 「共用体を返す関数」として読める。
///
/// **Why not（常に括る）**: `Amount` が `(number)` になり、書き下した `number` と
/// 別の綴りになる。**エイリアスを開いた側だけが単一化できなくなる。**
fn grouped_spelling_of(spelling: &str, placement: &Placement<'_>) -> String {
    if placement.is_bounded() || !is_compound_type(spelling) {
        return spelling.to_owned();
    }

    format!("({spelling})")
}

/// その型の綴りが、括弧の外で組み合わさっているか。
///
/// **深さ 0 に空白があれば組み合わさっている。** 前置きの型演算子（`keyof Model`）や
/// 条件型（`A extends B ? C : D`）は演算子の一覧に載らないが、どれも語を空白で
/// つないだ形になる。演算子を列挙すると**漏れた 1 つが黙って偽陽性を作る**。
///
/// 深さ 0 だけを見るのは、`Map<string, A | B>` や `{ id: string }` のように
/// **括弧の中で組み合わさっている型は、それ自体が 1 つのまとまりとして置ける**ため。
/// 既に括られている綴り（`(A | B)`）も深さ 0 には何も無いので、二重に括らない。
fn is_compound_type(spelling: &str) -> bool {
    let scan = SignatureScan::new(spelling);
    let joined_by_space = scan.has_top_level_whitespace();
    let joined_by_operator = UNGROUPED_TYPE_OPERATORS
        .iter()
        .any(|operator| scan.top_level_index_of(*operator).is_some());

    joined_by_space || joined_by_operator
}

/// 引用符の中にいるかどうかを、文字を 1 つずつ読みながら追う。
///
/// 畳む側（[`flattened`]）と深さを数える側（[`SignatureScan`]）が同じ判断を要るので、
/// **どちらにも同じ状態を持たせない**ために切り出してある。
struct QuoteState {
    open: Option<char>,
    escaped: bool,
}

impl QuoteState {
    fn new() -> Self {
        Self {
            open: None,
            escaped: false,
        }
    }

    /// その文字が引用符の**中**にあるか。開き引用符そのものは中に数えない。
    ///
    /// 閉じるのは**エスケープされていない**同じ引用符だけ。`"a\"  b"` の `\"` で
    /// 閉じてしまうと、そこから先が引用符の外として扱われ、
    /// **リテラルの中の空白が畳まれる / 中の区切りで型を切る**。
    fn is_inside(&mut self, character: char) -> bool {
        let Some(open) = self.open else {
            if matches!(character, '"' | '\'' | '`') {
                self.open = Some(character);
                self.escaped = false;
            }
            return false;
        };

        let escaped = self.escaped;
        self.escaped = character == '\\' && !escaped;

        if character == open && !escaped {
            self.open = None;
        }
        true
    }
}

/// 綴りの中の 1 文字と、**その文字の外側**の深さ。
///
/// 開き括弧とそれに対応する閉じ括弧が同じ深さになる。
struct ScannedCharacter {
    index: usize,
    character: char,
    depth: usize,
}

/// 綴りを 1 文字ずつ、括弧の深さを添えて見たもの。
///
/// 引用符の中の文字は含めない。**文字列リテラル型 `"a, b"` の中の区切りを、
/// 型の区切りと取り違えないため。**
///
/// `=>` の `>` は閉じ括弧として数えない。型の中には `(cb: () => void, b: string)` の
/// ように矢印が現れ、これを数えると以降の深さがずれる。
struct SignatureScan<'text> {
    text: &'text str,
    characters: Vec<ScannedCharacter>,
}

impl<'text> SignatureScan<'text> {
    fn new(text: &'text str) -> Self {
        let mut characters = Vec::new();
        let mut depth: usize = 0;
        let mut quote = QuoteState::new();
        let mut previous = ' ';

        for (index, character) in text.char_indices() {
            if quote.is_inside(character) {
                previous = character;
                continue;
            }

            let closes =
                matches!(character, ')' | ']' | '}') || (character == '>' && previous != '=');
            if closes {
                depth = depth.saturating_sub(1);
            }

            characters.push(ScannedCharacter {
                index,
                character,
                depth,
            });

            if matches!(character, '(' | '[' | '{' | '<') {
                depth += 1;
            }
            previous = character;
        }

        Self { text, characters }
    }

    /// 深さ 0 にある `separator` で切り分ける。括弧・引用符の中の区切りは無視する。
    fn top_level_parts(&self, separator: char) -> Vec<&'text str> {
        let mut parts = Vec::new();
        let mut start = 0;

        for scanned in &self.characters {
            if scanned.character != separator || scanned.depth != 0 {
                continue;
            }
            parts.push(&self.text[start..scanned.index]);
            start = scanned.index + separator.len_utf8();
        }
        parts.push(&self.text[start..]);

        parts
    }

    /// 深さ 0 にある最初の `separator` の位置。無ければ `None`。
    fn top_level_index_of(&self, separator: char) -> Option<usize> {
        self.characters
            .iter()
            .find(|scanned| scanned.character == separator && scanned.depth == 0)
            .map(|scanned| scanned.index)
    }

    /// 深さ 0 に空白があるか。
    fn has_top_level_whitespace(&self) -> bool {
        self.characters
            .iter()
            .any(|scanned| scanned.depth == 0 && scanned.character.is_whitespace())
    }

    /// 深さ 0 にある最後の `target` の位置。無ければ `None`。
    fn last_top_level_index_of(&self, target: char) -> Option<usize> {
        self.characters
            .iter()
            .rfind(|scanned| scanned.character == target && scanned.depth == 0)
            .map(|scanned| scanned.index)
    }

    /// `position` 番目の開き括弧に対応する、閉じ括弧の位置。
    ///
    /// 同じ深さに戻った最初の閉じ括弧が対応する相手になる。
    fn closing_index_after(&self, position: usize, depth: usize) -> Option<usize> {
        self.characters
            .get(position + 1..)?
            .iter()
            .find(|scanned| scanned.character == ')' && scanned.depth == depth)
            .map(|scanned| scanned.index)
    }
}

/// 綴りを引数リストの括弧組で 3 つに割った形。
///
/// **割った先で読むものが違う。** 手前には型変数の宣言と呼び出しの仕方、
/// 中には引数、後ろには戻り値の型がある。
struct SplitSignature<'text> {
    /// 括弧組の手前。
    prefix: &'text str,
    /// 括弧組の中身。
    parameter_list: &'text str,
    /// 括弧組の後ろ。
    after_parameters: &'text str,
}

impl<'text> SplitSignature<'text> {
    /// 引数リストの括弧組を見つけて割る。見つからなければ `None`。
    ///
    /// 引数リストとみなすのは、**閉じ括弧の後ろが `:` か `=>` になる最初の括弧組**。
    ///
    /// **Why not（`(method)` などの接頭辞を列挙して剥がす）**: hover の接頭辞は
    /// `(method)` / `(property)` / `(local function)` / `function` / `const` と
    /// サーバごとに増える。列挙から漏れた 1 つが、黙って比較を壊す。
    fn from_signature_text(text: &'text str) -> Option<Self> {
        let scan = SignatureScan::new(text);

        for (position, scanned) in scan.characters.iter().enumerate() {
            if scanned.character != '(' || scanned.depth != 0 {
                continue;
            }

            let Some(close) = scan.closing_index_after(position, scanned.depth) else {
                continue;
            };
            let after_parameters = text.get(close + 1..)?;
            let follows = after_parameters.trim_start();

            if follows.starts_with(':') || follows.starts_with("=>") {
                return Some(Self {
                    prefix: text.get(..scanned.index)?,
                    parameter_list: text.get(scanned.index + 1..close)?,
                    after_parameters,
                });
            }
        }

        None
    }

    /// 呼び出しの仕方。
    fn kind(&self) -> SignatureKind {
        SignatureKind::from_prefix(self.prefix)
    }

    /// 引数リストの後ろに書かれた戻り値の型。読み取れなければ `None`。
    ///
    /// 宣言形は `): number`、値形は `) => number` と区切りが違う。**どちらの区切りだったかは
    /// 持たない。** 同じ型を指す 2 通りの書かれ方で、比較でも出力でも分岐しないため
    /// (`rules/coding.md`「まだ動かない選択肢を enum に並べない」)。
    fn return_type(&self) -> Option<&'text str> {
        let follows = self.after_parameters.trim_start();
        let annotated = follows
            .strip_prefix("=>")
            .or_else(|| follows.strip_prefix(':'))?;
        let return_type = annotated.trim();

        if return_type.is_empty() {
            return None;
        }
        Some(return_type)
    }

    /// 引数の並び。1 つも無ければ空。1 つでも読み取れない引数があれば `None`。
    fn parameters(&self) -> Option<Vec<Parameter>> {
        if self.parameter_list.trim().is_empty() {
            return Some(Vec::new());
        }

        SignatureScan::new(self.parameter_list)
            .top_level_parts(',')
            .into_iter()
            .map(Parameter::from_text)
            .collect()
    }

    /// 引数リストの手前に書かれた型変数の宣言。宣言が無ければ空。
    fn declared_type_parameters(&self) -> Vec<DeclaredTypeParameter> {
        let prefix = self.prefix.trim_end();
        if !prefix.ends_with('>') {
            return Vec::new();
        }

        // 深さ 0 にある最後の `<` が、末尾の `>` の相手。`Map<string, X>` のような
        // 入れ子は深さで外れ、`Foo<A>.bar<T>` のように 2 組並んでも後ろが選ばれる。
        let Some(open) = SignatureScan::new(prefix).last_top_level_index_of('<') else {
            return Vec::new();
        };

        let Some(declarations) = prefix.get(open + 1..prefix.len() - '>'.len_utf8()) else {
            return Vec::new();
        };

        SignatureScan::new(declarations)
            .top_level_parts(',')
            .into_iter()
            .filter_map(DeclaredTypeParameter::from_text)
            .collect()
    }
}

/// 型 1 つ分を、名前の入らない形へ直す。読み取れない関数型では `None`。
///
/// 関数型（`(a: string) => void`）はそれ自身が引数名を持つので、そこでも名前を落とす。
/// **落とさないと `cb: (a: string) => void` と `cb: (b: string) => void` が別物になる。**
/// コールバックを取る関数はどこにでもあるので、名前の違いがそのまま偽陰性になる。
///
/// 総称型や括弧に包まれた関数型（`Array<(a: string) => void>`）までは踏み込まない。
/// そこまで見るには型そのものの構文解析が要る。
fn normalized_type(text: &str) -> Option<String> {
    let Some(split) = SplitSignature::from_signature_text(text) else {
        return Some(text.to_owned());
    };

    let return_type = normalized_type(split.return_type()?)?;
    let parameters: Vec<String> = split
        .parameters()?
        .iter()
        .map(Parameter::to_string)
        .collect();

    Some(format!(
        "{}({}) => {return_type}",
        split.prefix,
        parameters.join(", ")
    ))
}

/// 綴りに書かれたままの型変数の宣言。
///
/// 正規化の途中でしか使わないので公開しない。外へ出るのは付け替えた後の
/// [`TypeSignature`] だけ。
struct DeclaredTypeParameter {
    name: String,
    constraint: Option<String>,
    default: Option<String>,
}

impl DeclaredTypeParameter {
    /// 型変数 1 つ分の宣言から、名前・制約・既定の型を読む。名前が無ければ `None`。
    ///
    /// 既定の型を先に切り離す。`T extends X = D` は制約と既定の両方を持ち、
    /// 切り離さないと制約が `X = D` になる。
    fn from_text(declaration: &str) -> Option<Self> {
        let declaration = declaration.trim();
        let (bounded, default) = match declaration.split_once(DEFAULT_MARKER) {
            Some((bounded, default)) => (bounded, Some(default.trim().to_owned())),
            None => (declaration, None),
        };
        let name = bounded.split_whitespace().next()?;

        Some(Self {
            name: name.to_owned(),
            constraint: bounded
                .split_once(CONSTRAINT_KEYWORD)
                .map(|(_, constraint)| constraint.trim().to_owned()),
            default,
        })
    }
}

/// 付け替えを終えた型変数 1 つ分。名前は付け替えで消えているので持たない。
///
/// 既定の型を持つのは、**型引数を省いて呼んだときの型がそこで決まる**ため。
/// 落とすと `f<T = string>(): T` と `g<U = number>(): U` が同じ形になるが、
/// どちらも引数無しで呼ぶと戻り値の型が違う。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeParameter {
    constraint: Option<String>,
    default: Option<String>,
}

/// 型変数の名前を、出現順の綴り（`%0`, `%1`, …）へ付け替える対応。
///
/// **並びと対応を 1 つの型で持つ。** 型変数の宣言を並べ直すのにも、型の綴りを
/// 書き換えるのにも同じ並びが要るので、別々に持ち回すと片方だけが古くなる。
struct Placeholders {
    /// 付け替え後の並び。`ordered[0]` が `%0` になった型変数の元の名前。
    ordered: Vec<String>,
    by_name: HashMap<String, String>,
}

impl Placeholders {
    /// 引数と戻り値に現れる順で付け替えを決める。
    ///
    /// `occurrences` は引数の型と戻り値の型を、綴りに書かれた順に並べたもの。
    /// 一度も現れない型変数は宣言の順で後ろに置く。
    ///
    /// **Why（宣言順ではなく出現順）**: `f<T, U>(a: U, b: T)` と `g<A, B>(a: A, b: B)` は
    /// どちらも「異なる 2 つの型を取る」形で単一化できる。宣言順で付け替えると、
    /// 前者が `(%1, %0)`、後者が `(%0, %1)` になって別物になる。
    fn of<'a>(
        declared: &[DeclaredTypeParameter],
        occurrences: impl Iterator<Item = &'a str>,
    ) -> Self {
        let declared_names: BTreeSet<&str> = declared
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect();
        let mut ordered: Vec<String> = Vec::new();

        for occurrence in occurrences {
            for identifier in identifiers_of(occurrence) {
                let already_ordered = ordered.iter().any(|name| name == identifier);
                if !declared_names.contains(identifier) || already_ordered {
                    continue;
                }
                ordered.push(identifier.to_owned());
            }
        }

        for declaration in declared {
            if ordered.contains(&declaration.name) {
                continue;
            }
            ordered.push(declaration.name.clone());
        }

        let by_name = ordered
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), format!("{PLACEHOLDER_PREFIX}{index}")))
            .collect();

        Self { ordered, by_name }
    }

    /// 型変数を、付け替え後の並びで返す。
    ///
    /// 制約と既定の型も付け替えの対象にする。`<T, U extends T>` のように、
    /// どちらも別の型変数を指すことがある。
    fn type_parameters(&self, declared: &[DeclaredTypeParameter]) -> Vec<TypeParameter> {
        self.ordered
            .iter()
            .map(|name| {
                let declaration = declared
                    .iter()
                    .find(|declaration| declaration.name == *name);
                let renamed_part = |part: Option<&String>| part.map(|text| self.renamed(text));

                TypeParameter {
                    constraint: renamed_part(declaration.and_then(|it| it.constraint.as_ref())),
                    default: renamed_part(declaration.and_then(|it| it.default.as_ref())),
                }
            })
            .collect()
    }

    /// 型変数の名前を、付け替え後の綴りに置き換えた文字列。
    ///
    /// 識別子の単位で置き換える。部分一致で置き換えると `T` が `Tree` の中まで書き換える。
    fn renamed(&self, text: &str) -> String {
        let mut renamed = String::new();
        let mut identifier = String::new();

        for character in text.chars() {
            if is_identifier_character(character) {
                identifier.push(character);
                continue;
            }

            self.push_renamed(&mut renamed, &identifier);
            identifier.clear();
            renamed.push(character);
        }
        self.push_renamed(&mut renamed, &identifier);

        renamed
    }

    /// 識別子 1 つ分を、付け替え後の綴り（対応が無ければそのまま）で書き足す。
    fn push_renamed(&self, target: &mut String, identifier: &str) {
        match self.by_name.get(identifier) {
            Some(placeholder) => target.push_str(placeholder),
            None => target.push_str(identifier),
        }
    }
}

/// 文字列に現れる識別子を、書かれた順に返す。
fn identifiers_of(text: &str) -> Vec<&str> {
    text.split(|character: char| !is_identifier_character(character))
        .filter(|identifier| !identifier.is_empty())
        .collect()
}

/// 識別子を作る文字か。TypeScript の識別子には `_` と `$` も入る。
fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストが渡す綴りは読み取れる前提で組み立てる。解決した型名は無い。
    fn signature(text: &str) -> TypeSignature {
        signature_with(text, &ResolvedTypes::default())
    }

    /// 解決した型名を差し込んでから組み立てる。
    fn signature_with(text: &str, resolved: &ResolvedTypes) -> TypeSignature {
        TypeSignature::from_signature_text(text, resolved).expect("テストが渡す綴りは読み取れる")
    }

    /// 型名 1 つ分の解決。
    fn resolving(name: &str, spelling: &str) -> ResolvedTypes {
        ResolvedTypes::new([(name.to_owned(), spelling.to_owned())])
    }

    fn unifiable(one: &str, other: &str) -> bool {
        signature(one).is_unifiable_with(&signature(other))
    }

    #[test]
    fn test_signatures_differing_only_in_parameter_names_are_unifiable() {
        assert!(unifiable(
            "function applyDiscount(userId: string): void",
            "function greet(name: string): void"
        ));
    }

    #[test]
    fn test_signatures_differing_in_a_parameter_type_are_not_unifiable() {
        // 対照は上のテスト。引数名ではなく型を変えている
        assert!(!unifiable(
            "function applyDiscount(userId: string): void",
            "function greet(name: number): void"
        ));
    }

    #[test]
    fn test_signatures_differing_only_in_type_variable_names_are_unifiable() {
        assert!(unifiable(
            "function pickFirst<T>(items: T[]): T | undefined",
            "function head<U>(xs: U[]): U | undefined"
        ));
    }

    #[test]
    fn test_signatures_using_their_type_variables_in_the_same_order_are_unifiable() {
        // 宣言順で付け替えると (%1, %0) と (%0, %1) になって別物になる。
        // どちらも「異なる 2 つの型を取る」形なので単一化できる
        assert!(unifiable(
            "function swap<T, U>(a: U, b: T): void",
            "function pair<A, B>(a: A, b: B): void"
        ));
    }

    #[test]
    fn test_signatures_reusing_one_type_variable_are_not_unifiable_with_two_distinct_ones() {
        // 対照は上のテスト。片方だけ 2 つの引数が同じ型変数を指している
        assert!(!unifiable(
            "function same<T>(a: T, b: T): void",
            "function pair<A, B>(a: A, b: B): void"
        ));
    }

    #[test]
    fn test_a_value_form_signature_is_unifiable_with_the_declaration_form_of_the_same_type() {
        // アロー関数は `const arrow: (a: string) => number`、関数宣言は
        // `function decl(a: string): number` とサーバの返す綴りの形が違う
        assert!(unifiable(
            "const arrow: (value: string) => number",
            "function decl(text: string): number"
        ));
    }

    #[test]
    fn test_a_method_signature_is_not_confused_by_the_prefix_in_front_of_it() {
        // 接頭辞の `(method)` を引数リストと取り違えると、引数が 0 個の別物になる
        assert!(unifiable(
            "(method) Cart.total(items: string): number",
            "function decl(text: string): number"
        ));
    }

    #[test]
    fn test_an_optional_parameter_is_not_unifiable_with_a_required_one() {
        // 省略可の印を落とすと、必ず渡す引数と省ける引数が同じ形になる
        assert!(!unifiable(
            "function opt(value?: string): void",
            "function req(value: string): void"
        ));
    }

    #[test]
    fn test_a_rest_parameter_is_not_unifiable_with_a_plain_array_parameter() {
        assert!(!unifiable(
            "function spread(...values: number[]): void",
            "function listed(values: number[]): void"
        ));
    }

    #[test]
    fn test_signatures_with_a_different_number_of_parameters_are_not_unifiable() {
        assert!(!unifiable(
            "function one(a: string): void",
            "function two(a: string, b: string): void"
        ));
    }

    #[test]
    fn test_a_constrained_type_variable_is_not_unifiable_with_an_unconstrained_one() {
        // 制約を落とすと、任意の型を取れる形と id を持つ型しか取れない形が同じになる
        assert!(!unifiable(
            "function withId<T extends { id: string; }>(a: T): void",
            "function anything<U>(a: U): void"
        ));
    }

    #[test]
    fn test_signatures_with_the_same_constraint_written_differently_are_unifiable() {
        // 制約の中の型変数も付け替える。`<T, U extends T>` の形
        assert!(unifiable(
            "function bounded<T, U extends T>(a: T, b: U): void",
            "function limited<A, B extends A>(a: A, b: B): void"
        ));
    }

    #[test]
    fn test_type_variables_with_different_defaults_are_not_unifiable() {
        // 既定の型を落とすと、型引数を省いて呼んだときに戻り値の型が違う 2 つが
        // 同じ形になる
        assert!(!unifiable(
            "function withText<T = string>(): T",
            "function withCount<U = number>(): U"
        ));
    }

    #[test]
    fn test_type_variables_with_the_same_default_are_unifiable() {
        // 対照は上のテスト。既定の型だけを揃えている
        assert!(unifiable(
            "function withText<T = string>(): T",
            "function alsoText<U = string>(): U"
        ));
    }

    #[test]
    fn test_a_constructor_is_not_unifiable_with_a_function_returning_the_same_type() {
        // コンストラクタも method_definition なのでチャンクになる。呼び出しの仕方を
        // 落とすと、`new` が要る型と要らない型が同じ形になる
        assert!(!unifiable(
            "constructor Result(value: string): Result",
            "function create(value: string): Result"
        ));
    }

    #[test]
    fn test_two_constructors_of_the_same_shape_are_unifiable() {
        // 対照は上のテスト。どちらもコンストラクタなら同じ形になる
        assert!(unifiable(
            "constructor Result(value: string): Result",
            "constructor Wrapper(text: string): Result"
        ));
    }

    #[test]
    fn test_a_construct_signature_in_value_form_is_not_unifiable_with_a_call_signature() {
        // 値形の構築シグネチャ。`new` の有無だけが違う
        assert!(!unifiable(
            "const make: new (value: string) => Result",
            "const call: (value: string) => Result"
        ));
    }

    #[test]
    fn test_a_this_parameter_is_not_unifiable_with_an_ordinary_parameter_of_the_same_type() {
        // `this` は呼び出し時に渡さない引数。名前ごと落とすと、受け取る値が
        // 1 つの関数と 2 つの関数が同じ形になる
        assert!(!unifiable(
            "function bound(this: HTMLElement, value: string): void",
            "function plain(context: HTMLElement, value: string): void"
        ));
    }

    #[test]
    fn test_two_this_parameters_of_the_same_type_are_unifiable() {
        // 対照は上のテスト。`this` どうしなら名前が残っても同じ形になる
        assert!(unifiable(
            "function bound(this: HTMLElement, value: string): void",
            "function alsoBound(this: HTMLElement, text: string): void"
        ));
    }

    #[test]
    fn test_an_escaped_quote_does_not_end_a_string_literal_type() {
        // `\"` で引用符を閉じたことにすると、そこから先が引用符の外になり
        // リテラルの中の空白が畳まれる
        assert!(!unifiable(
            "function spaced(label: \"a\\\"  b\"): void",
            "function single(label: \"a\\\" b\"): void"
        ));
    }

    #[test]
    fn test_a_comma_after_an_escaped_quote_does_not_split_the_parameter_list() {
        // 引用符を早く閉じると、リテラルの中の `,` が引数の区切りに見える
        assert!(unifiable(
            "function labelled(label: \"a\\\", b\", count: number): void",
            "function other(name: \"a\\\", b\", total: number): void"
        ));
    }

    #[test]
    fn test_string_literal_types_differing_only_in_whitespace_are_not_unifiable() {
        // 引用符の中の空白は型の一部。一律に畳むと別の型が同じ形になる
        assert!(!unifiable(
            "function spaced(label: \"a  b\"): void",
            "function single(label: \"a b\"): void"
        ));
    }

    #[test]
    fn test_a_signature_broken_over_lines_reads_the_same_as_one_written_on_a_single_line() {
        // TS のサーバはオブジェクト型リテラルを複数行に展開して返す
        assert!(unifiable(
            "function constrained<T extends {\n    id: string;\n}, U>(a: T, b: U): [T, U]",
            "function inline<A extends { id: string; }, B>(x: A, y: B): [A, B]"
        ));
    }

    #[test]
    fn test_a_comma_inside_a_generic_argument_does_not_split_the_parameter_list() {
        // `Map<string, number>` の中のカンマで切ると、引数が 3 つに見える
        assert!(unifiable(
            "function mapped(lookup: Map<string, number>, key: string): void",
            "function other(table: Map<string, number>, name: string): void"
        ));
    }

    #[test]
    fn test_signatures_differing_inside_a_generic_argument_are_not_unifiable() {
        // 対照は上のテスト。総称型の中身は比べる対象に残る
        assert!(!unifiable(
            "function mapped(lookup: Map<string, number>, key: string): void",
            "function other(table: Map<string, Date>, name: string): void"
        ));
    }

    #[test]
    fn test_signatures_differing_only_in_a_callback_parameter_name_are_unifiable() {
        // 関数型の引数もそれ自身の引数名を持つ。落とさないと、コールバックを取る
        // 関数が名前の違いだけで別物になる
        assert!(unifiable(
            "function retry(onError: (reason: string) => void): void",
            "function repeat(handler: (message: string) => void): void"
        ));
    }

    #[test]
    fn test_an_arrow_inside_a_parameter_type_does_not_close_the_brackets_around_it() {
        // `=>` の `>` を閉じ括弧として数えると、タプルの中のカンマが深さ 0 に見えて
        // 引数を切り損なう
        assert!(unifiable(
            "function paired(pair: [(a: string) => void, number]): void",
            "function other(entry: [(a: string) => void, number]): void"
        ));
    }

    #[test]
    fn test_a_signature_without_a_parameter_list_cannot_be_read() {
        // 型エイリアスなど、関数でないものへの hover はこの形で返る
        assert_eq!(
            TypeSignature::from_signature_text("type UserId = string", &ResolvedTypes::default()),
            None
        );
    }

    #[test]
    fn test_a_signature_without_a_return_type_cannot_be_read() {
        assert_eq!(
            TypeSignature::from_signature_text(
                "function decl(a: string)",
                &ResolvedTypes::default()
            ),
            None
        );
    }

    #[test]
    fn test_a_parameter_without_a_type_annotation_cannot_be_read() {
        // 名前だけの引数からは型を取り出せない。空の型で埋めると、
        // 型注釈の無い引数どうしが「同じ型」として重なる
        assert_eq!(
            TypeSignature::from_signature_text(
                "function decl(a, b: string): void",
                &ResolvedTypes::default()
            ),
            None
        );
    }

    #[test]
    fn test_a_signature_without_parameters_is_read_as_an_empty_parameter_list() {
        assert!(unifiable(
            "function none(): number",
            "const value: () => number"
        ));
    }

    #[test]
    fn test_a_signature_without_parameters_is_not_unifiable_with_one_that_takes_a_parameter() {
        // 対照は上のテスト。引数が 0 個であることが形として残る
        assert!(!unifiable(
            "function none(): number",
            "function one(a: string): number"
        ));
    }

    #[test]
    fn test_a_comma_inside_a_string_literal_type_does_not_split_the_parameter_list() {
        assert!(unifiable(
            "function labelled(label: \"a, b\", count: number): void",
            "function other(name: \"a, b\", total: number): void"
        ));
    }

    #[test]
    fn test_a_signature_using_a_type_alias_is_unifiable_with_one_written_out() {
        // hover は `Amount` を展開しないので、解決を差し込まないと
        // `(Amount, number) => Amount` と `(number, number) => number` になる
        let aliased = signature_with(
            "function scaleAmount(amount: Amount, factor: number): Amount",
            &resolving("Amount", "number"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function scaleTotal(total: number, factor: number): number"
        )));
    }

    #[test]
    fn test_a_signature_using_an_unresolved_type_alias_is_not_unifiable_with_one_written_out() {
        // 対照は上のテスト。同じ綴りを解決なしで読む。解決が効いているかを見る
        assert!(!unifiable(
            "function scaleAmount(amount: Amount, factor: number): Amount",
            "function scaleTotal(total: number, factor: number): number"
        ));
    }

    #[test]
    fn test_a_signature_that_is_nothing_but_an_alias_is_read_after_the_alias_is_opened() {
        // 引数リストを持たない綴り。読んだ後に差し込む形では入口に入れない
        let aliased = signature_with(
            "const halveAmount: Scaling",
            &resolving("Scaling", "(amount: number) => number"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("const halveTotal: (total: number) => number"))
        );
    }

    #[test]
    fn test_a_signature_that_is_nothing_but_an_unresolved_alias_cannot_be_read() {
        // 対照は上のテスト。解決が無ければ引数リストが見つからない
        assert_eq!(
            TypeSignature::from_signature_text(
                "const halveAmount: Scaling",
                &ResolvedTypes::default()
            ),
            None
        );
    }

    #[test]
    fn test_a_type_name_inside_a_string_literal_type_is_not_substituted() {
        // 文字列リテラル型の中身は型の一部。差し替えると別の型になる
        let literal = signature_with(
            "function labelled(label: \"Amount\"): void",
            &resolving("Amount", "number"),
        );

        assert!(!literal.is_unifiable_with(&signature("function other(name: \"number\"): void")));
    }

    #[test]
    fn test_a_name_that_only_starts_with_a_resolved_type_name_is_not_substituted() {
        // 部分一致で差し替えると `Amount` が `AmountRate` の中まで書き換える
        let longer = signature_with(
            "function scale(rate: AmountRate): void",
            &resolving("Amount", "number"),
        );

        assert!(!longer.is_unifiable_with(&signature("function other(rate: numberRate): void")));
    }

    #[test]
    fn test_an_opened_alias_keeps_its_precedence_inside_an_array_type() {
        // `Maybe[]` へ `string | undefined` をそのまま差し込むと `string | undefined[]`
        // になり、`(string | undefined)[]` とは別の型を指す
        let aliased = signature_with(
            "function pick(values: Maybe[]): void",
            &resolving("Maybe", "string | undefined"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(items: (string | undefined)[]): void"
        )));
    }

    #[test]
    fn test_an_opened_alias_standing_on_its_own_is_not_wrapped() {
        // 対照は上のテスト。括弧を常に足すと、書き下した綴りと別物になる
        let aliased = signature_with(
            "function pick(value: Maybe): void",
            &resolving("Maybe", "string | undefined"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(item: string | undefined): void"))
        );
    }

    #[test]
    fn test_an_opened_alias_of_a_single_type_is_not_wrapped_inside_an_array_type() {
        // 括弧が要るのは組み合わさった型だけ。`number` を包むと `(number)[]` になり、
        // 書き下した `number[]` と別物になる
        let aliased = signature_with(
            "function pick(values: Amount[]): void",
            &resolving("Amount", "number"),
        );

        assert!(aliased.is_unifiable_with(&signature("function other(items: number[]): void")));
    }

    #[test]
    fn test_an_opened_callable_alias_inside_an_array_type_is_not_read_as_a_function() {
        // `Handler[]` は関数型の配列。包まないと「number の配列を返す関数」になり、
        // **別の型なのに単一化可能と出る**（倒れる向きが偽陽性）
        let aliased = signature_with(
            "function pick(values: Handler[]): void",
            &resolving("Handler", "(value: string) => number"),
        );

        assert!(!aliased.is_unifiable_with(&signature(
            "function other(items: (value: string) => number[]): void"
        )));
    }

    #[test]
    fn test_an_opened_callable_alias_keeps_its_precedence_inside_a_union() {
        // `Handler | null` へ `() => string` をそのまま差し込むと `() => string | null`
        // になり、**共用体を返す関数**という別の型として読める
        let aliased = signature_with(
            "function pick(handler: Handler | null): void",
            &resolving("Handler", "() => string"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(callback: (() => string) | null): void"
        )));
    }

    #[test]
    fn test_an_opened_callable_alias_inside_a_union_is_not_read_as_returning_it() {
        // 対照は上のテスト。括らないほうの読み方と重ならないことを見る（偽陽性の側）
        let aliased = signature_with(
            "function pick(handler: Handler | null): void",
            &resolving("Handler", "() => string"),
        );

        assert!(!aliased.is_unifiable_with(&signature(
            "function other(callback: () => string | null): void"
        )));
    }

    #[test]
    fn test_an_opened_alias_after_a_union_bar_is_wrapped_too() {
        // 括弧が要るかは後ろだけでは決まらない。手前が区切りでなければ同じく要る
        let aliased = signature_with(
            "function pick(handler: null | Handler): void",
            &resolving("Handler", "() => string"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(callback: null | (() => string)): void"
        )));
    }

    #[test]
    fn test_an_opened_alias_inside_a_generic_argument_is_not_wrapped() {
        // 型引数の中は区切りに挟まれているので括弧は要らない。括ると書き下した綴りと
        // 別物になる
        let aliased = signature_with(
            "function mapped(lookup: Map<string, Maybe>): void",
            &resolving("Maybe", "string | undefined"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(table: Map<string, string | undefined>): void"
        )));
    }

    #[test]
    fn test_a_member_name_that_matches_a_resolved_type_name_is_not_substituted() {
        // オブジェクト型のキーは型ではない。差し替えると `{ string: string }` になる
        let aliased = signature_with(
            "function pick(value: { ID: ID }): void",
            &resolving("ID", "string"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(value: { ID: string }): void"))
        );
    }

    #[test]
    fn test_an_optional_member_name_that_matches_a_resolved_type_name_is_not_substituted() {
        // 省略できるメンバーは `?:` で続く。`?` だけを見ると条件型の `?` と区別が付かない
        let aliased = signature_with(
            "function pick(value: { ID?: ID }): void",
            &resolving("ID", "string"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(value: { ID?: string }): void"))
        );
    }

    #[test]
    fn test_a_method_name_that_matches_a_resolved_type_name_is_not_substituted() {
        // メソッド名の後ろは `:` ではなく `(`。名前まで差し替えると
        // `{ string(): string }` になる
        let aliased = signature_with(
            "function pick(value: { ID(): ID }): void",
            &resolving("ID", "string"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(value: { ID(): string }): void"))
        );
    }

    #[test]
    fn test_a_generic_method_name_that_matches_a_resolved_type_name_is_not_substituted() {
        // 総称メソッドの名前の後ろは `<`。型名が `<` を伴うのは総称型のときだけで、
        // そちらは開く対象に入っていない
        let aliased = signature_with(
            "function pick(value: { ID<T>(x: T): ID }): void",
            &resolving("ID", "string"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(value: { ID<T>(x: T): string }): void"
        )));
    }

    #[test]
    fn test_an_opened_alias_after_a_type_operator_keyword_is_wrapped() {
        // `keyof Maybe` へ `string | number` をそのまま差し込むと
        // `keyof string | number` になり、TypeScript は `(keyof string) | number` と読む
        let aliased = signature_with(
            "function pick(key: keyof Maybe): void",
            &resolving("Maybe", "string | number"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(key: keyof (string | number)): void"
        )));
    }

    #[test]
    fn test_an_opened_alias_after_a_type_operator_keyword_is_not_read_as_binding_tighter() {
        // 対照は上のテスト。括らない読み方と重ならないことを見る（偽陽性の側）
        let aliased = signature_with(
            "function pick(key: keyof Maybe): void",
            &resolving("Maybe", "string | number"),
        );

        assert!(!aliased.is_unifiable_with(&signature(
            "function other(key: keyof string | number): void"
        )));
    }

    #[test]
    fn test_a_value_named_like_a_resolved_type_is_not_substituted() {
        // `typeof ID` の `ID` は値の名前。TypeScript は型と値で名前空間が別なので、
        // 同じ綴りが両方にありうる。差し替えると `typeof string` という綴りになる
        let aliased = signature_with(
            "function pick(x: ID, y: typeof ID): void",
            &resolving("ID", "string"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(a: string, b: typeof ID): void"))
        );
    }

    #[test]
    fn test_an_opened_alias_whose_body_starts_with_a_type_operator_is_wrapped() {
        // `type Keys = keyof string` を `Keys[]` の位置へそのまま差し込むと
        // `keyof string[]` になり、TypeScript は `keyof (string[])` と読む
        let aliased = signature_with(
            "function pick(keys: Keys[]): void",
            &resolving("Keys", "keyof string"),
        );

        assert!(
            aliased.is_unifiable_with(&signature("function other(keys: (keyof string)[]): void"))
        );
    }

    #[test]
    fn test_an_opened_alias_whose_body_starts_with_a_type_operator_is_not_read_as_an_array_of_it() {
        // 対照は上のテスト。括らない読み方と重ならないことを見る（偽陽性の側）
        let aliased = signature_with(
            "function pick(keys: Keys[]): void",
            &resolving("Keys", "keyof string"),
        );

        assert!(
            !aliased.is_unifiable_with(&signature("function other(keys: keyof string[]): void"))
        );
    }

    #[test]
    fn test_an_already_grouped_alias_body_is_not_wrapped_again() {
        // 括弧の中で組み合わさっている綴りは、それ自体が 1 つのまとまり。
        // 二重に括ると書き下した綴りと別物になる
        let aliased = signature_with(
            "function pick(value: Grouped[]): void",
            &resolving("Grouped", "(string | number)"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(value: (string | number)[]): void"
        )));
    }

    #[test]
    fn test_two_aliases_opening_onto_the_same_declared_type_name_are_not_unifiable() {
        // `interface` の後ろでは hover が展開を止め、右辺は名前のまま返る。
        // 別々のモジュールがそれぞれの `Local` を宣言していると、綴りだけを見て
        // 差し込んだ結果が重なる（偽陽性）
        let boxed = signature_with(
            "function labelBoxed(value: Boxed): string",
            &resolving("Boxed", "Local"),
        );
        let wrapped = signature_with(
            "function labelWrapped(value: Wrapped): string",
            &resolving("Wrapped", "Local"),
        );

        assert!(!boxed.is_unifiable_with(&wrapped));
    }

    #[test]
    fn test_two_aliases_opening_onto_the_same_predefined_type_are_unifiable() {
        // 対照は上のテスト。組み込みの型は宣言を辿らずに意味が決まるので、
        // 同じ綴りに開かれたら同じ型を指している
        let boxed = signature_with(
            "function labelBoxed(value: Boxed): string",
            &resolving("Boxed", "number"),
        );
        let wrapped = signature_with(
            "function labelWrapped(value: Wrapped): string",
            &resolving("Wrapped", "number"),
        );

        assert!(boxed.is_unifiable_with(&wrapped));
    }

    #[test]
    fn test_two_aliases_holding_the_same_declared_type_name_inside_are_not_unifiable() {
        // 名前が残るのは右辺全体のときだけではない。`{ x: Local }` の中に 1 つ残っても
        // 同じことが起きるので、綴りの形ではなく名前が残っているかどうかで見る
        let boxed = signature_with(
            "function labelBoxed(value: Boxed): string",
            &resolving("Boxed", "{ x: Local }"),
        );
        let wrapped = signature_with(
            "function labelWrapped(value: Wrapped): string",
            &resolving("Wrapped", "{ x: Local }"),
        );

        assert!(!boxed.is_unifiable_with(&wrapped));
    }

    #[test]
    fn test_two_aliases_opening_onto_the_same_generic_type_reference_are_not_unifiable() {
        // `Local<string>` の `Local` は宣言を辿る相手。`<` が続くのをメソッド名の印と
        // 読むと検証をすり抜け、別々のモジュールの `Local` が同じ綴りとして重なる
        let first = signature_with(
            "function a(x: First): void",
            &resolving("First", "Local<string>"),
        );
        let second = signature_with(
            "function b(y: Second): void",
            &resolving("Second", "Local<string>"),
        );

        assert!(!first.is_unifiable_with(&second));
    }

    #[test]
    fn test_two_aliases_opening_onto_the_same_generic_of_predefined_types_are_unifiable() {
        // 対照は上のテスト。総称型そのものを拒んでいるのではなく、**宣言を辿る名前が
        // 残っていること**を拒んでいる
        let first = signature_with(
            "function a(x: First): void",
            &resolving("First", "{ value: string; }"),
        );
        let second = signature_with(
            "function b(y: Second): void",
            &resolving("Second", "{ value: string; }"),
        );

        assert!(first.is_unifiable_with(&second));
    }

    #[test]
    fn test_an_alias_opening_onto_a_boolean_literal_is_substituted() {
        // `true` / `false` はリテラル型で、宣言を辿らずに意味が決まる。型名として
        // 数えると、書き下した綴りとの比較が止まる
        let enabled = signature_with(
            "function pick(x: Enabled): void",
            &resolving("Enabled", "true"),
        );

        assert!(enabled.is_unifiable_with(&signature("function other(x: true): void")));
    }

    #[test]
    fn test_an_alias_opening_onto_a_declared_type_is_still_not_substituted() {
        // 対照は上のテスト。リテラル型を通したことで、宣言された型名まで通っていないか
        let boxed = signature_with(
            "function pick(x: Boxed): void",
            &resolving("Boxed", "Local"),
        );

        assert!(!boxed.is_unifiable_with(&signature("function other(x: Local): void")));
    }

    #[test]
    fn test_an_alias_naming_only_members_and_predefined_types_is_substituted() {
        // メンバーの名前（後ろが `:`）と引数の名前は、宣言を辿る相手ではない。
        // これを型名と数えると、書き下した綴りとの比較がまるごと止まる
        let shaped = signature_with(
            "function labelShaped(value: Shape): string",
            &resolving("Shape", "{ readonly amount: number; }"),
        );

        assert!(shaped.is_unifiable_with(&signature(
            "function other(value: { readonly amount: number; }): string"
        )));
    }

    #[test]
    fn test_a_type_alias_opened_into_a_generic_argument_is_substituted_there_too() {
        // 総称型の中の型名も比較の対象に残る（`test_signatures_differing_inside_a_
        // generic_argument_are_not_unifiable`）ので、差し替えもそこまで届く必要がある
        let aliased = signature_with(
            "function mapped(lookup: Map<string, Amount>): void",
            &resolving("Amount", "number"),
        );

        assert!(aliased.is_unifiable_with(&signature(
            "function other(table: Map<string, number>): void"
        )));
    }
}
