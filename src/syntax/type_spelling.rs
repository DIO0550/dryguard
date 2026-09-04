//! 型 1 つ分の綴りを構文木として読み、そこに書かれた型名を別の綴りへ差し替える。
//!
//! **差し替える先は綴りだが、どこが型名かは構文木で決める。** 綴りを識別子の単位で歩くと、
//! 型名でないもの（メンバー名・メソッド名・`typeof` の後ろの値の名前）まで同じ顔をするので、
//! **差し替えない位置を出た形ごとに数え上げる**ことになる。構文木なら、それらは
//! そもそも型名のノードにならない。
//!
//! **何に差し替えるかは知らない。** 型名を渡して綴りを受け取る関数を呼び出し側から渡す形に
//! してあるので、ここは `semantics` を知らないまま保たれる
//! （`rules/architecture.md`「依存方向のルール」）。

use std::collections::BTreeSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::syntax::tree::{Grammar, SyntaxTree};

/// 型 1 つ分の綴りを、型として読める文へ包むときの前置き。
///
/// **型エイリアスの右辺に置く。** そこは型 1 つ分だけが書ける場所なので、
/// 綴りが型として読めたかどうかが、包んだ文が読めたかどうかと一致する。
const SPELLING_PREFIX: &str = "type __ = ";

/// 包むときの後置き。
const SPELLING_SUFFIX: &str = ";";

/// 型名 1 つを表すノードの種別。
const TYPE_IDENTIFIER_KIND: &str = "type_identifier";

/// 修飾された型名を表すノードの種別（`money.Amount`）。
///
/// **1 つのノードが綴り全体を覆う。** 末尾だけを差し替えると `money.number` になるので、
/// 差し替えの単位はこちらにする。
const NESTED_TYPE_IDENTIFIER_KIND: &str = "nested_type_identifier";

/// 型変数の宣言を表すノードの種別（`<T extends X>` の `T extends X`）。
const TYPE_PARAMETER_KIND: &str = "type_parameter";

/// マップ型の束縛を表すノードの種別（`{ [K in Keys]: T }` の `K in Keys`）。
const MAPPED_TYPE_CLAUSE_KIND: &str = "mapped_type_clause";

/// 条件型が型を捕まえる印を表すノードの種別（`infer U`）。
const INFER_TYPE_KIND: &str = "infer_type";

/// モジュールの指定子を取る演算子を表すノードの種別（`import("./local")`）。
const IMPORT_KIND: &str = "import";

/// 差し込んだ綴りを括るときの括弧。
const GROUP_OPEN: char = '(';

/// 差し込んだ綴りを括るときの閉じ括弧。
const GROUP_CLOSE: char = ')';

/// 綴りが右へ開いたまま終わる型の種別。
///
/// どれも末尾が「任意の型」で終わり、**閉じる印を持たない**。後ろに何か続けば飲み込むので、
/// 演算子で並べる型の中では括弧が要る。
///
/// **この一覧は grammar が閉じている。** 出た形ごとに増えるものではなく、
/// TypeScript の型の文法が決める。
const OPEN_TAILED_TYPE_KINDS: [&str; 3] = ["function_type", "constructor_type", "conditional_type"];

/// 演算子で型を並べる型の種別。
///
/// 並べる相手の間に閉じる印が無いので、**開いたまま終わる型を直接置けない**。
/// [`OPEN_TAILED_TYPE_KINDS`] と同じく grammar が閉じている一覧。
const COMPOSING_TYPE_KINDS: [&str; 2] = ["union_type", "intersection_type"];

/// 綴りに書かれた型名を差し替えた綴り。型として読めない綴りでは `None`。
///
/// `spelling` は型 1 つ分の綴り（`hover` が返した綴りを割った先の 1 つ）。
/// `opened` はその型名が指す綴りを返す関数で、開けない型名では `None` を返す。
///
/// **束縛された名前は渡さない。** 型変数の宣言（`<T>`）・マップ型の束縛（`[K in Keys]`）・
/// `infer U` の `U` は、その綴りの中でだけ意味を持つ。外側に同じ綴りのエイリアスがあると、
/// 差し替えが `infer string` のような綴りを作る（[`bound_names_of`]）。
///
/// **括弧は要るときだけ足す。** 判断は差し込んでから確かめて決める（[`needs_grouping`]）。
///
/// **Why not（括弧が要る位置を列挙する）**: 配列・共用体・`keyof` と、要る位置は
/// 綴りの形の数だけある。**確かめる形なら、その一覧が要らない。**
pub(crate) fn substituted_spelling_of(
    spelling: &str,
    opened: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let wrapped = wrapped(spelling);
    let spans = type_name_spans_in_wrapped(&wrapped)?;

    // 後ろから差し替える。前から差し替えると、後ろの範囲が差し込んだ長さの分だけずれる。
    let mut substituted = wrapped;
    for span in spans.into_iter().rev() {
        let Some(name) = substituted.get(span.clone()) else {
            continue;
        };
        let Some(replacement) = opened(name) else {
            continue;
        };

        substituted = replaced(&substituted, span, &replacement);
    }

    unwrapped(&substituted)
}

