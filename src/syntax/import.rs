//! import 文が指す依存先と、その集合。
//!
//! 指定子を**文字列のまま比べない**。`./pad` と `../utils/pad` は綴りが違うだけで
//! 同じファイルを指しうるので、そのまま比べると共有している依存を「別物」と数える。
//! 依存先が食い違っていると誤って言うのは、このツールが最も損をする外し方
//! （共有ユーティリティに「共通化するな」と言うことになる）。

use std::collections::HashSet;
use std::path::{Component, Path};

use tree_sitter::Node;

use crate::similarity::Similarity;
use crate::syntax::tree::SyntaxTree;

/// 解決済みの依存先。相対指定は importer の位置から畳んである。
///
/// 区切りは常に `/`。プラットフォームの区切りをそのまま持つと、
/// 同じ依存先が OS によって別の値になる。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath(String);

impl ModulePath {
    /// import 指定子を、それを書いているファイルの位置から解決する。
    ///
    /// `specifier` は依存の宣言が依存先として書いている文字列（`from` の後ろ・
    /// `require` の引数）、`importer` はそれを書いているファイル。
    /// 相対指定（`./` / `../`）だけを畳み、パッケージ名は書かれたまま返す
    /// （パッケージ名は importer の位置に依らない）。
    ///
    /// **ファイルシステムは見ない。** 指す先が実在するとは限らず
    /// （拡張子の省略・`tsconfig` のパスエイリアス）、`syntax` は I/O を持てない
    /// (rules/coding.md 禁止事項)。
    pub fn from_specifier(specifier: &str, importer: &Path) -> Self {
        if !is_relative(specifier) {
            return Self(specifier.to_string());
        }

        let directory = importer.parent().unwrap_or_else(|| Path::new(""));
        Self(folded_path(directory, specifier))
    }

    /// 解決済みの依存先そのもの。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// ファイル 1 つ分の、依存している先の集合。
///
/// 空では作れない。import が 1 つも無いことを空の集合として通すと、後段が
/// 「依存先が食い違っている」と「材料が無い」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// **欠けたままでも作れない。** 読み取れなかった宣言が 1 つでもあれば作らない
/// （同じ規約の適用。下の [`ImportSet::from_tree`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSet(HashSet<ModulePath>);

/// 依存先の集合を作れなかった理由。
///
/// **1 つにまとめない。** 利用者が次にすることが違う。宣言が無いファイルは
/// そういうファイルだが、読み取れなかったファイルは**書いてあるのに dryguard が
/// 読めていない**（`rules/architecture.md`「理由は落とさない」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportsUnavailable {
    /// 依存の宣言が 1 つも書かれていない。
    NoDeclarations,
    /// 依存の宣言はあるが、指定子を読み取れなかった。
    UnreadableDeclaration,
}

impl ImportSet {
    /// 構文木の import を集めて、解決済みの依存先の集合にする。
    ///
    /// `tree` は importer の構文木、`importer` はそのファイルの位置。
    ///
    /// # Errors
    ///
    /// 依存の宣言が 1 つも書かれていなければ [`ImportsUnavailable::NoDeclarations`]。
    ///
    /// **読み取れなかった宣言が 1 つでもあれば** [`ImportsUnavailable::UnreadableDeclaration`]。
    /// 欠けたまま集合を返すと、後段はその重なりを測れた値として読む。1 件落ちるだけで
    /// 重なりは過大にも過小にも動くので、**落ちたことが構造に出ないと区別できない**
    /// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    pub fn from_tree(tree: &SyntaxTree<'_>, importer: &Path) -> Result<Self, ImportsUnavailable> {
        let specifiers = specifiers_of(tree).ok_or(ImportsUnavailable::UnreadableDeclaration)?;
        let paths: HashSet<ModulePath> = specifiers
            .iter()
            .map(|specifier| ModulePath::from_specifier(specifier, importer))
            .collect();

        if paths.is_empty() {
            return Err(ImportsUnavailable::NoDeclarations);
        }
        Ok(Self(paths))
    }

    /// 2 つの依存先集合の Jaccard 係数（共通している依存先が、合わせたうちの何割か）。
    ///
    /// これが Phase 0 で唯一のドメインシグナル。Stage 2 を飛ばしているので、
    /// 依存先が同じかどうかはここでしか言えない。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        let shared = self.0.intersection(&other.0).count();
        let combined = self.0.union(&other.0).count();

        Similarity::from_shared_count(shared, combined)
    }
}

