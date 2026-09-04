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

/// 戻り値の注釈としてしか書けない綴りを包むときの前置き。
///
/// **型述語（`value is User` / `asserts value`）は型ではない。** 関数の戻り値の位置に
/// しか書けないので、エイリアスの右辺へ置くと読めない。引数を取らない関数型にすると
/// 読めるようになり、述語の主語（`value`）は値の名前のノードになる。
const RETURN_TYPE_PREFIX: &str = "type __ = () => ";

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

/// 型名 1 つを表さない、素の名前のノードの種別。
///
/// 型は `type_identifier` / `predefined_type` / `nested_type_identifier` になるので、
/// **型の綴りの中に出る `identifier` は型名ではない**（値の名前か、その綴りの中で
/// 束縛された名前）。
const IDENTIFIER_KIND: &str = "identifier";

/// 囲むクラスを指す型を表すノードの種別（`this`）。
const THIS_TYPE_KIND: &str = "this_type";

/// 型名でない名前が、その綴りの中でだけ意味を持つ場所のノードの種別。
///
/// 引数の名前（`(a: string) => void`）・タプルの要素の名前（`[first: string]`）・
/// インデックスシグネチャの引数の名前（`{ [index: number]: string }`）・
/// 型述語の主語（`value is User`）。**どれも外を指さない。**
///
/// **一覧にするのは外を指さないほう。** 外を指す形（値の名前・計算されたキー・
/// モジュールの指定子）は文法が増えるたびに増えるが、こちらは名前が束縛される場所なので
/// 閉じている。**一覧から漏れた形は site dependent の側へ倒れる**ので、
/// 倒れる向きは偽陰性になる。
const LOCAL_NAME_KINDS: [&str; 5] = [
    "required_parameter",
    "optional_parameter",
    "rest_pattern",
    "index_signature",
    "type_predicate",
];

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
    let wrapped = wrapped(spelling)?;
    let spans = type_name_spans_in_wrapped(&wrapped, BoundNames::Excluded)?;

    // 後ろから差し替える。前から差し替えると、後ろの範囲が差し込んだ長さの分だけずれる。
    let mut substituted = wrapped.text;
    for span in spans.into_iter().rev() {
        let Some(name) = substituted.get(span.clone()) else {
            continue;
        };
        let Some(replacement) = opened(name) else {
            continue;
        };

        substituted = replaced(&substituted, span, &replacement);
    }

    unwrapped(&substituted, wrapped.prefix)
}

/// 綴りの中で、差し替えてよい型名が書かれている範囲。型として読めない綴りでは `None`。
///
/// 範囲は `spelling` の中の位置。[`substituted_spelling_of`] が差し替える位置と同じものを
/// 返すので、**差し込む側と付け替える側が同じ判断で歩く**。
/// 束縛された名前と、修飾された型名の末尾は外してある。
///
/// **Why（範囲を返して差し込みは任せる）**: 付け替え後の綴り（`%0`）は型として読めず、
/// [`substituted_spelling_of`] の括弧の要否を確かめる再パースが必ず失敗する。
/// 綴り 1 つ分に収まる置き換えでは括弧が要らないので、位置だけを渡す。
pub(crate) fn substitutable_type_name_spans_of(spelling: &str) -> Option<Vec<Range<usize>>> {
    spans_in_spelling(spelling, BoundNames::Excluded)
}

/// 綴りに書かれた型名の範囲を、書かれた順に返す。型として読めない綴りでは `None`。
///
/// 範囲は `spelling` の中の位置。メンバー名・メソッド名・`typeof` の後ろの値の名前・
/// 文字列リテラル型の中身は、そもそも型名のノードにならないので入らない。
///
/// **束縛された名前も返す**（[`substitutable_type_name_spans_of`] との違いはここだけ）。
pub(crate) fn type_name_spans_of(spelling: &str) -> Option<Vec<Range<usize>>> {
    spans_in_spelling(spelling, BoundNames::Included)
}

/// 綴りを包んで歩き、返った範囲を綴りの中の位置へ戻す。
fn spans_in_spelling(spelling: &str, bound_names: BoundNames) -> Option<Vec<Range<usize>>> {
    let wrapped = wrapped(spelling)?;
    let prefix = wrapped.prefix;

    Some(
        type_name_spans_in_wrapped(&wrapped, bound_names)?
            .into_iter()
            .map(|span| span.start - prefix..span.end - prefix)
            .collect(),
    )
}