/// 綴りに書かれた型名の範囲を、書かれた順に返す。型として読めない綴りでは `None`。
///
/// 範囲は `spelling` の中の位置。[`substituted_spelling_of`] が差し替える位置と同じものを
/// 返すので、**差し込む先を見る側と、開いた綴りを見る側が同じ判断で歩く**。
///
/// メンバー名・メソッド名・`typeof` の後ろの値の名前・文字列リテラル型の中身は、
/// そもそも型名のノードにならない。束縛された名前と、修飾された型名の末尾は外してある。
///
/// **Why（範囲を返して差し込みは任せる）**: 付け替え後の綴り（`%0`）は型として読めず、
/// [`substituted_spelling_of`] の括弧の要否を確かめる再パースが必ず失敗する。
/// 綴り 1 つ分に収まる置き換えでは括弧が要らないので、位置だけを渡す。
pub(crate) fn type_name_spans_of(spelling: &str) -> Option<Vec<Range<usize>>> {
    let wrapped = wrapped(spelling);
    let prefix = SPELLING_PREFIX.len();

    Some(
        type_name_spans_in_wrapped(&wrapped)?
            .into_iter()
            .map(|span| span.start - prefix..span.end - prefix)
            .collect(),
    )
}

/// その綴りが、モジュールの指定子を書いているか。型として読めない綴りでは `None`。
///
/// `import("./local")` の指定子は**書いた人の位置から解決される**ので、同じ綴りが
/// 別のモジュールの型を指しうる。指定子は型名のノードにならないため、
/// [`type_name_spans_of`] では掬えない。
pub(crate) fn holds_a_specifier(spelling: &str) -> Option<bool> {
    let wrapped = wrapped(spelling);
    let tree = SyntaxTree::from_source(&wrapped, Grammar::TypeScript).ok()?;
    if tree.has_error() {
        return None;
    }

    Some(
        tree.named_descendants()
            .into_iter()
            .any(|node| node.kind() == IMPORT_KIND),
    )
}

/// 型 1 つ分の綴りを、型として読める文へ包む。
fn wrapped(spelling: &str) -> String {
    format!("{SPELLING_PREFIX}{spelling}{SPELLING_SUFFIX}")
}

/// 包んだ文から、型 1 つ分の綴りを取り出す。前置きと後置きが揃わなければ `None`。
fn unwrapped(wrapped: &str) -> Option<String> {
    Some(
        wrapped
            .strip_prefix(SPELLING_PREFIX)?
            .strip_suffix(SPELLING_SUFFIX)?
            .to_owned(),
    )
}

/// 差し込みつつ、要るなら括ってから書き戻した綴り。
///
/// **括弧の要否は差し込んでから確かめる。** 括らずに置いた綴りがその範囲を 1 つの型として
/// 読めるなら、括ると書き下した綴りと別物になる（`Amount` を開いた `number` が
/// `(number)` になり、`number[]` と重ならなくなる）。
fn replaced(wrapped: &str, span: Range<usize>, replacement: &str) -> String {
    let plain = spliced(wrapped, span.clone(), replacement);
    let placed = span.start..span.start + replacement.len();

    if !needs_grouping(&plain, placed) {
        return plain;
    }

    spliced(
        wrapped,
        span,
        &format!("{GROUP_OPEN}{replacement}{GROUP_CLOSE}"),
    )
}

