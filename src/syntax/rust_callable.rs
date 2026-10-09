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

use crate::syntax::tree::{Grammar, SyntaxTree, source_position_of};
use crate::syntax::type_reference::TypeReference;

mod projection;
pub(crate) use projection::{
    RustAssociatedDefinition, RustProjectionSource, associated_owner_position_of,
};
mod selection;
pub(crate) use selection::{RustImplBinding, RustImplCandidate, RustQualifiedProjection};
mod array_length;
pub(crate) use array_length::{
    ArithmeticOperator, ConstSyntaxError, RustArrayLengths, RustConstExpression, RustConstSource,
};

/// 付け替えた型変数の綴りの前置き。
///
/// `%` は識別子に使えない文字なので、**元の型名と衝突しない**
/// （`syntax::type_structure` と同じ印）。
const PLACEHOLDER_PREFIX: &str = "%";

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

/// 型の綴りに現れない子の種別。
///
/// **ソースではどこにでも書ける**（引数の間・`where` 句の中）。型の一部ではないので読み飛ばす。
/// hover の `// Bounds from impl:` は、境界を元の impl スコープから読み直すための区切り。
/// それ以外のコメントは型の綴りから落とす。
const COMMENT_KINDS: [&str; 2] = ["line_comment", "block_comment"];

/// 引数に付く属性（`#[cfg(..)] a: A`）。hover の綴りには現れない。
const ATTRIBUTE_KIND: &str = "attribute_item";

/// Rust の関数 1 つ分の、単一化の可否を比べられる形。
///
/// 関数名・引数名・可視性を落とし、型変数を impl / メソッド別に出現順で付け替える
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
    /// 囲む impl が束縛した型変数の数。メソッド自身の型変数とは別に比べる。
    impl_type_parameter_count: usize,
    /// 境界。**左辺の型の綴りと、境界 1 つの綴りの組**の集合。
    ///
    /// **集合で持つ。** inline と `where` の書き分け・`+` の並び・同じ左辺への述語の
    /// 分け方は、どれも同じ要求を 2 通り以上に綴ったもの。
    trait_bounds: BTreeSet<(String, String)>,
    /// 比較に残る綴りに現れた型名。名前順。
    ///
    /// **束縛した型変数・確認済みのプリミティブ・`?Sized`・関連型の名前（`Item = u8` の
    /// `Item`）は入らない。** 問い合わせに失敗したプリミティブ表記は理由を残すために数える。
    /// パスで書かれた型は**パスごと** 1 つの名前。
    type_names: BTreeSet<String>,
    /// impl の対象に置き換えられない Self、または Self を含む関連型が残るか。
    refers_to_self: bool,
    unopenable_alias: bool,
    unopenable_associated_type: bool,
}

impl RustCallable {
    /// hover が返した関数の綴りから読む。関数の宣言として読めなければ `None`。
    ///
    /// **const generics を持つ関数も `None`。** hover は使用箇所を `[u8; {const}]` と
    /// 綴り、引数の当てはめを扱わないため、読めた部分だけでは長さの違う配列が重なる。
    /// それ以外の配列長も、意味情報側で復元した整数だけを比較する。
    /// 型名の位置だけに、宣言側で正規化した右辺を差し込む。
    /// 束縛された型変数には差し込まない。`impl_header` は実ソースの直接の親 impl。
    pub(crate) fn from_spelling(
        spelling: &str,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
        impl_header: Option<&str>,
    ) -> Option<Self> {
        let function_spelling = match spelling
            .strip_prefix("impl ")
            .or_else(|| spelling.strip_prefix("impl<"))
        {
            Some(_) => {
                impl_header?;
                let (header, function) = spelling.split_once('\n')?;
                let header_source = format!("{header} {{}}");
                let header_tree = SyntaxTree::from_source(&header_source, Grammar::Rust).ok()?;
                if header_tree.has_error() {
                    return None;
                }
                function
            }
            None => spelling,
        };
        let declaration = match impl_header {
            Some(header) => format!("{header}\n{{ {function_spelling}{DECLARATION_TERMINATOR} }}"),
            None => format!("{function_spelling}{DECLARATION_TERMINATOR}"),
        };
        let tree = SyntaxTree::from_source(&declaration, Grammar::Rust).ok()?;
        if tree.has_error() {
            return None;
        }

        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == FUNCTION_SIGNATURE_KIND)?;

        let mut spelling = Spelling::new(&declaration);
        spelling.type_of = type_of;
        spelling.from_hover = true;
        let mut callable = spelling.callable_of(function)?;
        callable.renumber_variables();
        Some(callable)
    }

    /// エイリアスが引数の順序を変えても、展開後の初出順で比べる。
    fn renumber_variables(&mut self) {
        let mut method = Vec::new();
        let mut implementation = Vec::new();
        let mut renumber = |spelling: &str| -> String {
            let mut output = String::new();
            let mut rest = spelling;
            while let Some(position) = rest.find(['%', '"', '\'']) {
                if rest.as_bytes()[position] == b'\'' {
                    let end = character_literal_end(rest, position).unwrap_or(position + 1);
                    output.push_str(&rest[..end]);
                    rest = &rest[end..];
                    continue;
                }
                if rest.as_bytes()[position] == b'"' {
                    let end = string_literal_end(rest, position);
                    output.push_str(&rest[..end]);
                    rest = &rest[end..];
                    continue;
                }
                output.push_str(&rest[..position]);
                rest = &rest[position + 1..];
                let (numbers, prefix) = match rest.strip_prefix("impl") {
                    Some(suffix) => {
                        rest = suffix;
                        (&mut implementation, "%impl")
                    }
                    None => (&mut method, PLACEHOLDER_PREFIX),
                };
                let length = rest.bytes().take_while(u8::is_ascii_digit).count();
                if length == 0 {
                    output.push_str(prefix);
                    continue;
                }
                let original = &rest[..length];
                let index = match numbers.iter().position(|number| number == original) {
                    Some(index) => index,
                    None => {
                        numbers.push(original.to_owned());
                        numbers.len() - 1
                    }
                };
                output.push_str(&format!("{prefix}{index}"));
                rest = &rest[length..];
            }
            output.push_str(rest);
            output
        };
        self.parameters = self
            .parameters
            .iter()
            .map(|parameter| renumber(parameter))
            .collect();
        self.value_type = renumber(&self.value_type);
        self.trait_bounds = self
            .trait_bounds
            .iter()
            .map(|(left, bound)| (renumber(left), renumber(bound)))
            .collect();
    }

    /// 比較に残る綴りに現れた型名。名前順。
    pub(crate) fn type_names(&self) -> &BTreeSet<String> {
        &self.type_names
    }

    /// エイリアスの型引数を当てはめられなかったか。
    pub(crate) fn has_unopenable_alias(&self) -> bool {
        self.unopenable_alias
    }

    pub(crate) fn has_unopenable_associated_type(&self) -> bool {
        self.unopenable_associated_type
    }

    /// impl の文脈を使っても、Self の指す型を確定できない綴りが残るか。
    pub(crate) fn refers_to_self(&self) -> bool {
        self.refers_to_self
    }
}

/// 文字リテラルの終端。閉じる引用符を持たないライフタイムは `None`。
fn character_literal_end(source: &str, quote: usize) -> Option<usize> {
    let rest = source.get(quote + 1..)?;
    let first = rest.chars().next()?;
    let length = match first {
        '\\' => match rest.as_bytes().get(1)? {
            b'u' => rest.find('}')? + 1,
            b'x' => 4,
            _ => 2,
        },
        _ => first.len_utf8(),
    };
    (rest.as_bytes().get(length) == Some(&b'\'')).then_some(quote + 1 + length + 1)
}

/// 引用符から始まる文字列の終端。raw string の hash と通常の escape を区別する。
fn string_literal_end(source: &str, quote: usize) -> usize {
    let prefix = &source[..quote];
    let hashes = prefix
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'#')
        .count();
    let raw = prefix[..prefix.len() - hashes].ends_with('r');
    let bytes = source.as_bytes();
    let mut index = quote + 1;
    while index < bytes.len() {
        if !raw && bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        let closes = bytes[index] == b'"'
            && (!raw
                || bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#')));
        if closes {
            return index + 1 + if raw { hashes } else { 0 };
        }
        index += 1;
    }
    source.len()
}

/// Rust の宣言 hover を、開ける右辺・型エイリアスでない宣言・開けない宣言に分ける。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RustTypeResolution {
    Opened(String),
    Declared(String),
    Unresolved,
    Generic(RustGenericAlias),
    /// 関連型の RHS。型引数は captures（使用側の型変数名）→ RHS の Self → GAT の順に当てる。
    Associated {
        alias: RustGenericAlias,
        captures: Vec<String>,
        /// RHS に単独の Self があり、使用側の impl の対象型を当てるか。
        substitutes_self: bool,
    },
    NotAnAlias,
    Unopenable,
    ExpansionLimit,
}

/// 展開する右辺の上限。深さだけではタプルで倍増する連鎖を抑えられない。
const MAXIMUM_ALIAS_SPELLING_BYTES: usize = 64 * 1024;

