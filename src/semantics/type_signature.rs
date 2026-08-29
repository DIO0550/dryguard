//! hover が返した綴りを、単一化の可否を比べられる形へ直す。
//!
//! 綴りそのまま（`signature text`）と、正規化した形（`type signature`）を分けている
//! (`rules/naming.md`「このツールの語彙を固定する」)。**綴りのまま比べると、
//! 引数名や型変数名が違うだけのペアが別物になる。**
//!
//! LSP を呼ばないので、サーバが無くてもここは確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::{BTreeSet, HashMap};

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

/// 単一化の可否を比べられる形に直した型シグネチャ。
///
/// 引数名を落とし、型変数を出現順に付け替えてある。**同じ形になった 2 つは
/// 単一化できる**（[`TypeSignature::is_unifiable_with`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSignature {
    /// 型変数。付け替え後の並び。
    type_parameters: Vec<TypeParameter>,
    /// 引数の型。名前を落とし、`?` と `...` は型の一部として残す。
    parameters: Vec<String>,
    /// 戻り値の型。
    return_type: String,
}

impl TypeSignature {
    /// hover が返した綴りから組み立てる。
    ///
    /// `text` は正規化前の綴り（`lsp` の `Session::hover` が返すもの）。サーバごとに
    /// 宣言形（`function decl(a: string): number`）と値形
    /// （`const arrow: (a: string) => number`）の 2 通りがあり、どちらも同じ形へ直す。
    ///
    /// 引数リストと戻り値の型を読み取れない綴りでは `None`。**空の引数リストで
    /// 埋めない**ので、後段は「引数が無い関数」と「読めなかった」を区別できる
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    pub fn from_signature_text(text: &str) -> Option<Self> {
        let flattened = flattened(text);
        let (prefix, parameter_list, after_parameters) = parameter_list_of(&flattened)?;
        let return_type = normalized_type(return_type_of(after_parameters)?)?;
        let parameters = parameter_types_of(parameter_list)?;
        let declared = declared_type_parameters_of(prefix);

        let ordered = ordered_names_of(
            &declared,
            parameters
                .iter()
                .chain(std::iter::once(&return_type))
                .map(String::as_str),
        );
        let placeholders = placeholders_of(&ordered);

        Some(Self {
            type_parameters: type_parameters_of(&declared, &ordered, &placeholders),
            parameters: parameters
                .iter()
                .map(|parameter| renamed(parameter, &placeholders))
                .collect(),
            return_type: renamed(&return_type, &placeholders),
        })
    }

    /// 2 つの型シグネチャが同じ型構造に重なるか。
    ///
    /// 引数名と型変数名の違いは正規化の時点で消えているので、ここでは形が同じかを見る。
    /// 引数の数・省略可・可変長・制約・型変数の既定の型が違えば重ならない。
    pub fn is_unifiable_with(&self, other: &Self) -> bool {
        self == other
    }
}