/// 差し込んだ綴りに括弧が要るか。
///
/// 要るのは 2 つ。**周りの印に負けて 1 つの型として読めなくなった**とき
/// （`string | undefined` を `Maybe[]` の位置へ置くと `undefined` だけが配列になる）と、
/// **右へ開いたまま終わる型を、演算子で並べる型の中へ直接置いた**とき。
///
/// **後者は読めてしまうので、読めるかどうかでは掬えない。** tree-sitter は
/// `null | () => string` を意図どおりに読むが、**TypeScript はその綴りを書かない**ので、
/// サーバが返した綴りと重ならなくなる。
fn needs_grouping(plain: &str, placed: Range<usize>) -> bool {
    let Ok(tree) = SyntaxTree::from_source(plain, Grammar::TypeScript) else {
        return true;
    };
    if tree.has_error() {
        return true;
    }

    let placed = tree
        .named_descendants()
        .into_iter()
        .find(|node| node.start_byte() == placed.start && node.end_byte() == placed.end);
    let Some(placed) = placed else {
        return true;
    };

    let opens_to_the_right = OPEN_TAILED_TYPE_KINDS.contains(&placed.kind());
    let composed = placed
        .parent()
        .is_some_and(|parent| COMPOSING_TYPE_KINDS.contains(&parent.kind()));

    opens_to_the_right && composed
}

/// その範囲を差し替えた綴り。
fn spliced(text: &str, span: Range<usize>, replacement: &str) -> String {
    let mut spliced = String::with_capacity(text.len());
    spliced.push_str(text.get(..span.start).unwrap_or_default());
    spliced.push_str(replacement);
    spliced.push_str(text.get(span.end..).unwrap_or_default());

    spliced
}

/// 包んだ文の中で、差し替えてよい型名が書かれている範囲。読めない綴りでは `None`。
///
/// **前置きの中は返さない。** 包むために置いた `__` も型名のノードになるので、
/// 綴りそのものの範囲だけに絞る。
fn type_name_spans_in_wrapped(wrapped: &str) -> Option<Vec<Range<usize>>> {
    let tree = SyntaxTree::from_source(wrapped, Grammar::TypeScript).ok()?;
    if tree.has_error() {
        return None;
    }
    let nodes = tree.named_descendants();

    let spelling = SPELLING_PREFIX.len()..wrapped.len().saturating_sub(SPELLING_SUFFIX.len());
    let bound = bound_names_of(&nodes, wrapped);
    let mut spans: Vec<Range<usize>> = Vec::new();

    for node in nodes {
        let range = node.byte_range();
        let inside_spelling = range.start >= spelling.start && range.end <= spelling.end;
        // 修飾された型名は 1 つのノードで差し替えるので、その中の型名へは降りない。
        let already_covered = spans
            .last()
            .is_some_and(|taken| range.start < taken.end && taken.start <= range.start);
        let names_the_binding = wrapped
            .get(range.clone())
            .is_some_and(|name| bound.contains(name));

        if !names_a_substitutable_type(node) || !inside_spelling || already_covered {
            continue;
        }
        if names_the_binding {
            continue;
        }
        spans.push(range);
    }

    Some(spans)
}

/// その綴りの中で束縛された名前。束縛が無ければ空。
///
/// **束縛した場所だけでなく、その綴りに現れる同じ名前をすべて外すために集める。**
/// `T extends Promise<infer U> ? U : never` の真の枝の `U` は、外側の `type U` ではなく
/// **捕まえたほうの `U`** を指す。宣言だけを外すと、使うほうが差し替わって
/// **束縛と食い違う綴り**になる。
fn bound_names_of<'text>(nodes: &[Node<'_>], wrapped: &'text str) -> BTreeSet<&'text str> {
    nodes
        .iter()
        .filter(|node| node.kind() == TYPE_IDENTIFIER_KIND && is_bound(**node))
        .filter_map(|node| wrapped.get(node.byte_range()))
        .collect()
}

/// そのノードが、差し替えてよい型名か。
fn names_a_substitutable_type(node: Node<'_>) -> bool {
    let names_a_type = matches!(
        node.kind(),
        TYPE_IDENTIFIER_KIND | NESTED_TYPE_IDENTIFIER_KIND
    );

    names_a_type && !is_qualified_leaf(node)
}

/// そのノードが、修飾された型名の末尾か（`money.Amount` の `Amount`）。
///
/// 修飾ごと差し替えるので、末尾だけを別に数えない。
fn is_qualified_leaf(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == NESTED_TYPE_IDENTIFIER_KIND)
}