impl RustTypeResolution {
    /// 可視性を含む宣言から、展開候補の右辺を正規化する。
    /// プリミティブの綴りが指す実際の型の確認は、呼び出し側が意味情報から行う。
    /// ライフタイム・const 引数、および未知の型構文は開かない。
    pub(crate) fn from_spelling(spelling: &str) -> Self {
        Self::from_spelling_with(spelling, &|_| None)
    }

    /// 宣言側で解決した名前だけを右辺に差し込む。導入した綴りは再解釈しない。
    pub(crate) fn from_spelling_with(
        spelling: &str,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Self {
        let declaration = format!("{}{DECLARATION_TERMINATOR}", spelling.trim_end_matches(';'));
        let Ok(tree) = SyntaxTree::from_source(&declaration, Grammar::Rust) else {
            return Self::Unopenable;
        };
        let nodes = tree.named_descendants();
        let Some(alias) = nodes.iter().find(|node| {
            matches!(node.kind(), "type_item" | "associated_type")
                && node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "source_file")
        }) else {
            // 欠けた右辺でも type 宣言と分かるものは、通常の型の宣言にしない。
            let has_alias_keyword = nodes.iter().any(|node| {
                if node.kind() != "ERROR"
                    || node
                        .parent()
                        .is_none_or(|parent| parent.kind() != "source_file")
                {
                    return false;
                }
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .any(|child| child.kind() == "type")
            });
            if has_alias_keyword {
                return Self::Unopenable;
            }
            return Self::NotAnAlias;
        };
        if tree.has_error() {
            return Self::Unopenable;
        }
        let Some(right) = alias.child_by_field_name("type") else {
            return Self::Unopenable;
        };
        if let Some(parameters) = alias.child_by_field_name("type_parameters") {
            return match RustGenericAlias::from_nodes(parameters, right, &declaration) {
                Some(alias) => Self::Generic(alias),
                None => Self::Unopenable,
            };
        }
        if !is_resolved_type_syntax(right, &declaration, type_of) {
            return Self::Unopenable;
        }
        let mut spelling = Spelling::new(&declaration);
        spelling.type_of = type_of;
        spelling.spelling_limit = Some(MAXIMUM_ALIAS_SPELLING_BYTES);
        let resolved = spelling.spelling_of(right);
        if spelling.expansion_limit_reached {
            return Self::ExpansionLimit;
        }
        if spelling.unopenable_alias {
            return Self::Unopenable;
        }
        match resolved {
            Some(Some(resolved)) => Self::Opened(resolved),
            Some(None) | None => Self::Unopenable,
        }
    }
}

/// 引数のないエイリアスの宣言ソースと、右辺の型名の問い合わせ位置。
/// プリミティブ表記も含む。配列長の評価は宣言側の位置から別に行う。
pub(crate) struct RustAliasSource {
    array_lengths: RustArrayLengths,
    references: Vec<TypeReference>,
}

impl RustAliasSource {
    /// 宣言名の位置が一致する型エイリアスを読む。対応外の宣言なら `None`。
    pub(crate) fn from_source(
        source: &str,
        position: crate::source_position::SourcePosition,
    ) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let alias = tree.named_descendants().into_iter().find(|node| {
            node.kind() == "type_item"
                && node
                    .child_by_field_name("name")
                    .and_then(|name| source_position_of(name, source))
                    == Some(position)
        })?;
        let unsupported =
            alias.has_error() || alias.child_by_field_name("type_parameters").is_some();
        if unsupported {
            return None;
        }
        let right = alias.child_by_field_name("type")?;
        let array_lengths = RustArrayLengths::from_node(alias, source, alias.end_byte())?;
        let masked = array_lengths.with_values(&vec![0; array_lengths.len()])?;
        let supported = matches!(
            RustTypeResolution::from_spelling_with(&masked, &|_| {
                Some(RustTypeResolution::Declared(String::new()))
            }),
            RustTypeResolution::Opened(_)
        );
        if !supported {
            return None;
        }
        let mut spelling = Spelling::new(source);
        spelling.spelling_of(right)??;
        Some(Self {
            array_lengths,
            references: spelling.references,
        })
    }

    pub(crate) fn array_lengths(&self) -> &RustArrayLengths {
        &self.array_lengths
    }

    /// 宣言側スコープから尋ねる型名。プリミティブ表記も含む。
    pub(crate) fn references(&self) -> &[TypeReference] {
        &self.references
    }
}

/// hover が単独のプリミティブ型の綴りを返したか。
pub(crate) fn primitive_spelling_of(spelling: &str) -> Option<String> {
    let source = format!("type Value = {spelling};");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).ok()?;
    if tree.has_error() {
        return None;
    }
    let alias = tree
        .named_descendants()
        .into_iter()
        .find(|node| node.kind() == "type_item")?;
    let right = alias.child_by_field_name("type")?;
    (right.kind() == "primitive_type").then(|| source[right.byte_range()].to_owned())
}

/// 名前は宣言側で解決できたものだけ許可する。定数式や関連型の選択は許可しない。
fn is_resolved_type_syntax(
    node: Node<'_>,
    source: &str,
    type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
) -> bool {
    match node.kind() {
        "type_identifier" => source
            .get(node.byte_range())
            .is_some_and(|name| name != SELF_TYPE && type_of(name).is_some()),
        "scoped_type_identifier" => {
            let ordinary_path = node.child_by_field_name("path").is_some_and(|path| {
                matches!(
                    path.kind(),
                    "identifier" | "scoped_identifier" | "self" | "super" | "crate"
                )
            });
            ordinary_path
                && source
                    .get(node.byte_range())
                    .is_some_and(|name| type_of(&collapsed(name)).is_some())
        }
        "generic_type" | "type_arguments" | "reference_type" | "pointer_type" | "tuple_type"
        | "array_type" | "function_type" | "parameters" | "function_modifiers" => {
            named_children_of(node).all(|child| is_resolved_type_syntax(child, source, type_of))
        }
        "primitive_type" | "unit_type" | "never_type" | "integer_literal" | "mutable_specifier" => {
            true
        }
        "lifetime" => source.get(node.byte_range()) == Some(STATIC_LIFETIME),
        _ => false,
    }
}

/// 宣言側で意味を確定できた、型引数付きエイリアスのテンプレート。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustGenericAlias {
    parameters: Vec<AliasParameter>,
    right: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AliasParameter {
    name: String,
    default: Option<String>,
}

impl RustGenericAlias {
    /// 型パラメータだけを持ち、右辺・既定値の名前が束縛内に閉じる宣言を読む。
    fn from_nodes(parameters: Node<'_>, right: Node<'_>, source: &str) -> Option<Self> {
        let names: Option<Vec<_>> = named_children_of(parameters)
            .map(|parameter| {
                Some(
                    source
                        .get(parameter.child_by_field_name("name")?.byte_range())?
                        .to_owned(),
                )
            })
            .collect();
        if !is_openable_type_syntax(right, source, &names?) {
            return None;
        }
        Self::from_nodes_with(parameters, right, source, &|_| None)
    }