/// 空白の連なりを 1 つに畳んだ綴り。
///
/// TS のサーバはオブジェクト型リテラルを複数行に展開して返す
/// （`<T extends {\n    id: string;\n}, U>`）。改行と字下げが残ったままだと、
/// 同じ型が書かれ方の違いで別物になる。
fn flattened(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// 引数リストの括弧組を見つけ、その手前・中身・後ろに分ける。
///
/// 引数リストとみなすのは、**閉じ括弧の後ろが `:` か `=>` になる最初の括弧組**。
///
/// **Why not（`(method)` などの接頭辞を列挙して剥がす）**: hover の接頭辞は
/// `(method)` / `(property)` / `(local function)` / `function` / `const` と
/// サーバごとに増える。列挙から漏れた 1 つが、黙って比較を壊す。
fn parameter_list_of(text: &str) -> Option<(&str, &str, &str)> {
    let scanned = scanned(text);

    for (position, &(index, character, depth)) in scanned.iter().enumerate() {
        if character != '(' || depth != 0 {
            continue;
        }

        let Some(close) = closing_index_of(&scanned[position..], depth) else {
            continue;
        };
        let after_parameters = text.get(close + 1..)?;
        let follows = after_parameters.trim_start();

        if follows.starts_with(':') || follows.starts_with("=>") {
            return Some((
                text.get(..index)?,
                text.get(index + 1..close)?,
                after_parameters,
            ));
        }
    }

    None
}

/// 先頭の開き括弧に対応する閉じ括弧の位置。
///
/// `scanned` は開き括弧から始まる並び、`depth` はその括弧の外側の深さ。
/// 同じ深さに戻った最初の閉じ括弧が対応する相手になる。
fn closing_index_of(scanned: &[(usize, char, usize)], depth: usize) -> Option<usize> {
    scanned
        .iter()
        .skip(1)
        .find(|(_, character, at)| *character == ')' && *at == depth)
        .map(|(index, _, _)| *index)
}

/// 引数リストの後ろに書かれた戻り値の型。読み取れなければ `None`。
///
/// 宣言形は `): number`、値形は `) => number` と区切りが違う。
fn return_type_of(after_parameters: &str) -> Option<&str> {
    let follows = after_parameters.trim_start();
    let annotated = follows
        .strip_prefix("=>")
        .or_else(|| follows.strip_prefix(':'))?;
    let return_type = annotated.trim();

    if return_type.is_empty() {
        return None;
    }
    Some(return_type)
}

/// 引数リストから、名前を落とした型の並びを取り出す。
///
/// 引数が 1 つも無ければ空の並び。1 つでも読み取れない引数があれば `None`。
fn parameter_types_of(parameter_list: &str) -> Option<Vec<String>> {
    if parameter_list.trim().is_empty() {
        return Some(Vec::new());
    }

    top_level_parts_of(parameter_list, ',')
        .into_iter()
        .map(parameter_type_of)
        .collect()
}

/// 引数 1 つ分から名前を落として型だけにする。型注釈が無ければ `None`。
///
/// 省略可（`?`）と可変長（`...`）は型の一部として残す。**落とすと `a: string` と
/// `a?: string` が同じ形になる**が、前者は必ず渡す引数で後者は省ける。
fn parameter_type_of(parameter: &str) -> Option<String> {
    let parameter = parameter.trim();
    let (rest_marker, named) = match parameter.strip_prefix("...") {
        Some(named) => ("...", named),
        None => ("", parameter),
    };

    // 分割は深さ 0 の `:` で行う。分割代入の引数（`{ a: b }: Shape`）は
    // 名前の側にも `:` を持つ。
    let separator = top_level_index_of(named, ':')?;
    let name = named.get(..separator)?.trim_end();
    let optional_marker = if name.ends_with('?') { "?" } else { "" };
    let annotated = named.get(separator + 1..)?.trim();

    if annotated.is_empty() {
        return None;
    }
    let annotated = normalized_type(annotated)?;

    Some(format!("{rest_marker}{optional_marker}{annotated}"))
}

/// 型 1 つ分を、名前の入らない形へ直す。読み取れない関数型では `None`。
///
/// 関数型（`(a: string) => void`）はそれ自身が引数名を持つので、そこでも名前を落とす。
/// **落とさないと `cb: (a: string) => void` と `cb: (b: string) => void` が別物になる。**
/// コールバックを取る関数はどこにでもあるので、名前の違いがそのまま偽陰性になる。
///
/// 総称型の中に置かれた関数型（`Array<(a: string) => void>`）までは踏み込まない。
/// そこまで見るには型そのものの構文解析が要る。
fn normalized_type(text: &str) -> Option<String> {
    let Some((prefix, parameter_list, after_parameters)) = parameter_list_of(text) else {
        return Some(text.to_owned());
    };

    let return_type = normalized_type(return_type_of(after_parameters)?)?;
    let parameters = parameter_types_of(parameter_list)?;

    Some(format!(
        "{prefix}({}) => {return_type}",
        parameters.join(", ")
    ))
}

/// 引数リストの手前に書かれた型変数の宣言。宣言が無ければ空。
fn declared_type_parameters_of(prefix: &str) -> Vec<DeclaredTypeParameter> {
    let prefix = prefix.trim_end();
    if !prefix.ends_with('>') {
        return Vec::new();
    }

    // 深さ 0 にある最後の `<` が、末尾の `>` の相手。`Map<string, X>` のような
    // 入れ子は深さで外れ、`Foo<A>.bar<T>` のように 2 組並んでも後ろが選ばれる。
    let Some(open) = scanned(prefix)
        .into_iter()
        .rfind(|(_, character, depth)| *character == '<' && *depth == 0)
    else {
        return Vec::new();
    };

    let Some(declarations) = prefix.get(open.0 + 1..prefix.len() - '>'.len_utf8()) else {
        return Vec::new();
    };

    top_level_parts_of(declarations, ',')
        .into_iter()
        .filter_map(declared_type_parameter_of)
        .collect()
}

/// 型変数 1 つ分の宣言から、名前・制約・既定の型を取り出す。名前が無ければ `None`。
///
/// 既定の型を先に切り離す。`T extends X = D` は制約と既定の両方を持ち、
/// 切り離さないと制約が `X = D` になる。
fn declared_type_parameter_of(declaration: &str) -> Option<DeclaredTypeParameter> {
    let declaration = declaration.trim();
    let (bounded, default) = match declaration.split_once(DEFAULT_MARKER) {
        Some((bounded, default)) => (bounded, Some(default.trim().to_owned())),
        None => (declaration, None),
    };
    let name = bounded.split_whitespace().next()?;

    Some(DeclaredTypeParameter {
        name: name.to_owned(),
        constraint: bounded
            .split_once(CONSTRAINT_KEYWORD)
            .map(|(_, constraint)| constraint.trim().to_owned()),
        default,
    })
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

/// 型変数の名前を、引数と戻り値に現れる順に並べる。
///
/// `occurrences` は引数の型と戻り値の型を、綴りに書かれた順に並べたもの。
/// 一度も現れない型変数は宣言の順で後ろに置く。
///
/// **Why（宣言順ではなく出現順）**: `f<T, U>(a: U, b: T)` と `g<A, B>(a: A, b: B)` は
/// どちらも「異なる 2 つの型を取る」形で単一化できる。宣言順で付け替えると、
/// 前者が `(%1, %0)`、後者が `(%0, %1)` になって別物になる。
fn ordered_names_of<'a>(
    declared: &[DeclaredTypeParameter],
    occurrences: impl Iterator<Item = &'a str>,
) -> Vec<String> {
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

    ordered
}

/// 型変数の名前から、付け替え後の綴りへの対応。
fn placeholders_of(ordered: &[String]) -> HashMap<String, String> {
    ordered
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), format!("{PLACEHOLDER_PREFIX}{index}")))
        .collect()
}