/// そのノードが、この綴りの中で束縛された名前か。
///
/// 束縛は 3 通り。型変数の宣言（`<T>`）・マップ型の束縛（`[K in Keys]`）・`infer U`。
/// **どれも先頭の型名だけが束縛**で、続く制約（`<T extends Amount>` の `Amount`、
/// `[K in Keys]` の `Keys`、`infer U extends X` の `X`）は差し替えの相手に残る。
/// 制約も infer の束縛も同じ `infer_type` の直下に並ぶので、先頭かどうかで見分ける。
fn is_bound(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if !matches!(
        parent.kind(),
        TYPE_PARAMETER_KIND | MAPPED_TYPE_CLAUSE_KIND | INFER_TYPE_KIND
    ) {
        return false;
    }

    let mut cursor = parent.walk();
    parent.named_children(&mut cursor).next() == Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 型名 1 つだけを開く。
    fn opening(name: &str, spelling: &str) -> impl Fn(&str) -> Option<String> {
        let name = name.to_owned();
        let spelling = spelling.to_owned();

        move |asked: &str| (asked == name).then(|| spelling.clone())
    }

    /// 何も開かない。
    fn opening_nothing(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn test_a_type_name_written_on_its_own_is_substituted() {
        assert_eq!(
            substituted_spelling_of("Amount", opening("Amount", "number")),
            Some("number".to_owned())
        );
    }

    #[test]
    fn test_a_name_that_only_starts_with_the_opened_one_is_not_substituted() {
        // 対照は上のテスト。部分一致で差し替えると `AmountRate` の中まで書き換える
        assert_eq!(
            substituted_spelling_of("AmountRate", opening("Amount", "number")),
            Some("AmountRate".to_owned())
        );
    }

    #[test]
    fn test_a_qualified_type_name_is_substituted_as_one_spelling() {
        // 末尾だけを差し替えると `money.number` になる
        assert_eq!(
            substituted_spelling_of("money.Amount", opening("money.Amount", "number")),
            Some("number".to_owned())
        );
    }

    #[test]
    fn test_the_tail_of_a_qualified_type_name_is_not_substituted_on_its_own() {
        // 対照は上のテスト。末尾の `Amount` へ向けた解決は効かない
        assert_eq!(
            substituted_spelling_of("money.Amount", opening("Amount", "number")),
            Some("money.Amount".to_owned())
        );
    }

    #[test]
    fn test_a_type_name_interpolated_into_a_template_literal_type_is_substituted() {
        assert_eq!(
            substituted_spelling_of("`${Amount}`", opening("Amount", "number")),
            Some("`${number}`".to_owned())
        );
    }

    #[test]
    fn test_a_word_inside_a_string_literal_type_is_not_substituted() {
        // 対照は上のテスト。文字列リテラル型の中身は型の一部で、差し替えると別の型になる
        assert_eq!(
            substituted_spelling_of("\"Amount\"", opening("Amount", "number")),
            Some("\"Amount\"".to_owned())
        );
    }

    #[test]
    fn test_a_member_name_matching_an_opened_type_name_is_not_substituted() {
        assert_eq!(
            substituted_spelling_of("{ ID: ID }", opening("ID", "string")),
            Some("{ ID: string }".to_owned())
        );
    }

    #[test]
    fn test_a_method_name_matching_an_opened_type_name_is_not_substituted() {
        assert_eq!(
            substituted_spelling_of("{ ID(): ID }", opening("ID", "string")),
            Some("{ ID(): string }".to_owned())
        );
    }

    #[test]
    fn test_a_value_name_after_typeof_is_not_substituted() {
        assert_eq!(
            substituted_spelling_of("typeof ID", opening("ID", "string")),
            Some("typeof ID".to_owned())
        );
    }

    #[test]
    fn test_a_qualified_value_name_after_typeof_is_not_substituted() {
        // `.` を挟んだ先も値の名前。綴りを歩く形では直前が `.` になって見分けが付かない
        assert_eq!(
            substituted_spelling_of("typeof ns.ID", opening("ns.ID", "string")),
            Some("typeof ns.ID".to_owned())
        );
    }

    #[test]
    fn test_a_type_name_in_the_true_branch_of_a_conditional_type_is_substituted() {
        // 後ろに続く `:` は条件型の区切りで、メンバーの印ではない
        assert_eq!(
            substituted_spelling_of("T extends number ? ID : never", opening("ID", "string")),
            Some("T extends number ? string : never".to_owned())
        );
    }

    #[test]
    fn test_a_name_bound_by_infer_is_not_substituted() {
        // 真の枝の `U` は外側の `type U` ではなく、捕まえたほうの `U` を指す
        assert_eq!(
            substituted_spelling_of(
                "T extends Promise<infer U> ? U : never",
                opening("U", "string")
            ),
            Some("T extends Promise<infer U> ? U : never".to_owned())
        );
    }

    #[test]
    fn test_the_constraint_of_an_infer_binding_is_substituted() {
        // `infer U extends X` の `U` と `X` はどちらも infer_type の直下だが、束縛は先頭の
        // `U` だけ。制約 `X` は差し替えの相手に残る
        assert_eq!(
            substituted_spelling_of(
                "T extends Promise<infer U extends X> ? U : never",
                opening("X", "string")
            ),
            Some("T extends Promise<infer U extends string> ? U : never".to_owned())
        );
    }

    #[test]
    fn test_a_name_the_same_spelling_does_not_bind_is_substituted_beside_a_binding() {
        // 対照は上のテスト。束縛と同じ綴りでない名前は、同じ位置でも差し替わる
        assert_eq!(
            substituted_spelling_of(
                "T extends Promise<infer U> ? ID : never",
                opening("ID", "string")
            ),
            Some("T extends Promise<infer U> ? string : never".to_owned())
        );
    }

    #[test]
    fn test_a_name_bound_by_a_mapped_type_is_not_substituted() {
        // 値の側の `K` も、束縛したほうの `K` を指す
        assert_eq!(
            substituted_spelling_of("{ [K in Keys]: K }", opening("K", "string")),
            Some("{ [K in Keys]: K }".to_owned())
        );
    }

    #[test]
    fn test_a_constraint_of_a_mapped_type_binding_is_substituted() {
        assert_eq!(
            substituted_spelling_of("{ [K in Keys]: K }", opening("Keys", "\"a\" | \"b\"")),
            Some("{ [K in \"a\" | \"b\"]: K }".to_owned())
        );
    }

    #[test]
    fn test_an_identifier_holding_a_combining_mark_is_substituted() {
        // 分解された `É`（`E` + U+0301）。識別子を英数字で切ると結合文字のところで切れる
        assert_eq!(
            substituted_spelling_of("Ame\u{0301}unt", opening("Ame\u{0301}unt", "number")),
            Some("number".to_owned())
        );
    }

    #[test]
    fn test_an_opened_compound_type_is_grouped_inside_an_array_type() {
        // 括らないと `string | undefined[]` になり、`undefined` だけが配列になる
        assert_eq!(
            substituted_spelling_of("Maybe[]", opening("Maybe", "string | undefined")),
            Some("(string | undefined)[]".to_owned())
        );
    }

    #[test]
    fn test_an_opened_single_type_is_not_grouped_inside_an_array_type() {
        // 対照は上のテスト。常に括ると `(number)[]` になり、書き下した `number[]` と別物になる
        assert_eq!(
            substituted_spelling_of("Amount[]", opening("Amount", "number")),
            Some("number[]".to_owned())
        );
    }

    #[test]
    fn test_an_opened_compound_type_standing_on_its_own_is_not_grouped() {
        assert_eq!(
            substituted_spelling_of("Maybe", opening("Maybe", "string | undefined")),
            Some("string | undefined".to_owned())
        );
    }

    #[test]
    fn test_an_opened_compound_type_is_not_grouped_inside_a_generic_argument() {
        // 型引数の中は区切りに挟まれているので括弧は要らない
        assert_eq!(
            substituted_spelling_of("Map<string, Maybe>", opening("Maybe", "string | undefined")),
            Some("Map<string, string | undefined>".to_owned())
        );
    }

    #[test]
    fn test_an_opened_callable_type_is_grouped_inside_a_union() {
        // 括らないと `() => string | null` になり、共用体を返す関数として読める
        assert_eq!(
            substituted_spelling_of("Handler | null", opening("Handler", "() => string")),
            Some("(() => string) | null".to_owned())
        );
    }

    #[test]
    fn test_an_opened_callable_type_after_a_union_bar_is_grouped_too() {
        // 括弧が要るかは後ろだけでは決まらない
        assert_eq!(
            substituted_spelling_of("null | Handler", opening("Handler", "() => string")),
            Some("null | (() => string)".to_owned())
        );
    }

    #[test]
    fn test_an_opened_callable_type_is_not_grouped_inside_a_generic_argument() {
        // 対照は上の 2 つ。型引数の中は区切りに挟まれているので、開いたまま終わる型でも
        // 括弧は要らない。括ると書き下した綴りと別物になる
        assert_eq!(
            substituted_spelling_of("Map<string, Handler>", opening("Handler", "() => void")),
            Some("Map<string, () => void>".to_owned())
        );
    }

    #[test]
    fn test_an_opened_type_starting_with_a_type_operator_is_grouped_inside_an_array_type() {
        // 括らないと `keyof string[]` になり、`keyof (string[])` と読まれる
        assert_eq!(
            substituted_spelling_of("Keys[]", opening("Keys", "keyof string")),
            Some("(keyof string)[]".to_owned())
        );
    }

    #[test]
    fn test_an_already_grouped_opened_type_is_not_grouped_again() {
        assert_eq!(
            substituted_spelling_of("Grouped[]", opening("Grouped", "(string | number)")),
            Some("(string | number)[]".to_owned())
        );
    }

    #[test]
    fn test_a_spelling_that_does_not_read_as_a_type_cannot_be_substituted() {
        assert_eq!(substituted_spelling_of("=> )(", opening_nothing), None);
    }

    #[test]
    fn test_a_spelling_missing_a_closing_token_cannot_be_substituted() {
        // 欠けた字句は名前を持たないノードとして残るので、名前付きだけを歩いても
        // 見つからない。対照は下のテスト（同じ形で閉じているもの）
        assert_eq!(
            substituted_spelling_of("Map<string", opening("Amount", "number")),
            None
        );
    }

    #[test]
    fn test_a_spelling_missing_a_closing_parenthesis_cannot_be_substituted() {
        assert_eq!(
            substituted_spelling_of("(a: Amount => void", opening("Amount", "number")),
            None
        );
    }

    #[test]
    fn test_a_spelling_with_nothing_to_open_comes_back_unchanged() {
        assert_eq!(
            substituted_spelling_of("Map<string, Amount>", opening_nothing),
            Some("Map<string, Amount>".to_owned())
        );
    }

    #[test]
    fn test_every_occurrence_of_an_opened_type_name_is_substituted() {
        assert_eq!(
            substituted_spelling_of("Map<Amount, Amount>", opening("Amount", "number")),
            Some("Map<number, number>".to_owned())
        );
    }

    /// 綴りの中で型名が書かれている綴りを、書かれた順に。
    fn type_names_of(spelling: &str) -> Option<Vec<&str>> {
        Some(
            type_name_spans_of(spelling)?
                .into_iter()
                .filter_map(|span| spelling.get(span))
                .collect(),
        )
    }

    #[test]
    fn test_the_type_names_of_a_spelling_leave_out_what_is_not_a_type_name() {
        // メンバー名（`ID`）・組み込みの型（`string`）・文字列リテラル型の中身は型名ではない
        assert_eq!(
            type_names_of("{ ID: Local<string>; mode: \"Local\" }"),
            Some(vec!["Local"])
        );
    }

    #[test]
    fn test_the_type_names_of_a_spelling_leave_out_a_name_bound_inside_it() {
        // 対照は上のテスト。`U` は捕まえたほうの名前で、外の宣言を指していない
        assert_eq!(
            type_names_of("T extends Promise<infer U> ? U : never"),
            Some(vec!["T", "Promise"])
        );
    }

    #[test]
    fn test_the_type_names_of_a_spelling_that_does_not_read_as_a_type_are_not_returned() {
        // 空で返すと、型名が 1 つも無い綴りと区別が付かない
        assert_eq!(type_names_of("=> )("), None);
    }

    #[test]
    fn test_a_spelling_querying_an_imported_module_holds_a_specifier() {
        assert_eq!(holds_a_specifier("typeof import(\"./local\")"), Some(true));
    }

    #[test]
    fn test_a_spelling_naming_a_type_in_an_imported_module_holds_a_specifier() {
        // `typeof` を伴わない書き方でも指定子は指定子
        assert_eq!(holds_a_specifier("import(\"./local\").Thing"), Some(true));
    }

    #[test]
    fn test_a_string_literal_type_holds_no_specifier() {
        // 対照は上の 2 つ。引用符があることではなく、指定子であることを見ている
        assert_eq!(holds_a_specifier("\"on\" | \"off\""), Some(false));
    }

    #[test]
    fn test_a_spelling_that_does_not_read_as_a_type_cannot_be_asked_for_a_specifier() {
        assert_eq!(holds_a_specifier("=> )("), None);
    }
}