/// 束縛された名前を、返す範囲に含めるかどうか。
///
/// **外しすぎたときに倒れる向きが逆になる。** 差し替える側は外しすぎても差し込みを
/// 見送るだけ（偽陰性）だが、宣言を辿る名前を数える側は外しすぎると
/// **「辿る名前が残っていない」と答えてしまう**（偽陽性）。
///
/// 束縛は綴りごとに 1 つの集合で見ているので、同じ綴りの束縛が外側の名前を隠す
/// （`(<Local>() => Local) & Local` の末尾の `Local`）。数える側はそれを外さない。
#[derive(Clone, Copy)]
enum BoundNames {
    /// 差し替えの相手から外す。
    Excluded,
    /// 綴りに書かれた型名として数える。
    Included,
}

/// その綴りが、型の名前だけで書かれているか。型として読めない綴りでは `None`。
///
/// **型名にならないのに、指す先が書いた人の位置で決まる綴りがある。**
/// 値の名前（`typeof localValue`）・オブジェクト型の計算されたキー
/// （`{ [key]: string }`）・`this` 型（囲むクラスを指す）・モジュールの指定子
/// （`import("./local")`）で、どれも [`type_name_spans_of`] では掬えない。
///
/// **見分けるのは「外を指す形」ではなく「中に留まる名前」。** 外を指す形は文法が
/// 増えるたびに増えるが、型名でない名前が許される場所は閉じている
/// （[`LOCAL_NAME_KINDS`]）。
pub(crate) fn names_only_types(spelling: &str) -> Option<bool> {
    let wrapped = wrapped(spelling)?;
    let tree = SyntaxTree::from_source(&wrapped.text, Grammar::TypeScript).ok()?;

    let reaches_outside = tree
        .named_descendants()
        .into_iter()
        .any(names_outside_the_type_namespace);

    Some(!reaches_outside)
}

/// そのノードが、型の名前空間の外を指しているか。
fn names_outside_the_type_namespace(node: Node<'_>) -> bool {
    if matches!(node.kind(), THIS_TYPE_KIND | IMPORT_KIND) {
        return true;
    }
    if node.kind() != IDENTIFIER_KIND {
        return false;
    }

    let names_locally = node
        .parent()
        .is_some_and(|parent| LOCAL_NAME_KINDS.contains(&parent.kind()));

    !names_locally
}

/// 綴りを包んで構文木にできた文と、そのとき使った前置きの長さ。
struct Wrapped {
    text: String,
    prefix: usize,
}

/// 綴りを、構文木にできる文へ包む。どの包み方でも読めなければ `None`。
///
/// **包み方が 2 つあるのは、型が書ける場所が 1 つではないため。** 型述語
/// （`value is User`）は関数の戻り値の位置にしか書けず、エイリアスの右辺では読めない
/// （[`RETURN_TYPE_PREFIX`]）。
///
/// **Why not（はじめから戻り値の位置で包む）**: そこは型 1 つ分だけが書ける場所ではなく、
/// 型述語も通る。**綴りが型として読めたかどうかと、包んだ文が読めたかどうかが
/// 一致しなくなる**ので、型として読める包み方を先に試す。
fn wrapped(spelling: &str) -> Option<Wrapped> {
    for prefix in [SPELLING_PREFIX, RETURN_TYPE_PREFIX] {
        let text = format!("{prefix}{spelling}{SPELLING_SUFFIX}");
        let readable =
            SyntaxTree::from_source(&text, Grammar::TypeScript).is_ok_and(|tree| !tree.has_error());

        if readable {
            return Some(Wrapped {
                text,
                prefix: prefix.len(),
            });
        }
    }

    None
}