    fn from_nodes_with(
        parameters: Node<'_>,
        right: Node<'_>,
        source: &str,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Option<Self> {
        let mut names = Vec::new();
        let mut parsed = Vec::new();
        for parameter in named_children_of(parameters) {
            if parameter.kind() != "type_parameter" {
                return None;
            }
            let name = source
                .get(parameter.child_by_field_name("name")?.byte_range())?
                .to_owned();
            if names.contains(&name) {
                return None;
            }
            let default = match parameter.child_by_field_name("default_type") {
                Some(default) => {
                    if !is_openable_type_syntax(default, source, &names) {
                        return None;
                    }
                    Some(Spelling::new(source).spelling_of(default)??)
                }
                None => None,
            };
            names.push(name.clone());
            parsed.push(AliasParameter { name, default });
        }
        let resolved_type = |name: &str| {
            if names.iter().any(|parameter| parameter == name) {
                return Some(RustTypeResolution::Declared(String::new()));
            }
            type_of(name)
        };
        if !is_resolved_type_syntax(right, source, &resolved_type) {
            return None;
        }
        let free_type = |name: &str| {
            if names.iter().any(|parameter| parameter == name) {
                return None;
            }
            type_of(name)
        };
        let mut spelling = Spelling::new(source);
        spelling.type_of = &free_type;
        spelling.spelling_limit = Some(MAXIMUM_ALIAS_SPELLING_BYTES);
        let right = spelling.spelling_of(right)??;
        if spelling.unopenable_alias {
            return None;
        }
        Some(Self {
            parameters: parsed,
            right,
        })
    }

    /// 正規化した型引数を当てはめる。既定値の置換も、右辺と同じ上限で組み立てる。
    fn instantiated(
        &self,
        arguments: &[String],
        limit: Option<usize>,
    ) -> Result<String, AliasInstantiationError> {
        if arguments.len() > self.parameters.len() {
            return Err(AliasInstantiationError::Unopenable);
        }
        let mut substitutions = Vec::new();
        for (index, parameter) in self.parameters.iter().enumerate() {
            let argument = match arguments.get(index) {
                Some(argument) => argument.clone(),
                None => substituted(
                    parameter
                        .default
                        .as_deref()
                        .ok_or(AliasInstantiationError::Unopenable)?,
                    &substitutions,
                    limit,
                )?,
            };
            substitutions.push((parameter.name.as_str(), argument));
        }
        substituted(&self.right, &substitutions, limit)
    }
}

enum AliasInstantiationError {
    Unopenable,
    ExpansionLimit,
}

/// 正規化した綴りの中の、型変数の名前の識別子だけを置換する。
/// 導入した引数は再走査しないので、宣言側の別の型変数に捕捉されない。
///
/// **空白ではなく識別子の境界で切る。** 宣言元の型の型引数は `@type(..)<T>` のように
/// 空白なしで綴られるので、空白で切ると `<T>` の `T` が置換されずに残り、
/// 別の impl の同名の型変数と綴りで一致してしまう。
/// 引用符の中（宣言元のパス）と、`%` / `@` / `'` に続く識別子（付け替えた型変数・
/// 宣言元の印・ライフタイム）は置換しない。
fn substituted(
    template: &str,
    substitutions: &[(&str, String)],
    limit: Option<usize>,
) -> Result<String, AliasInstantiationError> {
    let is_identifier = |character: char| character.is_alphanumeric() || character == '_';
    let mut output = String::new();
    let mut rest = template;
    while let Some(first) = rest.chars().next() {
        let (part, length) = match first {
            '"' => {
                let end = string_literal_end(rest, 0);
                (&rest[..end], end)
            }
            '%' | '@' | '\'' => {
                let end = first.len_utf8()
                    + rest[first.len_utf8()..]
                        .find(|character: char| !is_identifier(character))
                        .unwrap_or(rest.len() - first.len_utf8());
                (&rest[..end], end)
            }
            _ if is_identifier(first) => {
                let end = rest
                    .find(|character: char| !is_identifier(character))
                    .unwrap_or(rest.len());
                let token = &rest[..end];
                let replaced = substitutions
                    .iter()
                    .find(|(name, _)| *name == token)
                    .map_or(token, |(_, argument)| argument.as_str());
                (replaced, end)
            }
            _ => (&rest[..first.len_utf8()], first.len_utf8()),
        };
        let within_limit = limit.is_none_or(|limit| output.len() + part.len() <= limit);
        if !within_limit {
            return Err(AliasInstantiationError::ExpansionLimit);
        }
        output.push_str(part);
        rest = &rest[length..];
    }
    Ok(output)
}

/// 追加前に長さを確かめ、上限以上の中間文字列も作らない。
fn joined_with_limit<'part>(
    parts: impl IntoIterator<Item = &'part str>,
    separator: &str,
    limit: Option<usize>,
) -> Option<String> {
    let mut joined = String::new();
    for (index, part) in parts.into_iter().enumerate() {
        let separator = if index == 0 { "" } else { separator };
        let length = joined
            .len()
            .checked_add(separator.len())?
            .checked_add(part.len())?;
        if limit.is_some_and(|limit| length > limit) {
            return None;
        }
        joined.push_str(separator);
        joined.push_str(part);
    }
    Some(joined)
}

/// 宣言の名前の位置から、右辺のプリミティブ表記と問い合わせ位置を返す。
/// ソースの宣言を読めない、または正規化した右辺が hover と一致しなければ `None`。
pub(crate) fn primitive_references_of_alias(
    source: &str,
    position: crate::source_position::SourcePosition,
    expected: &RustTypeResolution,
) -> Option<Vec<TypeReference>> {
    let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
    let alias = tree.named_descendants().into_iter().find(|node| {
        node.kind() == "type_item"
            && node
                .child_by_field_name("name")
                .and_then(|name| source_position_of(name, source))
                == Some(position)
    })?;
    if alias.has_error() {
        return None;
    }
    let actual = RustTypeResolution::from_spelling(source.get(alias.byte_range())?);
    if &actual != expected {
        return None;
    }
    let right = alias.child_by_field_name("type")?;
    let defaults: Vec<_> = alias
        .child_by_field_name("type_parameters")
        .into_iter()
        .flat_map(named_children_of)
        .filter_map(|parameter| parameter.child_by_field_name("default_type"))
        .collect();
    tree.named_descendants()
        .into_iter()
        .filter(|node| {
            node.kind() == "primitive_type"
                && !matches!(expected, RustTypeResolution::Generic(alias) if alias.parameters.iter().any(|parameter| source.get(node.byte_range()) == Some(parameter.name.as_str())))
                && (right.byte_range().contains(&node.start_byte())
                    || defaults.iter().any(|default| default.byte_range().contains(&node.start_byte())))
        })
        .map(|node| {
            Some(TypeReference::new(
                source.get(node.byte_range())?.to_owned(),
                source_position_of(node, source)?,
            ))
        })
        .collect()
}

/// 展開候補にできる型構文か。未知の構文は許可しない。
/// プリミティブ名の shadow は構文だけでは分からず、意味情報を取る側で確認する。
fn is_openable_type_syntax(node: Node<'_>, source: &str, parameters: &[String]) -> bool {
    match node.kind() {
        "type_identifier" => source
            .get(node.byte_range())
            .is_some_and(|name| parameters.iter().any(|parameter| parameter == name)),
        "primitive_type" | "unit_type" | "never_type" | "integer_literal" | "mutable_specifier" => {
            true
        }
        "lifetime" => source.get(node.byte_range()) == Some(STATIC_LIFETIME),
        "reference_type" | "pointer_type" | "tuple_type" | "array_type" | "function_type"
        | "parameters" | "function_modifiers" => {
            named_children_of(node).all(|child| is_openable_type_syntax(child, source, parameters))
        }
        _ => false,
    }
}

/// 直接所属する impl のヘッダー。本体や外側の自由関数の impl は含めない。
pub(crate) fn impl_header_of(function: Node<'_>, source: &str) -> Option<String> {
    let implementation = enclosing_impl_of(function)?;
    let body = implementation.child_by_field_name("body")?;
    Some(
        source
            .get(implementation.start_byte()..body.start_byte())?
            .to_owned(),
    )
}