/// 依存先を宣言する文の種別。どちらも `source` フィールドに指定子の文字列を持つ。
///
/// `export_statement` を入れるのは `export { pad } from "./pad"` と
/// `export * from "./pad"` のため。`from` を持たない `export` は
/// `source` フィールドを持たないので、同じ規則のまま外れる。
const DEPENDENCY_STATEMENT_KINDS: [&str; 2] = ["import_statement", "export_statement"];

/// `import dep = require("./dep")` の右辺。
///
/// この形は `call_expression` にならないので、呼び出しの規則では拾えない。
/// 名前空間の別名（`import dep = A.B`）は別の種別（`import_alias`）になるため、
/// 種別で見るだけで外れる。
const IMPORT_REQUIRE_CLAUSE_KIND: &str = "import_require_clause";

const CALL_EXPRESSION_KIND: &str = "call_expression";
const MEMBER_EXPRESSION_KIND: &str = "member_expression";
const UNARY_EXPRESSION_KIND: &str = "unary_expression";
/// 中の式の値をそのまま返す包み。**綴りを見る前に剥がす。**
///
/// 括弧と、TypeScript の型だけの注記（`require as NodeRequire` / `require!` /
/// `require satisfies NodeRequire`）。どれも実行時の値は中の式そのもの。
/// **どれも最初の子が中の式**なので、1 つの規則で剥がせる。
///
/// **ここから漏れた包みは [`RequireSpelling::Rebound`] へ落ちる。** 落ちた先は
/// 「読み取れない」なので、**値を作らずに測れないと言う**だけで済む
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
///
/// **Why not（`<NodeRequire>require` / `(0, require)` も入れる）**: 前者は最初の子が型、
/// 後者は値が最後の子で、剥がし方が違う。上の落ち方で足りるので規則を増やさない。
const TRANSPARENT_WRAPPER_KINDS: [&str; 4] = [
    "parenthesized_expression",
    "as_expression",
    "satisfies_expression",
    "non_null_expression",
];
const IDENTIFIER_KIND: &str = "identifier";
/// メンバアクセスや、オブジェクト・クラスの要素に書かれた名前。**束縛を作らない。**
const MEMBER_NAME_KIND: &str = "property_identifier";
const DYNAMIC_IMPORT_KIND: &str = "import";
const STRING_FRAGMENT_KIND: &str = "string_fragment";
/// 引数の並びには現れるが、引数ではないもの。
const COMMENT_KIND: &str = "comment";

/// 指定子を書ける文字列リテラルの種別。
///
/// テンプレートリテラルを入れるのは、**置換を持つものも依存の宣言だから**。
/// 種別で外すと「宣言していない」と区別が付かなくなる（読めないことは
/// [`unquoted_text_of`] が言う）。
const STRING_LITERAL_KINDS: [&str; 2] = ["string", "template_string"];

/// CommonJS が依存を読み込む関数の名前。
const REQUIRE_FUNCTION_NAME: &str = "require";

/// そのファイルで `require` の綴りが何を指しているか。
///
/// `require` は予約語ではないので、綴りだけでは CommonJS の読み込みだと言えない
/// （動的 `import` はキーワードなのでこの曖昧さが無い）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequireSpelling {
    /// CommonJS の読み込み。呼び出しを依存の宣言として採ってよい。
    ModuleLoader,
    /// 読み込み以外の位置にも現れる綴り。同じファイルで束縛され直しているかもしれない。
    Rebound,
}

/// ソースに書かれた import 指定子。書かれた順に返す。
///
/// 文字列を無条件に拾わず、**依存を宣言する文が指定子として持っているものだけ**を採る。
/// `const path = "./pad";` を拾うと、依存していないファイルが依存しているように見える。
///
/// **読み取れない宣言が 1 つでもあれば `None` を返す。** 残りだけを返すと、
/// 呼び出し側は欠けた集合を揃った集合として扱う。
fn specifiers_of<'source>(tree: &SyntaxTree<'source>) -> Option<Vec<&'source str>> {
    // 綴りが何を指すかはファイル全体を見ないと決まらないので、木を歩く前に 1 回だけ決める。
    // **束縛され直しているかもしれないファイルは、呼び出しを見つけたかに関わらず
    // 読み取れないとして返す。** 見つけられなかった読み込みがあるかもしれず、
    // 「見つけた呼び出しの数」ではそれを言えない
    if require_spelling_of(tree) == RequireSpelling::Rebound {
        return None;
    }
    let mut specifiers = Vec::new();

    for node in tree.named_descendants() {
        match specifier_of(tree, node) {
            SpecifierReading::NotADeclaration => {}
            SpecifierReading::Specifier(specifier) => specifiers.push(specifier),
            SpecifierReading::Unreadable => return None,
        }
    }
    Some(specifiers)
}