/// 包んだ文から、綴りを取り出す。前置きと後置きが揃わなければ `None`。
fn unwrapped(wrapped: &str, prefix: usize) -> Option<String> {
    Some(
        wrapped
            .get(prefix..)?
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

/// 包んだ文の中で、型名が書かれている範囲。読めない綴りでは `None`。
///
/// `bound_names` が [`BoundNames::Excluded`] なら、束縛された名前は返さない。
///
/// **前置きの中は返さない。** 包むために置いた `__` も型名のノードになるので、
/// 綴りそのものの範囲だけに絞る。
fn type_name_spans_in_wrapped(
    wrapped: &Wrapped,
    bound_names: BoundNames,
) -> Option<Vec<Range<usize>>> {
    let text = wrapped.text.as_str();
    let tree = SyntaxTree::from_source(text, Grammar::TypeScript).ok()?;
    if tree.has_error() {
        return None;
    }
    let nodes = tree.named_descendants();

    let spelling = wrapped.prefix..text.len().saturating_sub(SPELLING_SUFFIX.len());
    let bound = match bound_names {
        BoundNames::Excluded => bound_names_of(&nodes, text),
        BoundNames::Included => BTreeSet::new(),
    };
    let mut spans: Vec<Range<usize>> = Vec::new();

    for node in nodes {
        let range = node.byte_range();
        let inside_spelling = range.start >= spelling.start && range.end <= spelling.end;
        // 修飾された型名は 1 つのノードで差し替えるので、その中の型名へは降りない。
        let already_covered = spans
            .last()
            .is_some_and(|taken| range.start < taken.end && taken.start <= range.start);
        let names_the_binding = text
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

    /// 綴りの中で差し替えてよい型名の綴りを、書かれた順に。
    fn substitutable_type_names_of(spelling: &str) -> Option<Vec<&str>> {
        Some(
            substitutable_type_name_spans_of(spelling)?
                .into_iter()
                .filter_map(|span| spelling.get(span))
                .collect(),
        )
    }

    /// 綴りに書かれた型名の綴りを、書かれた順に。
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
    fn test_the_substitutable_type_names_of_a_spelling_leave_out_a_name_bound_inside_it() {
        // `U` は捕まえたほうの名前。差し替えると `infer string` のような綴りになる
        assert_eq!(
            substitutable_type_names_of("T extends Promise<infer U> ? U : never"),
            Some(vec!["T", "Promise"])
        );
    }

    #[test]
    fn test_the_type_names_of_a_spelling_hold_a_name_a_nested_binder_shadows() {
        // 対照は上のテスト。束縛は綴りごとに 1 つの集合なので、差し替える側は
        // 末尾の `Local` まで外す。**書かれた型名を数える側はそれを外さない**
        assert_eq!(
            substitutable_type_names_of("(<Local>() => Local) & Local"),
            Some(vec![])
        );
        assert_eq!(
            type_names_of("(<Local>() => Local) & Local"),
            Some(vec!["Local", "Local", "Local"])
        );
    }

    #[test]
    fn test_the_type_names_of_a_spelling_that_does_not_read_as_a_type_are_not_returned() {
        // 空で返すと、型名が 1 つも無い綴りと区別が付かない
        assert_eq!(type_names_of("=> )("), None);
        assert_eq!(substitutable_type_names_of("=> )("), None);
    }

    #[test]
    fn test_a_type_predicate_is_read_in_its_return_annotation_context() {
        // 型述語は関数の戻り値の位置にしか書けない。主語（`value`）は引数の名前なので
        // 型名にならず、絞る先の `User` だけが返る
        assert_eq!(type_names_of("value is User"), Some(vec!["User"]));
    }

    #[test]
    fn test_a_spelling_written_with_type_names_alone_names_only_types() {
        assert_eq!(names_only_types("Local<string> | \"on\""), Some(true));
    }

    #[test]
    fn test_a_spelling_naming_a_parameter_names_only_types() {
        // 引数の名前・タプルの要素の名前・インデックスシグネチャの引数の名前・
        // 型述語の主語は、その綴りの中でだけ意味を持つ
        assert_eq!(names_only_types("(a: string) => void"), Some(true));
        assert_eq!(names_only_types("[first: string]"), Some(true));
        assert_eq!(names_only_types("{ [index: number]: string }"), Some(true));
        assert_eq!(names_only_types("(a: unknown) => a is Local"), Some(true));
    }

    #[test]
    fn test_a_spelling_querying_a_value_does_not_name_only_types() {
        // 対照は上のテスト。同じ `identifier` でも、`typeof` の後ろは外の値を指す
        assert_eq!(names_only_types("typeof localValue"), Some(false));
    }

    #[test]
    fn test_a_spelling_with_a_computed_key_does_not_name_only_types() {
        // 計算されたキーは `unique symbol` の値を指す。別々のモジュールが同じ綴りの
        // `key` を宣言していると、同じ綴りが別の型を指す
        assert_eq!(names_only_types("{ [key]: string }"), Some(false));
    }

    #[test]
    fn test_a_spelling_naming_the_enclosing_class_does_not_name_only_types() {
        // `this` 型は囲むクラスを指すので、どこに書かれたかで意味が変わる
        assert_eq!(names_only_types("this"), Some(false));
    }

    #[test]
    fn test_a_spelling_querying_an_imported_module_does_not_name_only_types() {
        assert_eq!(names_only_types("typeof import(\"./local\")"), Some(false));
    }

    #[test]
    fn test_a_spelling_naming_a_type_in_an_imported_module_does_not_name_only_types() {
        // `typeof` を伴わない書き方でも指定子は指定子
        assert_eq!(names_only_types("import(\"./local\").Thing"), Some(false));
    }

    #[test]
    fn test_a_string_literal_type_names_only_types() {
        // 対照は上の 2 つ。引用符があることではなく、外を指すことを見ている
        assert_eq!(names_only_types("\"on\" | \"off\""), Some(true));
    }

    #[test]
    fn test_a_spelling_that_does_not_read_as_a_type_cannot_be_asked_what_it_names() {
        assert_eq!(names_only_types("=> )("), None);
    }
}