fn enclosing_impl_of(function: Node<'_>) -> Option<Node<'_>> {
    let body = function.parent()?;
    if body.kind() != "declaration_list" {
        return None;
    }
    body.parent().filter(|parent| parent.kind() == "impl_item")
}

fn signature_has_error(node: Node<'_>) -> bool {
    named_children_of(node)
        .filter(|child| node.child_by_field_name("body") != Some(*child))
        .any(|child| child.has_error())
}

/// ソースに書かれた関数の宣言から、比較に残る型名とその尋ねる位置を集める。
/// 1 つも書かれていなければ空。
///
/// `function` は本体のある関数の宣言（`function_item`）、`source` はそれを含む
/// ファイル全体のソース。
///
/// **hover の綴りを読むのと同じ歩き方で読む。** rust-analyzer の hover は境界を `where` へ
/// 寄せるが、**パスは書かれたとおりに綴る**（rust-analyzer 1.94.1 で実測）。同じ歩き方で
/// ソースを読めば、[`RustCallable::type_names`] と同じ綴りの名前が位置つきで取れる。
/// 歩き方を 2 つ持つと、片方だけが新しい形に追随したときに綴りが食い違う。
///
/// **読めなければ空。** シグネチャに構文エラーがある・歩き方の一覧に無い形がある
/// ときがこれで、比較に残る型名は記録が無いまま「尋ねていない」に倒れる（偽陰性）。
/// 構文エラーから回復した木の名前を採ると、**綴りごとに最初の 1 つ**しか覚えないので、
/// 回復が作った範囲の名前が同じ綴りの記録として先に入り、**別の宣言の場所で引かれうる**
/// （偽陽性）。**本体の構文エラーは見ない** — 本体は歩かず、hover の綴りにも現れない。
pub(crate) fn type_references_of(function: Node<'_>, source: &str) -> Vec<TypeReference> {
    if signature_has_error(function) {
        return Vec::new();
    }

    let mut spelling = Spelling::new(source);
    if spelling.callable_of(function).is_none() {
        return Vec::new();
    }

    spelling.references
}

/// 1 つの関数の綴りを読み進める途中の状態。
///
/// **付け替えの番号は読んだ順に振る**ので、引数 → 戻り値 → 境界の順に歩く。
struct Spelling<'source, 'tree> {
    source: &'source str,
    type_of: &'source dyn Fn(&str) -> Option<RustTypeResolution>,
    /// 関数が束縛した型変数の名前。宣言された順。
    declared: Vec<String>,
    impl_declared: Vec<String>,
    impl_numbered: Vec<String>,
    self_type: Option<Node<'tree>>,
    from_hover: bool,
    /// 付け替えた型変数の名前。**番号の順**。
    numbered: Vec<String>,
    type_names: BTreeSet<String>,
    /// 型名が書かれた位置。[`Spelling::type_names`] と同じ綴りで、**綴りごとに最初の 1 つ**。
    ///
    /// **綴りの書き手がソースのときだけ意味を持つ。** hover の綴りを読むときにも数えるが、
    /// それは hover の中の位置で、尋ねる先にはならない。
    references: Vec<TypeReference>,
    refers_to_self: bool,
    unopenable_alias: bool,
    unopenable_associated_type: bool,
    spelling_limit: Option<usize>,
    expansion_limit_reached: bool,
}

impl<'source, 'tree> Spelling<'source, 'tree> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            type_of: &|_| None,
            declared: Vec::new(),
            impl_declared: Vec::new(),
            impl_numbered: Vec::new(),
            self_type: None,
            from_hover: false,
            numbered: Vec::new(),
            type_names: BTreeSet::new(),
            references: Vec::new(),
            refers_to_self: false,
            unopenable_alias: false,
            unopenable_associated_type: false,
            spelling_limit: None,
            expansion_limit_reached: false,
        }
    }

    /// 関数の宣言のノードから組み立てる。読めない形があれば `None`。
    ///
    /// **本体の無い宣言（hover の綴り）と本体のある宣言（ソース）のどちらも読める。**
    /// 見るのは名前付きのフィールドと `where` 句だけで、本体には触れない。
    fn callable_of(&mut self, function: Node<'tree>) -> Option<RustCallable> {
        let modifiers = self.modifiers_of(function)?;
        let enclosing_impl = enclosing_impl_of(function);
        let mut impl_bounded = Vec::new();
        if let Some(implementation) = enclosing_impl {
            if signature_has_error(implementation) {
                return None;
            }
            if let Some(parameters) = implementation.child_by_field_name("type_parameters") {
                impl_bounded = self.declared_type_parameters_of(parameters)?;
            }
            self.impl_declared = self.declared.clone();
            self.self_type = implementation.child_by_field_name("type");
        }

        // 境界は型変数をすべて宣言し終えてから読む（`T: Into<U>` の `U` が後ろで宣言されうる）
        let mut bounded = Vec::new();
        if let Some(type_parameters) = function.child_by_field_name("type_parameters") {
            bounded = self.declared_type_parameters_of(type_parameters)?;
        }

        let parameter_list = function.child_by_field_name("parameters")?;
        let mut parameters = Vec::new();
        for parameter in
            named_children_of(parameter_list).filter(|node| node.kind() != ATTRIBUTE_KIND)
        {
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
            let mut cursor = clause.walk();
            for predicate in clause.named_children(&mut cursor) {
                if COMMENT_KINDS.contains(&predicate.kind()) {
                    let inherited_bounds = self.from_hover
                        && enclosing_impl.is_some()
                        && self.text_of(predicate)?.trim() == "// Bounds from impl:";
                    if inherited_bounds {
                        // impl の境界は、メソッドの型変数が shadow しない元のスコープから読む。
                        break;
                    }
                    continue;
                }
                self.insert_predicate(&mut trait_bounds, predicate)?;
            }
        }

        if let Some(implementation) = enclosing_impl {
            let method_declared = std::mem::replace(&mut self.declared, self.impl_declared.clone());
            for (left, bounds) in impl_bounded {
                self.insert_trait_bounds(&mut trait_bounds, left, bounds)?;
            }
            for clause in
                named_children_of(implementation).filter(|node| node.kind() == "where_clause")
            {
                for predicate in named_children_of(clause) {
                    self.insert_predicate(&mut trait_bounds, predicate)?;
                }
            }
            if let Some(implemented_trait) = implementation.child_by_field_name("trait") {
                let target = self.self_spelling()?;
                let bound = self.spelling_of(implemented_trait)??;
                trait_bounds.insert((target, bound));
            }
            self.declared = method_declared;
        }

        // どこにも現れない型変数も番号を持たせる。数が型の一部なので
        for name in self.declared.clone() {
            self.variable_spelling_of(&name);
        }

        Some(RustCallable {
            modifiers,
            parameters,
            value_type,
            type_parameter_count: self.declared.len() - self.impl_declared.len(),
            impl_type_parameter_count: self.impl_declared.len(),
            trait_bounds,
            type_names: self.type_names.clone(),
            refers_to_self: self.refers_to_self,
            unopenable_alias: self.unopenable_alias,
            unopenable_associated_type: self.unopenable_associated_type,
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
    fn declared_type_parameters_of<'node>(
        &mut self,
        type_parameters: Node<'node>,
    ) -> Option<Vec<(Node<'node>, Node<'node>)>> {
        let mut bounded = Vec::new();

        for parameter in named_children_of(type_parameters) {
            match parameter.kind() {
                // 落とす（[`RustCallable`] の doc）。`'a: 'b` もライフタイムどうしの約束なので一緒に落ちる
                "lifetime_parameter" => {}
                "type_parameter" => {
                    let name = parameter.child_by_field_name("name")?;
                    let name_text = self.text_of(name)?.to_owned();
                    if self.declared.contains(&name_text) {
                        return None;
                    }
                    self.declared.push(name_text);
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
                let target = self.self_spelling()?;
                let mut cursor = parameter.walk();
                let borrowed = parameter
                    .children(&mut cursor)
                    .any(|child| child.kind() == "&");
                if !borrowed {
                    return Some(target);
                }
                let mut parts = vec!["&".to_owned()];
                for child in named_children_of(parameter).filter(|child| child.kind() != "self") {
                    if let Some(part) = self.spelling_of(child)? {
                        parts.push(part);
                    }
                }
                parts.push(target);
                Some(parts.join(" "))
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
            "primitive_type" => self.primitive_spelling_of(node)?,
            "scoped_type_identifier" => self.scoped_type_spelling_of(node)?,
            // `?Sized` は `Sized` にしか付けられず、宣言を辿る相手ではない
            "removed_trait_bound" => collapsed(self.text_of(node)?),
            // 関連型の名前は、それを持つトレイトの側で決まる
            "type_binding" => {
                let name = self.text_of(node.child_by_field_name("name")?)?;
                let bound = node.child_by_field_name("type")?;
                format!("{name} = {}", self.spelling_of(bound)?.unwrap_or_default())
            }
            "generic_type" => self.generic_spelling_of(node)?,
            "array_type" => {
                let element = self.spelling_of(node.child_by_field_name("element")?)??;
                match node.child_by_field_name("length") {
                    Some(length) => {
                        let length = self.array_length_spelling_of(length)?;
                        format!("[ {element} ; {length} ]")
                    }
                    None => format!("[ {element} ]"),
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

        if self
            .spelling_limit
            .is_some_and(|limit| spelled.len() > limit)
        {
            self.expansion_limit_reached = true;
            return None;
        }
        Some(Some(spelled))
    }

    /// リテラルを整数へ揃える。ソースの未評価の式は問い合わせ位置の抽出用にだけ残す。
    fn array_length_spelling_of(&self, length: Node<'_>) -> Option<String> {
        let text = self.text_of(length)?;
        if length.kind() == "integer_literal" {
            if let Ok(value) = array_length::integer_of(text) {
                return Some(value.to_string());
            }
        }
        if self.from_hover {
            return None;
        }
        Some(collapsed(text))
    }

    /// 型引数を使用側で正規化してから、エイリアス全体へ当てはめる。
    fn generic_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        let head = node.child_by_field_name("type")?;
        let argument_nodes = node.child_by_field_name("type_arguments")?;
        let associated_head = contains_self_type(head, self.source);
        let name = match associated_head {
            true => projection_name_of(head, self.source)?,
            false => collapsed(self.text_of(head)?),
        };
        let bound_head = name.split("::").next().is_some_and(|head| {
            head == SELF_TYPE || self.declared.iter().any(|declared| declared == head)
        });
        let can_resolve_head = !bound_head || associated_head;
        if can_resolve_head {
            if let Some(alias) = (self.type_of)(&name).filter(|alias| {
                !matches!(
                    alias,
                    RustTypeResolution::NotAnAlias | RustTypeResolution::Unresolved
                )
            }) {
                if matches!(alias, RustTypeResolution::Declared(_)) {
                    let arguments = self.listed_spellings_of(argument_nodes)?;
                    return Some(self.resolved_spelling(alias, &arguments));
                }
                // ライフタイム・const・関連型束縛を型引数として数えてはいけない。
                let type_arguments_only = named_children_of(argument_nodes).all(|argument| {
                    matches!(
                        argument.kind(),
                        "primitive_type"
                            | "type_identifier"
                            | "scoped_type_identifier"
                            | "generic_type"
                            | "reference_type"
                            | "pointer_type"
                            | "tuple_type"
                            | "unit_type"
                            | "never_type"
                            | "array_type"
                            | "function_type"
                            | "dynamic_type"
                            | "abstract_type"
                            | "bounded_type"
                    )
                });
                if !type_arguments_only {
                    self.unopenable_associated_type |=
                        matches!(alias, RustTypeResolution::Associated { .. });
                    self.unopenable_alias = true;
                    return Some(name);
                }
                let arguments = self.listed_spellings_of(argument_nodes)?;
                return Some(self.resolved_spelling(alias, &arguments));
            }
        }
        let generic = self.spelling_of(head)?.unwrap_or_default();
        let arguments = self.listed_spellings_of(argument_nodes)?;
        if arguments.is_empty() {
            return Some(generic);
        }
        self.generic_spelling(&generic, &arguments)
    }

    fn generic_spelling(&mut self, head: &str, arguments: &[String]) -> Option<String> {
        let arguments = joined_with_limit(
            arguments.iter().map(String::as_str),
            ", ",
            self.spelling_limit,
        );
        let joined = arguments.and_then(|arguments| {
            joined_with_limit([head, "<", &arguments, ">"], "", self.spelling_limit)
        });
        if joined.is_none() {
            self.expansion_limit_reached = true;
        }
        joined
    }

    /// 具体的な型は引数を保ち、エイリアスは当てはめる。開けない形には印を残す。
    fn resolved_spelling(&mut self, alias: RustTypeResolution, arguments: &[String]) -> String {
        let associated = matches!(alias, RustTypeResolution::Associated { .. });
        let resolved = match alias {
            RustTypeResolution::Opened(right) => arguments
                .is_empty()
                .then_some(right)
                .ok_or(AliasInstantiationError::Unopenable),
            RustTypeResolution::Declared(identity) if arguments.is_empty() => Ok(identity),
            RustTypeResolution::Declared(identity) => self
                .generic_spelling(&identity, arguments)
                .ok_or(AliasInstantiationError::ExpansionLimit),
            RustTypeResolution::Generic(alias) => {
                alias.instantiated(arguments, self.spelling_limit)
            }
            RustTypeResolution::Associated {
                alias,
                captures,
                substitutes_self,
            } => {
                let mut captured: Option<Vec<_>> = captures
                    .iter()
                    .map(|name| self.variable_spelling_of(name))
                    .collect();
                // Why: RHS を書いた impl の対象型は、使用側の対象型と型変数の付け替えだけで一致する
                // （直接囲む impl はそれ自身、別の impl は束縛と宣言元の照合で確かめてある）。
                // 対象型に投影は現れないので、綴る途中でここへ再入しない。
                if substitutes_self {
                    let target = self.self_spelling();
                    captured = captured.zip(target).map(|(mut captured, target)| {
                        captured.push(target);
                        captured
                    });
                }
                match captured {
                    Some(mut captured) => {
                        captured.extend_from_slice(arguments);
                        alias.instantiated(&captured, self.spelling_limit)
                    }
                    None => Err(AliasInstantiationError::Unopenable),
                }
            }
            RustTypeResolution::ExpansionLimit => Err(AliasInstantiationError::ExpansionLimit),
            RustTypeResolution::NotAnAlias
            | RustTypeResolution::Unresolved
            | RustTypeResolution::Unopenable => Err(AliasInstantiationError::Unopenable),
        };
        match resolved {
            Ok(right) => right,
            Err(AliasInstantiationError::Unopenable) => {
                self.unopenable_associated_type |= associated;
                self.unopenable_alias = true;
                String::new()
            }
            Err(AliasInstantiationError::ExpansionLimit) => {
                self.expansion_limit_reached = true;
                self.unopenable_alias = true;
                String::new()
            }
        }
    }

    /// 名前付きの子を 1 つずつ綴り、落ちたものを除いて返す。読めなければ `None`。
    fn listed_spellings_of(&mut self, list: Node<'_>) -> Option<Vec<String>> {
        let mut spelled = Vec::new();
        let mut length = 0_usize;
        for child in named_children_of(list) {
            if let Some(child) = self.spelling_of(child)? {
                let separator_length = if spelled.is_empty() { 0 } else { 2 };
                length = length
                    .checked_add(child.len())?
                    .checked_add(separator_length)?;
                if self.spelling_limit.is_some_and(|limit| length > limit) {
                    self.expansion_limit_reached = true;
                    return None;
                }
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

        let mut spelled = String::new();
        let mut count = 0;
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        for child in children {
            if let Some(child) = self.spelling_of(child)? {
                let separator = if count == 0 { "" } else { " " };
                let length = spelled
                    .len()
                    .checked_add(separator.len())?
                    .checked_add(child.len())?;
                if self.spelling_limit.is_some_and(|limit| length > limit) {
                    self.expansion_limit_reached = true;
                    return None;
                }
                spelled.push_str(separator);
                spelled.push_str(&child);
                count += 1;
            }
        }

        Some(spelled)
    }

    /// 型名 1 つ。関数が束縛した型変数なら付け替え、そうでなければ型名として数える。
    fn type_identifier_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        let name = self.text_of(node)?;
        if name == SELF_TYPE {
            return self.self_spelling();
        }
        if let Some(variable) = self.variable_spelling_of(name) {
            return Some(variable);
        }

        if let Some(resolved) = (self.type_of)(name).filter(|alias| {
            !matches!(
                alias,
                RustTypeResolution::NotAnAlias | RustTypeResolution::Unresolved
            )
        }) {
            return Some(self.resolved_spelling(resolved, &[]));
        }

        self.insert_type_name(name.to_owned(), node);

        Some(name.to_owned())
    }

    /// プリミティブ表記も宣言側で別の型を指しうる。束縛された型変数には問い合わせない。
    fn primitive_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        let name = self.text_of(node)?;
        if let Some(variable) = self.variable_spelling_of(name) {
            return Some(variable);
        }
        if let Some(resolved) = (self.type_of)(name) {
            if matches!(
                resolved,
                RustTypeResolution::NotAnAlias | RustTypeResolution::Unresolved
            ) {
                self.insert_type_name(name.to_owned(), node);
                return Some(name.to_owned());
            }
            return Some(self.resolved_spelling(resolved, &[]));
        }
        if !self.from_hover {
            let already_asked = self
                .references
                .iter()
                .any(|reference| reference.name() == name);
            if !already_asked {
                self.references.push(TypeReference::new(
                    name.to_owned(),
                    source_position_of(node, self.source)?,
                ));
            }
        }
        Some(name.to_owned())
    }

    /// パスで書かれた型。
    ///
    /// **先頭が型変数なら関連型**（`T::Item`）で、型変数だけを付け替える。
    /// **先頭がモジュールなら、パスごと 1 つの型名。** 末尾の名前だけを数えると、
    /// 別のモジュールの同名の型と重なる。
    fn scoped_type_spelling_of(&mut self, node: Node<'_>) -> Option<String> {
        if contains_self_type(node, self.source) {
            // Why: RHS の Self は hover 側で対象型として綴られるので、その型名を尋ねる位置を
            // ソース側でも集める。receiver の無い inherent impl のメソッドは他に対象型を歩かない。
            if !self.from_hover && self.self_type.is_some() {
                self.self_spelling()?;
            }
            let name = projection_name_of(node, self.source)?;
            if let Some(resolved) = (self.type_of)(&name)
                .filter(|resolution| !matches!(resolution, RustTypeResolution::Unresolved))
            {
                return Some(self.resolved_spelling(resolved, &[]));
            }
            self.insert_type_name(name.clone(), node.child_by_field_name("name")?);
            self.refers_to_self = true;
            return Some(name);
        }
        let path = node.child_by_field_name("path")?;
        let name = self.text_of(node.child_by_field_name("name")?)?.to_owned();

        match path.kind() {
            "identifier" | "type_identifier" => {
                let head = self.text_of(path)?;
                if let Some(variable) = self.variable_spelling_of(head) {
                    return Some(format!("{variable}::{name}"));
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
    ///
    /// **尋ねる位置は末尾の名前。** 先頭（`billing::User` の `billing`）を指すと、
    /// 型ではなくモジュールの宣言が返る。
    fn whole_path_of(&mut self, node: Node<'_>) -> Option<String> {
        let path = collapsed(self.text_of(node)?);
        if let Some(resolved) = (self.type_of)(&path).filter(|alias| {
            !matches!(
                alias,
                RustTypeResolution::NotAnAlias | RustTypeResolution::Unresolved
            )
        }) {
            return Some(self.resolved_spelling(resolved, &[]));
        }
        self.insert_type_name(path.clone(), node.child_by_field_name("name")?);

        Some(path)
    }

    /// 比較に残る型名を 1 つ数え、`asked` をその綴りの尋ねる位置として覚える。
    ///
    /// **同じ綴りは最初の 1 つだけ覚える。** 1 つの関数の宣言の中では、同じ綴りは
    /// 同じスコープで解決されるので同じ宣言を指す（`Self` は数えない）。
    fn insert_type_name(&mut self, name: String, asked: Node<'_>) {
        let first = self.type_names.insert(name.clone());
        if !first {
            return;
        }

        // 位置が文字の境界に乗らない綴りは尋ねられない。数えた名前は記録が無いまま残り、
        // 「尋ねていない」に倒れる
        if let Some(position) = source_position_of(asked, self.source) {
            self.references.push(TypeReference::new(name, position));
        }
    }

    /// impl の対象を、メソッドの型変数に捕捉されないスコープで綴る。
    /// 対象が分からなければ場所依存のまま残す。
    fn self_spelling(&mut self) -> Option<String> {
        let Some(target) = self.self_type.take() else {
            self.refers_to_self = true;
            return Some(SELF_TYPE.to_owned());
        };
        let method_declared = std::mem::replace(&mut self.declared, self.impl_declared.clone());
        let spelling = self.spelling_of(target);
        self.declared = method_declared;
        self.self_type = Some(target);
        spelling?
    }

    /// 型変数を、impl とメソッドで別々の名前空間に付け替える。
    fn variable_spelling_of(&mut self, name: &str) -> Option<String> {
        if !self.declared.iter().any(|declared| declared == name) {
            return None;
        }
        let from_impl = self.impl_declared.iter().any(|declared| declared == name);
        let (numbered, prefix) = match from_impl {
            true => (&mut self.impl_numbered, "%impl"),
            false => (&mut self.numbered, PLACEHOLDER_PREFIX),
        };
        let number = match numbered.iter().position(|numbered| numbered == name) {
            Some(number) => number,
            None => {
                numbered.push(name.to_owned());
                numbered.len() - 1
            }
        };
        Some(format!("{prefix}{number}"))
    }

    fn text_of(&self, node: Node<'_>) -> Option<&'source str> {
        self.source.get(node.byte_range())
    }
}

/// 修飾された関連型の中で Self を参照するか。置換前の構文で確かめる。
fn contains_self_type(node: Node<'_>, source: &str) -> bool {
    let is_self = matches!(node.kind(), "type_identifier" | "identifier")
        && source.get(node.byte_range()) == Some(SELF_TYPE);
    is_self || named_children_of(node).any(|child| contains_self_type(child, source))
}

/// ソースと hover のパスの空白・コメントの違いを落とす。識別子間の区切りは保つ。
fn projection_name_of(node: Node<'_>, source: &str) -> Option<String> {
    if COMMENT_KINDS.contains(&node.kind()) {
        return Some(String::new());
    }
    if node.child_count() == 0 {
        return Some(source.get(node.byte_range())?.to_owned());
    }
    let mut output = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let part = projection_name_of(child, source)?;
        let identifier_boundary = output
            .chars()
            .last()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
            && part
                .chars()
                .next()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if identifier_boundary {
            output.push(' ');
        }
        output.push_str(&part);
    }
    Some(output)
}

/// `+` で並ぶ要求を集めた途中の形。
#[derive(Default)]
struct BoundedMembers {
    /// `dyn` / `impl`。並び全体に掛かる。
    keyword: Option<String>,
    /// 要求 1 つずつの綴り。
    spelled: Vec<String>,
}

/// 名前付きの子。書かれた順。コメント（[`COMMENT_KINDS`]）は除く。
fn named_children_of(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node
        .named_children(&mut cursor)
        .filter(|child| !COMMENT_KINDS.contains(&child.kind()))
        .collect();
    children.into_iter()
}

/// 空白の並びを 1 つに畳む。hover は `where` 句を改行と字下げで返す。
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_alias_source_names_are_queried_at_the_selected_declaration_in_utf16() {
        let source =
            "mod a { type Alias = Wrong; }\nmod b { /*🦀*/ type Alias = (other::Customer, u8); }";
        let line = crate::line_number::LineNumber::from_index(1);
        let position = crate::source_position::SourcePosition::from_preceding_text(
            line,
            "mod b { /*🦀*/ type ",
        );
        let body = super::RustAliasSource::from_source(source, position).unwrap();
        let references = body.references();
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].name(), "other::Customer");
        assert_eq!(
            references[0].position(),
            crate::source_position::SourcePosition::from_preceding_text(
                line,
                "mod b { /*🦀*/ type Alias = (other::"
            )
        );
        assert_eq!(references[1].name(), "u8");
    }

    #[test]
    fn test_alias_source_rejects_unknown_type_syntax_and_generic_declarations() {
        for right in ["generated!()", "Self", "&'a u8", "<Customer as Trait>::Out"] {
            let source = format!("type Alias = {right};");
            let position = crate::source_position::SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::from_index(0),
                "type ",
            );
            assert!(
                super::RustAliasSource::from_source(&source, position).is_none(),
                "{right}"
            );
        }
        let source = "type Alias<T> = T;";
        let position = crate::source_position::SourcePosition::from_preceding_text(
            crate::line_number::LineNumber::from_index(0),
            "type ",
        );
        assert!(super::RustAliasSource::from_source(source, position).is_none());
        assert!(super::RustAliasSource::from_source("type Alias = [u8; 4];", position).is_some());
    }

    #[test]
    fn test_alias_declared_names_cannot_capture_use_site_type_variables() {
        let nominal = super::RustTypeResolution::Declared("@type(\"/model.rs\", 1, 0)".to_owned());
        let alias =
            super::RustTypeResolution::from_spelling_with("type Alias = Customer", &|name| {
                (name == "Customer").then(|| nominal.clone())
            });
        let captured = super::RustCallable::from_spelling(
            "fn f<Customer>(value: Alias, other: Customer) -> Alias",
            &|name| (name == "Alias").then(|| alias.clone()),
            None,
        )
        .unwrap();
        let explicit = super::RustCallable::from_spelling(
            "fn g<T>(value: Customer, other: T) -> Customer",
            &|name| (name == "Customer").then(|| nominal.clone()),
            None,
        )
        .unwrap();
        assert_eq!(captured, explicit);
    }

    #[test]
    fn test_alias_resolution_keeps_nominal_generic_arguments_and_rejects_invalid_alias_arguments() {
        let nominal = super::RustTypeResolution::Declared("@type(\"/model.rs\", 1, 0)".to_owned());
        let type_of = |name: &str| (name == "Wrapper").then(|| nominal.clone());
        let first =
            super::RustTypeResolution::from_spelling_with("type Alias = Wrapper<u8>", &type_of);
        let second =
            super::RustTypeResolution::from_spelling_with("type Alias = Wrapper<u64>", &type_of);
        assert!(matches!(first, super::RustTypeResolution::Opened(_)));
        assert_ne!(first, second);
        let alias = super::RustTypeResolution::from_spelling("type Pair<T> = (T, T)");
        let invalid =
            super::RustTypeResolution::from_spelling_with("type Alias = Pair<u8, u64>", &|_| {
                Some(alias.clone())
            });
        assert_eq!(invalid, super::RustTypeResolution::Unopenable);
    }
    #[test]
    fn test_alias_expansion_size_accepts_the_boundary_and_rejects_wide_right_hand_sides() {
        let limit = super::MAXIMUM_ALIAS_SPELLING_BYTES;
        let at_limit = super::RustTypeResolution::Opened("x".repeat(limit));
        let type_of = |name: &str| (name == "Large").then(|| at_limit.clone());
        assert_eq!(
            super::RustTypeResolution::from_spelling_with("type Alias = Large", &type_of),
            at_limit,
        );
        for declaration in ["type Alias = (Large, Large)", "type Alias = Wrapper<Large>"] {
            let resolved =
                super::RustTypeResolution::from_spelling_with(declaration, &|name| match name {
                    "Large" => Some(at_limit.clone()),
                    "Wrapper" => Some(super::RustTypeResolution::Declared("wrapper".to_owned())),
                    _ => None,
                });
            assert_eq!(resolved, super::RustTypeResolution::ExpansionLimit);
        }
    }

    #[test]
    fn test_alias_expansion_size_limits_generic_substitution_and_defaults() {
        for declaration in ["type Pair<T> = (T, T)", "type Pair<T, U = (T, T)> = U"] {
            let generic = super::RustTypeResolution::from_spelling(declaration);
            let resolved = super::RustTypeResolution::from_spelling_with(
                "type Alias = Pair<Large>",
                &|name| match name {
                    "Large" => Some(super::RustTypeResolution::Opened(
                        "x".repeat(super::MAXIMUM_ALIAS_SPELLING_BYTES / 2),
                    )),
                    "Pair" => Some(generic.clone()),
                    _ => None,
                },
            );
            assert_eq!(resolved, super::RustTypeResolution::ExpansionLimit);
        }
    }

    #[test]
    fn test_impl_type_variables_are_renamed_with_their_bounds() {
        let first = RustCallable::from_spelling(
            "fn a(value: T) -> Self",
            &|_| None,
            Some("impl<T: Clone> Holder<T>"),
        )
        .unwrap();
        let second = RustCallable::from_spelling(
            "fn b(value: U) -> Holder<U>",
            &|_| None,
            Some("impl<U> Holder<U> where U: Clone"),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.type_names(),
            &BTreeSet::from(["Clone".to_owned(), "Holder".to_owned()])
        );
    }

    #[test]
    fn test_impl_and_method_variables_keep_their_binding_scope() {
        let first = RustCallable::from_spelling(
            "fn a<U>(value: T) -> U",
            &|_| None,
            Some("impl<T> Holder<T>"),
        )
        .unwrap();
        let second = RustCallable::from_spelling(
            "fn b<U>(value: U) -> T",
            &|_| None,
            Some("impl<T> Holder<T>"),
        )
        .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn test_impl_receiver_borrow_and_mutability_are_preserved() {
        let read =
            |text| RustCallable::from_spelling(text, &|_| None, Some("impl Ledger")).unwrap();
        assert_eq!(read("fn a(mut self)"), read("fn b(self)"));
        assert_eq!(read("fn a(&'a mut self)"), read("fn b(value: &mut Ledger)"));
        assert_eq!(
            read("fn a(self: Box<Self>)"),
            read("fn b(value: Box<Ledger>)")
        );
        assert_ne!(read("fn a(&self)"), read("fn b(&mut self)"));
        assert_ne!(read("fn a(self)"), read("fn b(&self)"));
        assert_ne!(read("fn a(&'static self)"), read("fn b(&self)"));
    }

    #[test]
    fn test_impl_bounds_and_target_ignore_method_type_name_shadowing() {
        let header = "impl<T: Bound> Holder<T>";
        let source_style = RustCallable::from_spelling(
            "fn a<Bound>(value: Bound) -> Self",
            &|_| None,
            Some(header),
        )
        .unwrap();
        let hover_style = RustCallable::from_spelling("impl<T> Holder<T>\nfn b<Other>(value: Other) -> Self\nwhere\n// Bounds from impl:\nT: Bound,", &|_| None, Some(header)).unwrap();
        assert_eq!(source_style, hover_style);
        let target_shadow = RustCallable::from_spelling(
            "fn a<Holder>(value: Holder) -> Self",
            &|_| None,
            Some(header),
        )
        .unwrap();
        assert_eq!(target_shadow, hover_style);
    }

    #[test]
    fn test_impl_different_targets_and_bounds_do_not_match() {
        let read =
            |header| RustCallable::from_spelling("fn a(&self)", &|_| None, Some(header)).unwrap();
        assert_ne!(read("impl A"), read("impl B"));
        assert_ne!(
            read("impl<T: Clone> Holder<T>"),
            read("impl<T: Send> Holder<T>")
        );
        assert_ne!(read("impl TraitA for A"), read("impl TraitB for A"));
    }

    #[test]
    fn test_impl_self_projections_remain_site_dependent() {
        for result in [
            "Self::Item",
            "<Self as Iterator>::Item",
            "Self::Item::Nested",
            "Self::Item<u8>",
            "<Self::Item as Trait>::Out",
        ] {
            let spelling = format!("fn a(&self) -> {result}");
            let callable =
                RustCallable::from_spelling(&spelling, &|_| None, Some("impl Iterator for Holder"))
                    .unwrap();
            assert!(callable.refers_to_self(), "{result}");
        }
        assert!(
            !RustCallable::from_spelling("fn a(&self) -> Self", &|_| None, Some("impl Holder"))
                .unwrap()
                .refers_to_self()
        );
    }

    #[test]
    fn test_impl_type_references_include_bounds_at_their_source_positions() {
        let source = "impl<T: Bound> Holder<T> where T: Other { fn f(&self, x: T) {} }";
        let references = references_of(source);
        assert_eq!(
            references,
            vec![
                ("Holder".to_owned(), 1, source.find("Holder").unwrap()),
                ("Bound".to_owned(), 1, source.find("Bound").unwrap()),
                ("Other".to_owned(), 1, source.find("Other").unwrap()),
            ]
        );
    }

    #[test]
    fn test_impl_context_does_not_reach_nested_free_functions() {
        let source = "impl<T: Bound> Holder<T> { fn outer() { fn inner(x: User) {} } }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let inner = tree
            .named_descendants()
            .into_iter()
            .rfind(|node| node.kind() == "function_item")
            .unwrap();
        assert_eq!(impl_header_of(inner, source), None);
        assert_eq!(
            type_references_of(inner, source)
                .iter()
                .map(|r| r.name())
                .collect::<Vec<_>>(),
            vec!["User"]
        );
    }

    #[test]
    fn test_impl_const_parameters_remain_unreadable() {
        assert_eq!(
            RustCallable::from_spelling(
                "fn f(&self)",
                &|_| None,
                Some("impl<const N: usize> Holder<N>")
            ),
            None
        );
    }

    #[test]
    fn test_impl_self_and_receiver_match_the_explicit_target_type() {
        let implicit =
            RustCallable::from_spelling("fn a(&self) -> Self", &|_| None, Some("impl Ledger "))
                .unwrap();
        let explicit = RustCallable::from_spelling(
            "fn b(value: &Ledger) -> Ledger",
            &|_| None,
            Some("impl Ledger "),
        )
        .unwrap();
        assert_eq!(implicit, explicit);
        assert!(!implicit.refers_to_self());
        assert_eq!(
            implicit.type_names(),
            &BTreeSet::from(["Ledger".to_owned()])
        );
    }

    #[test]
    fn test_rust_alias_primitive_positions_follow_the_selected_declaration_in_utf16() {
        let source = "mod a { type Value = u8; }\nmod b { /*🦀*/ type Value = u64; }";
        let line = crate::line_number::LineNumber::from_index(1);
        let position = crate::source_position::SourcePosition::from_preceding_text(
            line,
            "mod b { /*🦀*/ type ",
        );
        let references = super::primitive_references_of_alias(
            source,
            position,
            &super::RustTypeResolution::Opened("u64".to_owned()),
        )
        .expect("宣言の右辺が一致する");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].name(), "u64");
        assert_eq!(
            references[0].position(),
            crate::source_position::SourcePosition::from_preceding_text(
                line,
                "mod b { /*🦀*/ type Value = "
            )
        );
        assert_eq!(
            super::primitive_references_of_alias(
                source,
                position,
                &super::RustTypeResolution::Opened("u8".to_owned())
            ),
            None
        );
    }

    #[test]
    fn test_rust_generic_alias_arguments_are_instantiated() {
        let alias = super::RustTypeResolution::from_spelling("type Pair<T> = (T, T)");
        let actual = RustCallable::from_spelling(
            "fn f(value: Pair<u8>)",
            &|name| (name == "Pair").then(|| alias.clone()),
            None,
        )
        .expect("型引数付き関数を読める");
        assert_eq!(actual, read("fn f(value: (u8, u8))"));
    }

    #[test]
    fn test_rust_generic_alias_defaults_nesting_and_capture_avoidance() {
        for (declaration, actual, expected) in [
            (
                "type Pair<T> = (T, T)",
                "fn f(x: Pair<Pair<u8>>)",
                "fn f(x: ((u8,u8),(u8,u8)))",
            ),
            (
                "type Pair<T = u8, U = T> = (T, U)",
                "fn f(x: Pair)",
                "fn f(x: (u8,u8))",
            ),
            (
                "type Pair<T = u8, U = T> = (T, U)",
                "fn f(x: Pair<u64>)",
                "fn f(x: (u64,u64))",
            ),
            (
                "type Pair<T = u8, U = [T; 2]> = (T, U)",
                "fn f(x: Pair<u64>)",
                "fn f(x: (u64,[u64;2]))",
            ),
            (
                "type Pair<T, U> = (T, U)",
                "fn f<U>(x: Pair<U,u8>, y: U)",
                "fn f<V>(x: (V,u8), y: V)",
            ),
            (
                "type Pair<T, U> = (U, T)",
                "fn f<T,U>(x: Pair<T,U>, y: T)",
                "fn f<A,B>(x: (B,A), y: A)",
            ),
            (
                "type Pair<T> = (T, TT)",
                "fn f<T>(x: Pair<T>)",
                "fn f<T>(x: Pair<T>)",
            ),
            (
                "type Pair<T> = (T, T)\nwhere T: Copy,",
                "fn f(x: Pair<u8>)",
                "fn f(x: (u8,u8))",
            ),
            (
                "type Pair<u8> = (u8, u8)",
                "fn f(x: Pair<u64>)",
                "fn f(x: (u64,u64))",
            ),
        ] {
            let alias = super::RustTypeResolution::from_spelling(declaration);
            if declaration.contains("TT") {
                assert_eq!(alias, super::RustTypeResolution::Unopenable);
                continue;
            }
            let actual = RustCallable::from_spelling(
                actual,
                &|name| (name == "Pair").then(|| alias.clone()),
                None,
            )
            .unwrap();
            assert_eq!(actual, read(expected), "{declaration}");
        }
    }

    #[test]
    fn test_rust_generic_alias_invalid_arguments_are_unopenable() {
        let alias = super::RustTypeResolution::from_spelling("type Pair<T> = (T, T)");
        for signature in [
            "fn f(x: Pair)",
            "fn f(x: Pair<u8,u64>)",
            "fn f(x: Pair<'static>)",
            "fn f(x: Pair<3>)",
            "fn f(x: Pair<true>)",
            "fn f(x: Pair<'x'>)",
        ] {
            let actual = RustCallable::from_spelling(
                signature,
                &|name| (name == "Pair").then(|| alias.clone()),
                None,
            )
            .unwrap_or_else(|| panic!("型として読める: {signature}"));
            assert!(actual.has_unopenable_alias(), "{signature}");
        }
    }

    #[test]
    fn test_generic_alias_integer_array_defaults_match_direct_integer_spellings() {
        let alias =
            super::RustTypeResolution::from_spelling("type Pair<T, U = [T; 0x2_usize]> = (T, U)");
        let aliased = RustCallable::from_spelling(
            "fn f(a: Pair<u64>)",
            &|name| (name == "Pair").then(|| alias.clone()),
            None,
        )
        .unwrap();
        assert_eq!(aliased, read("fn g(a: (u64, [u64; 2]))"));
    }

    #[test]
    fn test_nested_array_lengths_keep_the_outer_length_distinct() {
        assert_ne!(read("fn f(a: [[u8; 2]; 3])"), read("fn f(a: [[u8; 2]; 4])"));
    }

    #[test]
    fn test_rust_unevaluated_array_expressions_are_not_comparable() {
        for signature in [
            r#"fn f(x: [u8; b"%1"[1] as usize])"#,
            r##"fn f(x: [u8; br#"\"%1"#[2] as usize])"##,
            "fn f(x: [u8; 4 % 3])",
            "fn f(x: [u8; COUNT])",
            "fn f(x: [u8; {const}])",
        ] {
            assert!(
                RustCallable::from_spelling(signature, &|_| None, None).is_none(),
                "{signature}"
            );
        }
    }

    #[test]
    fn test_rust_alias_variable_order_after_an_array_length_is_preserved() {
        let alias = super::RustTypeResolution::from_spelling("type Flip<A,B> = (B,A)");
        let actual = RustCallable::from_spelling(
            "fn f<T,U>(x: ([u8; 34], Flip<T,U>), y:T)",
            &|name| (name == "Flip").then(|| alias.clone()),
            None,
        )
        .unwrap();
        let expected = read("fn g<A,B>(x: ([u8; 34], (B,A)), y:A)");
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rust_generic_alias_external_defaults_cannot_capture_use_site_names() {
        for declaration in [
            "type Pair<T = Customer> = (T,T)",
            "type Pair<T = U, U = u8> = (T,U)",
            "type Pair<T> = (T, T::Item)",
            "type Pair<T> = (T, [u8; COUNT])",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(declaration),
                super::RustTypeResolution::Unopenable,
                "{declaration}"
            );
        }
    }

    #[test]
    fn test_rust_alias_visibility_does_not_change_its_right_hand_side() {
        for declaration in [
            "type Amount = u64",
            "pub type Amount = u64;",
            "pub(crate) type Amount = u64",
            "pub(in crate::billing) type Amount = u64",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(declaration),
                super::RustTypeResolution::Opened("u64".to_owned()),
                "{declaration}"
            );
        }
    }

    #[test]
    fn test_rust_alias_nested_scope_independent_types_are_opened() {
        for right in [
            "(u8, u64)",
            "&'static [u8]",
            "[u8; 4]",
            "*const u8",
            "fn(u8) -> u64",
            "()",
            "!",
        ] {
            let declaration = format!("pub type Value = {right}");
            let super::RustTypeResolution::Opened(resolved) =
                super::RustTypeResolution::from_spelling(&declaration)
            else {
                panic!("右辺を開ける: {declaration}");
            };
            assert_eq!(
                read(&format!("fn f() -> {resolved}")),
                read(&format!("fn g() -> {right}"))
            );
        }
    }

    #[test]
    fn test_rust_alias_generic_parameters_cannot_be_opened() {
        for declaration in [
            "type Bytes<'a> = &'a [u8]",
            "type Bytes<const N: usize> = [u8; N]",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(declaration),
                super::RustTypeResolution::Unopenable,
                "{declaration}"
            );
        }
    }

    #[test]
    fn test_rust_alias_scope_dependent_right_hand_sides_cannot_be_opened() {
        for right in [
            "Customer",
            "module::Customer",
            "Self",
            "[u8; COUNT]",
            "[u8; size_of::<u8>()]",
            "generated!()",
            "&'a u8",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(&format!("type Value = {right}")),
                super::RustTypeResolution::Unopenable,
                "{right}"
            );
        }
        assert!(matches!(
            super::RustTypeResolution::from_spelling("type Value = [u8; 4]"),
            super::RustTypeResolution::Opened(_)
        ));
    }

    #[test]
    fn test_rust_alias_missing_or_malformed_right_hand_side_is_unopenable() {
        for declaration in [
            "type Amount",
            "pub type Amount =",
            "pub type Amount = (u8,",
            "pub type Amount = {unknown}",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(declaration),
                super::RustTypeResolution::Unopenable,
                "{declaration}"
            );
        }
    }

    #[test]
    fn test_rust_nominal_declarations_are_not_aliases() {
        for declaration in [
            "pub struct User",
            "pub enum Color",
            "pub trait Display",
            "pub trait Iterator { type Item; }",
            "pub struct User { r#type: u8 }",
        ] {
            assert_eq!(
                super::RustTypeResolution::from_spelling(declaration),
                super::RustTypeResolution::NotAnAlias,
                "{declaration}"
            );
        }
    }

    use super::*;

    #[test]
    fn test_substituted_replaces_whole_identifiers_outside_identities_and_placeholders() {
        let substitutions = [
            ("T", "X".to_owned()),
            ("impl0", "Y".to_owned()),
            ("static", "Z".to_owned()),
        ];
        let actual = substituted(
            "(@type(\"a/T.rs\", 1, 2)<T>, TT, T, %impl0, &'static T, \"q\\\"T\")",
            &substitutions,
            None,
        );
        assert_eq!(
            actual.ok().as_deref(),
            Some("(@type(\"a/T.rs\", 1, 2)<X>, TT, X, %impl0, &'static X, \"q\\\"T\")")
        );
    }

    #[test]
    fn test_substituted_fails_only_when_the_result_exceeds_the_limit() {
        let substitutions = [("T", "abc".to_owned())];
        assert_eq!(
            substituted("<T>", &substitutions, Some(5)).ok().as_deref(),
            Some("<abc>")
        );
        assert!(matches!(
            substituted("<T>", &substitutions, Some(4)),
            Err(AliasInstantiationError::ExpansionLimit)
        ));
    }

    fn read(spelling: &str) -> RustCallable {
        RustCallable::from_spelling(spelling, &|_| None, None).expect("関数の綴りとして読める")
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
            RustCallable::from_spelling(
                "pub fn h<const N: usize>(x: [u8; {const}]) -> (i32, u8)",
                &|_| None,
                None
            ),
            None
        );
    }

    #[test]
    fn test_from_spelling_non_function_is_unreadable() {
        assert_eq!(
            RustCallable::from_spelling("pub struct S", &|_| None, None),
            None
        );
    }

    /// ソースに書かれた関数 1 つから集めた型名の、綴りと位置（行, 列）の組。
    fn references_of(source: &str) -> Vec<(String, usize, usize)> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).expect("Rust として読める");
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .expect("関数の宣言がある");

        type_references_of(function, source)
            .iter()
            .map(|reference| {
                let position = reference.position();
                (
                    reference.name().to_owned(),
                    position.line().get(),
                    position.character(),
                )
            })
            .collect()
    }

    /// 集めた型名の綴りだけ。名前順。
    fn reference_names_of(source: &str) -> BTreeSet<String> {
        references_of(source)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    }

    #[test]
    fn test_type_references_of_a_source_function_are_the_type_names_of_its_hover_spelling() {
        // hover は inline の境界を `where` へ寄せ、パスを書かれたとおりに綴る
        // （rust-analyzer 1.94.1 で実測）。比較に残る型名と同じ綴りで集めないと、
        // 集めた記録を綴りで引けない
        let source = "pub fn shown<T: Display>(user: billing::User, items: &[T]) -> Option<Vec<String>>\nwhere\n    T: Clone,\n{\n    None\n}\n";
        let hover = "pub fn shown<T>(user: billing::User, items: &[T]) -> Option<Vec<String>>\nwhere\n    T: Display + Clone,";

        assert_eq!(reference_names_of(source), read(hover).type_names().clone());
    }

    #[test]
    fn test_type_references_of_a_path_point_at_its_last_segment() {
        // 綴りはパスごと、尋ねる位置は末尾の名前。先頭の `billing` を指すと
        // モジュールの宣言が返る
        let references = references_of("fn scoped(user: billing::User) {}\n");

        assert_eq!(references, vec![("billing::User".to_owned(), 1, 25)]);
    }

    #[test]
    fn test_type_references_of_a_source_function_leave_out_what_has_no_declaration_to_trace() {
        // Why: 型変数・関連型名は除き、shadow されうるプリミティブ表記は尋ねる。
        let names = reference_names_of(
            "fn f<T: Iterator<Item = u8>>(t: T, n: usize) -> T::Item { todo!() }\n",
        );

        assert_eq!(
            names,
            BTreeSet::from(["Iterator".to_owned(), "u8".to_owned(), "usize".to_owned()])
        );
    }

    #[test]
    fn test_type_references_of_a_source_function_name_the_same_type_only_once() {
        let references = references_of("fn f(a: User, b: User) -> User { a }\n");

        assert_eq!(references, vec![("User".to_owned(), 1, 8)]);
    }

    #[test]
    fn test_type_references_of_a_source_function_skip_comments_and_attributes_in_parameters() {
        // hover の綴りには現れないので、読めないとして落とすと
        // 注釈を書いてある関数が「尋ねていない」側へ倒れる
        let names = reference_names_of("fn f(/* 金額 */ a: User, #[allow(unused)] b: Amount) {}\n");

        assert_eq!(
            names,
            BTreeSet::from(["Amount".to_owned(), "User".to_owned()])
        );
    }

    #[test]
    fn test_type_references_of_a_source_function_with_a_broken_signature_are_empty() {
        // 読めない形から名前を拾うと、hover の綴りに無い名前まで尋ねることになる。
        // 空なら比較に残る型名は「尋ねていない」に倒れる（偽陰性）
        let names = reference_names_of("fn f(a: User, b: Vec<Amount $>) {}\n");

        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn test_type_references_of_a_source_function_with_a_broken_body_are_still_collected() {
        // 対照は上のテスト。本体は歩かないので、本体の構文エラーで捨てると
        // 書きかけの関数の型名がまとめて「尋ねていない」へ倒れる
        let names = reference_names_of("fn f(a: User) -> Total {\n    let x = ;\n}\n");

        assert_eq!(
            names,
            BTreeSet::from(["Total".to_owned(), "User".to_owned()])
        );
    }

    #[test]
    fn test_type_references_of_a_source_method_trace_the_impl_target_for_self() {
        // Self の問い合わせ先はメソッドの綴りではなく impl の対象型。
        let names = reference_names_of("impl A { fn f(&self, user: User) -> Self { todo!() } }\n");

        assert_eq!(names, BTreeSet::from(["A".to_owned(), "User".to_owned()]));
    }
}