/// そのファイルで `require` の綴りが CommonJS の読み込みを指しているか。
///
/// 読み込み以外の位置にその綴りが 1 つでも現れたら [`RequireSpelling::Rebound`]。
/// **束縛の形を数え上げない。** 一覧を持つと、そこから漏れた束縛が
/// 「読み込み」の側へ落ちて**依存していない先を依存として数える**
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
/// 数え上げないぶん、読み込みと関係ない綴り（`require` という中身の文字列など）でも
/// 集めない側へ倒れるが、そちらは落とすだけで済む。
fn require_spelling_of(tree: &SyntaxTree<'_>) -> RequireSpelling {
    let rebound = tree
        .named_descendants()
        .into_iter()
        .any(|node| is_require_spelled_outside_a_call(tree, node));

    if rebound {
        return RequireSpelling::Rebound;
    }
    RequireSpelling::ModuleLoader
}

/// そのノードが、呼ばれる側ではない `require` の綴りか。
///
/// **メンバアクセスや要素の名前（`registry.require`）は数えない。** 束縛を作らないので、
/// 数えると `require` という名前の要素を触るだけでそのファイルの読み込みが全部落ちる。
fn is_require_spelled_outside_a_call(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if tree.text_of(node) != Some(REQUIRE_FUNCTION_NAME) {
        return false;
    }
    !is_use_that_cannot_bind(node)
}

/// そのノードが、新しい名前を導入しえない位置に置かれた名前か。
///
/// 呼び出しの呼ばれる側（`require("./dep")`）・メンバアクセスの受け側
/// （`require.resolve(…)`）・要素の名前（`registry.require`）・単項演算の対象
/// （`typeof require`）。**どれも束縛を作らない。**
///
/// **ここから漏れた位置は「束縛かもしれない」側へ落ちる。** 落ちた先は
/// [`SpecifierReading::Unreadable`] なので、**値を作らずに測れないと言う**だけで済む。
fn is_use_that_cannot_bind(node: Node<'_>) -> bool {
    if node.kind() == MEMBER_NAME_KIND {
        return true;
    }
    // 包みは値を変えないので、外まで戻ってから位置を見る（`(require)("./dep")`）
    let positioned = outside_wrappers(node);
    let Some(parent) = positioned.parent() else {
        return false;
    };

    let is_called = parent.kind() == CALL_EXPRESSION_KIND
        && parent.child_by_field_name("function") == Some(positioned);
    let is_member_object = parent.kind() == MEMBER_EXPRESSION_KIND
        && parent.child_by_field_name("object") == Some(positioned);
    let is_unary_operand = parent.kind() == UNARY_EXPRESSION_KIND
        && parent.child_by_field_name("argument") == Some(positioned);

    is_called || is_member_object || is_unary_operand
}

/// 包んでいる包みの外まで遡ったノード。
fn outside_wrappers(node: Node<'_>) -> Node<'_> {
    let mut current = node;

    while let Some(parent) = current.parent() {
        if !TRANSPARENT_WRAPPER_KINDS.contains(&parent.kind()) {
            break;
        }
        current = parent;
    }
    current
}

/// 包みを剥がした中の式。
fn inside_wrappers(node: Node<'_>) -> Node<'_> {
    let mut current = node;

    while TRANSPARENT_WRAPPER_KINDS.contains(&current.kind()) {
        let Some(inner) = first_expression_child_of(current) else {
            break;
        };
        current = inner;
    }
    current
}

/// そのノードの直下にある最初の式。コメントは式ではないので飛ばす。
fn first_expression_child_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| child.kind() != COMMENT_KIND)
}

/// ノード 1 つを依存の宣言として読んだ結果。
enum SpecifierReading<'source> {
    /// 依存を宣言していないノード。
    NotADeclaration,
    /// 読み取れた指定子。
    Specifier(&'source str),
    /// 依存を宣言しているが、指定子を読み取れなかった。
    ///
    /// **落とさずにここへ出す。** 黙って落とすと、同じファイルの他の宣言だけで
    /// 重なりが測れてしまう (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    Unreadable,
}