/// 型変数を、付け替え後の並びで返す。
///
/// 制約と既定の型も付け替えの対象にする。`<T, U extends T>` のように、
/// どちらも別の型変数を指すことがある。
fn type_parameters_of(
    declared: &[DeclaredTypeParameter],
    ordered: &[String],
    placeholders: &HashMap<String, String>,
) -> Vec<TypeParameter> {
    ordered
        .iter()
        .map(|name| {
            let declaration = declared
                .iter()
                .find(|declaration| declaration.name == *name);
            let renamed_part = |part: Option<&String>| part.map(|text| renamed(text, placeholders));

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
fn renamed(text: &str, placeholders: &HashMap<String, String>) -> String {
    let mut renamed = String::new();
    let mut identifier = String::new();

    for character in text.chars() {
        if is_identifier_character(character) {
            identifier.push(character);
            continue;
        }

        push_renamed(&mut renamed, &identifier, placeholders);
        identifier.clear();
        renamed.push(character);
    }
    push_renamed(&mut renamed, &identifier, placeholders);

    renamed
}

/// 識別子 1 つ分を、付け替え後の綴り（対応が無ければそのまま）で書き足す。
fn push_renamed(target: &mut String, identifier: &str, placeholders: &HashMap<String, String>) {
    match placeholders.get(identifier) {
        Some(placeholder) => target.push_str(placeholder),
        None => target.push_str(identifier),
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

/// 深さ 0 にある `separator` で切り分ける。括弧・引用符の中にある区切りは無視する。
fn top_level_parts_of(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;

    for (index, character, depth) in scanned(text) {
        if character != separator || depth != 0 {
            continue;
        }
        parts.push(&text[start..index]);
        start = index + character.len_utf8();
    }
    parts.push(&text[start..]);

    parts
}

/// 深さ 0 にある最初の `separator` の位置。無ければ `None`。
fn top_level_index_of(text: &str, separator: char) -> Option<usize> {
    scanned(text)
        .into_iter()
        .find(|(_, character, depth)| *character == separator && *depth == 0)
        .map(|(index, _, _)| index)
}

/// 文字を、その位置・文字そのもの・**その文字の外側の深さ**の組で返す。
///
/// 開き括弧と、それに対応する閉じ括弧は同じ深さになる。引用符の中は括弧を数えない
/// （文字列リテラル型 `"a, b"` の中の区切りを、型の区切りと取り違えないため）。
///
/// `=>` の `>` は閉じ括弧として数えない。型の中には `(cb: () => void, b: string)` の
/// ように矢印が現れ、これを数えると以降の深さがずれる。
fn scanned(text: &str) -> Vec<(usize, char, usize)> {
    let mut scanned = Vec::new();
    let mut depth: usize = 0;
    let mut quote: Option<char> = None;
    let mut previous = ' ';

    for (index, character) in text.char_indices() {
        if let Some(open) = quote {
            // 引用符の中は数えない。文字列リテラル型 `"a, b"` の中の区切りを、
            // 型の区切りと取り違えないため。
            if character == open {
                quote = None;
            }
            previous = character;
            continue;
        }

        let closes = matches!(character, ')' | ']' | '}') || (character == '>' && previous != '=');
        if closes {
            depth = depth.saturating_sub(1);
        }

        scanned.push((index, character, depth));

        if matches!(character, '(' | '[' | '{' | '<') {
            depth += 1;
        }
        if matches!(character, '"' | '\'' | '`') {
            quote = Some(character);
        }
        previous = character;
    }

    scanned
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストが渡す綴りは読み取れる前提で組み立てる。
    fn signature(text: &str) -> TypeSignature {
        TypeSignature::from_signature_text(text).expect("テストが渡す綴りは読み取れる")
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
            TypeSignature::from_signature_text("type UserId = string"),
            None
        );
    }

    #[test]
    fn test_a_signature_without_a_return_type_cannot_be_read() {
        assert_eq!(
            TypeSignature::from_signature_text("function decl(a: string)"),
            None
        );
    }

    #[test]
    fn test_a_parameter_without_a_type_annotation_cannot_be_read() {
        // 名前だけの引数からは型を取り出せない。空の型で埋めると、
        // 型注釈の無い引数どうしが「同じ型」として重なる
        assert_eq!(
            TypeSignature::from_signature_text("function decl(a, b: string): void"),
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
}