/// そのノードが宣言している依存先の指定子。
///
/// **`require` の綴りが束縛され直しているファイルはここへ来ない**
/// （[`specifiers_of`] が先に読み取れないとして返す）。
fn specifier_of<'source>(tree: &SyntaxTree<'source>, node: Node<'_>) -> SpecifierReading<'source> {
    if DEPENDENCY_STATEMENT_KINDS.contains(&node.kind()) {
        let Some(source) = node.child_by_field_name("source") else {
            return SpecifierReading::NotADeclaration;
        };
        return reading_of(tree, source);
    }
    if node.kind() == IMPORT_REQUIRE_CLAUSE_KIND {
        return reading_of_first_string_child(tree, node);
    }
    if calls_dynamic_import(node) {
        return reading_of_argument(tree, node);
    }
    if calls_require(tree, node) {
        return reading_of_argument(tree, node);
    }
    SpecifierReading::NotADeclaration
}

/// その呼び出しが動的 import か（`import("./pad")`）。
fn calls_dynamic_import(node: Node<'_>) -> bool {
    if node.kind() != CALL_EXPRESSION_KIND {
        return false;
    }

    node.child_by_field_name("function")
        .is_some_and(|called| called.kind() == DYNAMIC_IMPORT_KIND)
}

/// その呼び出しが `require` の綴りを呼んでいるか。
///
/// `registry.require("./pad")` は呼ばれる側が `member_expression` になるので外れる。
/// 括弧は剥がす（`(require)("./pad")` も同じ呼び出し）。
fn calls_require(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if node.kind() != CALL_EXPRESSION_KIND {
        return false;
    }

    node.child_by_field_name("function").is_some_and(|called| {
        let called = inside_wrappers(called);
        called.kind() == IDENTIFIER_KIND && tree.text_of(called) == Some(REQUIRE_FUNCTION_NAME)
    })
}

/// その呼び出しの**最初の引数**から読み取った指定子。
///
/// **2 つ目以降は見ない。** `require(name, "./fallback")` の `"./fallback"` は
/// 読み込む先ではないので、拾うと**どちらのファイルも読み込んでいない依存**が
/// 集合に入る。最初の引数が文字列リテラルでなければ読み取れないこととして返す
/// （`require(name)` も依存の宣言ではある）。
fn reading_of_argument<'source>(
    tree: &SyntaxTree<'source>,
    call: Node<'_>,
) -> SpecifierReading<'source> {
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return SpecifierReading::Unreadable;
    };
    let Some(first) = first_expression_child_of(arguments) else {
        return SpecifierReading::Unreadable;
    };
    if !STRING_LITERAL_KINDS.contains(&first.kind()) {
        return SpecifierReading::Unreadable;
    }

    reading_of(tree, first)
}

/// そのノードの直下にある最初の文字列リテラルから読み取った指定子。
fn reading_of_first_string_child<'source>(
    tree: &SyntaxTree<'source>,
    node: Node<'_>,
) -> SpecifierReading<'source> {
    let mut cursor = node.walk();
    let literal = node
        .named_children(&mut cursor)
        .find(|child| STRING_LITERAL_KINDS.contains(&child.kind()));

    let Some(literal) = literal else {
        return SpecifierReading::Unreadable;
    };
    reading_of(tree, literal)
}

/// その文字列リテラルから読み取った指定子。
fn reading_of<'source>(tree: &SyntaxTree<'source>, literal: Node<'_>) -> SpecifierReading<'source> {
    let Some(text) = unquoted_text_of(tree, literal) else {
        return SpecifierReading::Unreadable;
    };

    SpecifierReading::Specifier(text)
}

/// 引用符の中身。**綴りをそのまま取り出せないときは `None`。**
///
/// 取り出せるのは、中身が**断片 1 つだけ**でできているときに限る。
/// 断片が分かれるのは次の 3 つで、どれも最初の断片を採ると別の依存先になる。
///
/// - エスケープを含む（`"./pad"` の断片は `./pa` で、続きが別のノードになる）
/// - 置換を持つテンプレートリテラル（`` import(`./${name}`) ``。書いた時点で先が決まらない）
/// - 中身が空（`import("")`。指定子として使えない）
///
/// **置換を持たないテンプレートリテラル（`` require(`./pad`) ``）は断片 1 つなので取り出せる。**
/// 引用符の違いだけで落とすと、同じ依存先を書き方の違いで別物と数える。
fn unquoted_text_of<'source>(
    tree: &SyntaxTree<'source>,
    literal: Node<'_>,
) -> Option<&'source str> {
    let mut cursor = literal.walk();
    let parts: Vec<Node<'_>> = literal.named_children(&mut cursor).collect();

    let [only] = parts.as_slice() else {
        return None;
    };
    if only.kind() != STRING_FRAGMENT_KIND {
        return None;
    }
    tree.text_of(*only)
}

/// その指定子が importer の位置から解決するものか。
fn is_relative(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../") || specifier == ".."
}

/// `directory` から見た `specifier` を、`.` と `..` を畳んだ 1 本のパスにする。
fn folded_path(directory: &Path, specifier: &str) -> String {
    let mut segments: Vec<String> = directory
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => climb(&mut segments),
            name => segments.push(name.to_string()),
        }
    }

    segments.join("/")
}

/// 1 つ上のディレクトリへ移る。
///
/// 遡る先が無いときは `..` を残す。畳めなかったことを黙って捨てると、
/// ツリーの外を指す別々の依存先が同じ値になる。
fn climb(segments: &mut Vec<String>) {
    let can_pop = segments.last().is_some_and(|segment| segment != "..");
    if can_pop {
        segments.pop();
        return;
    }
    segments.push("..".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::tree::Grammar;
    use std::path::Path;

    fn resolved(specifier: &str, importer: &str) -> String {
        ModulePath::from_specifier(specifier, Path::new(importer))
            .as_str()
            .to_string()
    }

    #[test]
    fn test_module_path_of_a_sibling_specifier_is_relative_to_the_importer() {
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_path_of_a_parent_specifier_climbs_out_of_the_importer_directory() {
        assert_eq!(
            resolved("../utils/pad", "src/report/dateHelper.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_paths_of_two_importers_pointing_at_the_same_file_are_equal() {
        // このツールの最悪の外し方（共有ユーティリティに「共通化するな」と言う）は、
        // 指定子を文字列のまま比べたときに起きる
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            resolved("../utils/pad", "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_module_path_that_climbs_past_the_top_keeps_the_remaining_parent_steps() {
        // 遡り切れなかった `..` を捨てると、ツリーの外を指す別々の依存先が同じ値になる
        assert_eq!(resolved("../../shared", "src/pad.ts"), "../shared");
    }

    #[test]
    fn test_module_path_of_a_package_specifier_is_kept_as_written() {
        // パッケージ名は importer の位置に依らない
        assert_eq!(resolved("react", "src/utils/formatDate.ts"), "react");
    }

    fn tree_of(source: &str) -> SyntaxTree<'_> {
        SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる")
    }

    fn import_set(source: &str, importer: &str) -> ImportSet {
        ImportSet::from_tree(&tree_of(source), Path::new(importer))
            .expect("テストが渡すソースには import がある")
    }

    fn overlap(source_a: &str, importer_a: &str, source_b: &str, importer_b: &str) -> f64 {
        import_set(source_a, importer_a)
            .jaccard(&import_set(source_b, importer_b))
            .value()
    }

    const IMPORTS_PAD_FROM_SIBLING: &str = r#"import { pad } from "./pad";

export function formatDate(value: Date): string {
  return pad(value.getMonth() + 1);
}
"#;

    const IMPORTS_PAD_FROM_PARENT: &str = r#"import { pad } from "../utils/pad";

export function dateHelper(value: Date): string {
  return pad(value.getDate());
}
"#;

    #[test]
    fn test_import_overlap_of_two_files_reaching_the_same_module_is_total() {
        // 綴りが違うだけで同じ依存先。文字列のまま比べると 0.0 になる
        assert_eq!(
            overlap(
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/formatDate.ts",
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_depending_on_different_modules_is_zero() {
        let billing = r#"import { Invoice } from "./invoice";"#;
        let inventory = r#"import { Stock } from "./stock";"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/billing/b.ts"),
            0.0
        );
    }

    #[test]
    fn test_import_set_ignores_strings_that_are_not_import_specifiers() {
        // 拾ってよい import を 1 件、拾ってはいけない文字列を 1 件、同じソースに置く。
        // 文字列を無条件に拾う実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let real_import_and_a_decoy = r#"import { pad } from "./pad";
const path = "./stock";
"#;

        assert_eq!(
            overlap(
                real_import_and_a_decoy,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_multiline_import_reaches_the_same_module() {
        let split_over_lines = r#"import {
  pad,
} from "./pad";
"#;

        assert_eq!(
            overlap(
                split_over_lines,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_re_export_reaches_the_same_module() {
        let re_export = r#"export { pad } from "./pad";"#;

        assert_eq!(
            overlap(
                re_export,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_side_effect_import_reaches_the_same_module() {
        let side_effect_only = r#"import "./pad";"#;

        assert_eq!(
            overlap(
                side_effect_only,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_dynamic_import_reaches_the_same_module() {
        // import(".../pad") も依存の宣言。import 文だけを見ると、遅延読み込みしている
        // ファイルの依存先が丸ごと消える
        let dynamic = r#"export async function load(): Promise<unknown> {
  return import("./pad");
}
"#;

        assert_eq!(
            overlap(
                dynamic,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_call_reaches_the_same_module() {
        // require で書いた依存も同じ依存先へ届く。集めないと、この 2 つは
        // 「依存の宣言が無い」と「1 件ある」になり重なりを測れない
        let requires_pad = r#"const { pad } = require("./pad");

export function formatDate(value: Date): string {
  return pad(value.getMonth() + 1);
}
"#;

        assert_eq!(
            overlap(
                requires_pad,
                "src/utils/formatDate.ts",
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_an_import_equals_require_reaches_the_same_module() {
        // TypeScript の import-equals 形式。`call_expression` にならないので、
        // 呼び出しの規則だけでは拾えない
        let import_equals = r#"import pad = require("./pad");"#;

        assert_eq!(
            overlap(
                import_equals,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_written_as_a_statement_reaches_the_same_module() {
        // ESM 側は副作用だけの `import "./pad";` を拾う。CJS でそれに当たるのが
        // この形なので、右辺に限ると同じ形が片方でだけ拾えなくなる
        let side_effect_only = r#"require("./pad");"#;

        assert_eq!(
            overlap(
                side_effect_only,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_member_access_reaches_the_same_module() {
        let behind_member_access = r#"const pad = require("./pad").pad;"#;

        assert_eq!(
            overlap(
                behind_member_access,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_ignores_a_require_called_on_an_object() {
        // 拾ってよい import を 1 件、拾ってはいけない呼び出しを 1 件、同じソースに置く。
        // 呼ばれる側の綴りの末尾だけを見る実装だと集合に "./stock" が入り、
        // 重なりが 0.5 に落ちる
        let real_import_and_a_method_named_require = r#"import { pad } from "./pad";
const stock = registry.require("./stock");
"#;

        assert_eq!(
            overlap(
                real_import_and_a_method_named_require,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_a_template_literal_reaches_the_same_module() {
        // 置換が無いので綴りは書いた時点で決まっている。引用符の違いだけで落とすと、
        // 同じ依存先を書き方の違いで別物と数える
        let templated_require = r#"const pad = require(`./pad`);"#;

        assert_eq!(
            overlap(
                templated_require,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_substituted_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。読み取れない宣言を黙って落とす実装だと
        // "./pad" だけの集合が作れてしまい、欠けた集合で重なりを測ることになる
        let real_import_and_a_substituted_require = r#"import { pad } from "./pad";
const stock = require(`./${target}`);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_substituted_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration)
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_an_escaped_specifier_cannot_be_created() {
        // エスケープを含む綴りは断片が分かれる。最初の断片だけを採る実装だと
        // "./pa" を依存先として集合に入れてしまう（"./pad" とは別物になる）
        let escaped = format!(
            "import {{ pad }} from \"./pa{}u0064\";\nimport {{ trim }} from \"./trim\";\n",
            char::from(92u8)
        );

        assert_eq!(
            ImportSet::from_tree(&tree_of(&escaped), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::UnreadableDeclaration)
        );
    }

    #[test]
    fn test_import_set_of_a_file_that_rebinds_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`require` は予約語ではないので、
        // 綴りだけを見る実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let real_import_and_a_rebound_require = r#"import { pad } from "./pad";

function require(target: string): string {
  return target;
}

const stock = require("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_rebound_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration)
        );
    }

    #[test]
    fn test_import_set_of_a_require_called_through_parentheses_reaches_the_same_module() {
        // 括弧は式を変えない。呼ばれる側を綴りで見る前に剥がさないと、この呼び出しは
        // 宣言として分類されないまま中の `require` が束縛と見なされる
        let through_parentheses = r#"const pad = (require)("./pad");"#;

        assert_eq!(
            overlap(
                through_parentheses,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_type_annotation_reaches_the_same_module() {
        // 型だけの注記は実行時の値を変えない。剥がさないと呼び出しが宣言として
        // 分類されず、中の `require` だけが束縛と見なされる
        let annotated = r#"const pad = (require as NodeRequire)("./pad");"#;

        assert_eq!(
            overlap(
                annotated,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_non_null_assertion_reaches_the_same_module() {
        let asserted = r#"const pad = require!("./pad");"#;

        assert_eq!(
            overlap(
                asserted,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_wrapping_require_in_an_unpeeled_form_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。剥がせない包み（値が最後の子にある
        // 順次評価）は呼び出しとして分類されないが、**綴りは束縛の側へ落ちる**ので
        // ファイルごと測れないになる。ここが崩れると、包みの形を 1 つ見つけるたびに
        // 欠けた集合で測ることになる
        let real_import_and_an_indirect_require = r#"import { pad } from "./pad";
const stock = (0, require)("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_an_indirect_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration)
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_nonliteral_first_argument_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。2 つ目以降の引数まで探す実装だと
        // "./fallback" が集合に入る（**どちらのファイルも読み込んでいない依存**）
        let real_import_and_a_fallback_argument = r#"import { pad } from "./pad";
const stock = require(target, "./fallback");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_fallback_argument),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration)
        );
    }

    #[test]
    fn test_import_set_of_a_dynamic_import_with_attributes_reaches_the_same_module() {
        // 2 つ目の引数（読み込みの属性）は依存先ではない。最初の引数だけを見る規則が
        // この形を落とさないことを見る
        let with_attributes = r#"const pad = import("./pad", { with: { type: "json" } });"#;

        assert_eq!(
            overlap(
                with_attributes,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_guarded_by_typeof_reaches_the_same_module() {
        // `typeof require` も `require.resolve` も束縛を作らない。束縛を疑って
        // 落とすと、CommonJS でよくあるこの書き方のファイルが軒並み測れなくなる
        let guarded = r#"if (typeof require !== "undefined") {
  require("./pad");
}
const resolved = require.resolve("./pad");
"#;

        assert_eq!(
            overlap(
                guarded,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_whose_required_modules_differ_is_not_total() {
        // ESM と require が混在し、require 側の依存だけが食い違う。require を落とすと
        // 集合が両側とも "src/shared/util" だけになり、重なりが 1.00 と過大に出る
        let billing = r#"import { util } from "../shared/util";
const invoice = require("./invoice");
"#;
        let inventory = r#"import { util } from "../shared/util";
const stock = require("./stock");
"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/inventory/b.ts"),
            1.0 / 3.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_sharing_only_required_modules_is_not_zero() {
        // 同じ混在でも、共有しているのが require 側の依存だけのとき。require を落とすと
        // 共有が 1 件も残らず、重なりが 0.00 と過小に出る
        let billing = r#"import { format } from "./format";
const money = require("../shared/money");
const clock = require("../shared/clock");
"#;
        let inventory = r#"import { render } from "./render";
const money = require("../shared/money");
const clock = require("../shared/clock");
"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/inventory/b.ts"),
            0.5
        );
    }

    #[test]
    fn test_import_set_ignores_an_export_that_has_no_source() {
        // 対照に `from` 付きの export を 1 件置く。export の中の文字列を無条件に拾う
        // 実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let re_export_and_a_decoy = r#"export { pad } from "./pad";
export const fallback = "./stock";
"#;

        assert_eq!(
            overlap(
                re_export_and_a_decoy,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_ignores_an_import_written_inside_a_comment() {
        let commented_out = r#"// import { pad } from "./pad";
const value = 1;
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(commented_out), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::NoDeclarations)
        );
    }

    #[test]
    fn test_import_set_of_a_source_without_imports_cannot_be_created() {
        // 「import が無い」を空の集合で通すと、依存先が食い違っているのか
        // 材料が無いだけなのかを後段が区別できない
        let no_imports = r#"export function pad(value: number): string {
  return String(value);
}
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(no_imports), Path::new("src/utils/pad.ts")),
            Err(ImportsUnavailable::NoDeclarations)
        );
    }
}
