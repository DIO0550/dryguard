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
    /// `specifier` は `from` の後ろに書かれた文字列、`importer` はそれを書いているファイル。
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSet(HashSet<ModulePath>);

impl ImportSet {
    /// 構文木の import を集めて、解決済みの依存先の集合にする。
    ///
    /// `tree` は importer の構文木、`importer` はそのファイルの位置。
    /// 依存の宣言を 1 つも読み取れなかったときは作れないので `None` を返す。
    /// **「宣言が無い」とは言えない。** 読み取れる形は文法が持つ書き方より狭い。
    pub fn from_tree(tree: &SyntaxTree<'_>, importer: &Path) -> Option<Self> {
        let paths: HashSet<ModulePath> = specifiers_of(tree)
            .iter()
            .map(|specifier| ModulePath::from_specifier(specifier, importer))
            .collect();

        if paths.is_empty() {
            return None;
        }
        Some(Self(paths))
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

/// CommonJS が依存を読み込む関数の名前。
const REQUIRE_FUNCTION_NAME: &str = "require";

/// ソースに書かれた import 指定子。書かれた順に返す。
///
/// 文字列を無条件に拾わず、**依存を宣言する文が指定子として持っているものだけ**を採る。
/// `const path = "./pad";` を拾うと、依存していないファイルが依存しているように見える。
fn specifiers_of<'source>(tree: &SyntaxTree<'source>) -> Vec<&'source str> {
    tree.named_descendants()
        .into_iter()
        .filter_map(|node| specifier_literal_of(tree, node))
        .filter_map(|literal| unquoted_text_of(tree, literal))
        .collect()
}

/// その文が依存先として指している文字列リテラルのノード。依存を宣言していなければ `None`。
fn specifier_literal_of<'tree>(tree: &SyntaxTree<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    if DEPENDENCY_STATEMENT_KINDS.contains(&node.kind()) {
        return node.child_by_field_name("source");
    }
    if node.kind() == IMPORT_REQUIRE_CLAUSE_KIND {
        return first_string_child_of(node);
    }
    if is_dependency_call(tree, node) {
        return first_string_argument_of(node);
    }
    None
}

/// その呼び出しが依存を読み込む形か（`import("./pad")` / `require("./pad")`）。
///
/// 動的 import も CommonJS の `require` も依存の宣言なので拾う。
/// `registry.require("./pad")` は呼ばれる側が `member_expression` になるので外れる。
///
/// **`require` という綴りが同じファイルで束縛され直していても区別できない。**
/// 束縛を追うには束縛の形の一覧を持つことになり、そちらのほうが漏れる
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
fn is_dependency_call(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if node.kind() != "call_expression" {
        return false;
    }
    let Some(called) = node.child_by_field_name("function") else {
        return false;
    };

    let calls_import = called.kind() == "import";
    let calls_require =
        called.kind() == "identifier" && tree.text_of(called) == Some(REQUIRE_FUNCTION_NAME);

    calls_import || calls_require
}

/// そのノードの直下にある最初の文字列リテラル。
fn first_string_child_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| child.kind() == "string")
}

/// その呼び出しの最初の引数が文字列リテラルなら、そのノード。
///
/// テンプレートリテラル（`` import(`./${name}`) ``）は書いた時点で先が決まらないので採らない。
fn first_string_argument_of(node: Node<'_>) -> Option<Node<'_>> {
    let arguments = node.child_by_field_name("arguments")?;

    first_string_child_of(arguments)
}

/// 引用符の中身。空文字列（`import("")`）のときは中身のノードが無いので `None`。
fn unquoted_text_of<'source>(
    tree: &SyntaxTree<'source>,
    literal: Node<'_>,
) -> Option<&'source str> {
    let mut cursor = literal.walk();
    let fragment = literal
        .named_children(&mut cursor)
        .find(|child| child.kind() == "string_fragment")?;

    tree.text_of(fragment)
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
    fn test_import_set_ignores_a_require_given_a_template_literal() {
        // 動的 import と同じ理由で採らない（書いた時点で先が決まらない）。
        // テンプレートリテラルの中の断片まで拾う実装だと集合に "./stock" が入り、
        // 重なりが 0.5 に落ちる
        let real_import_and_a_templated_require = r#"import { pad } from "./pad";
const stock = require(`./stock`);
"#;

        assert_eq!(
            overlap(
                real_import_and_a_templated_require,
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
            None
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
            None
        );
    }
}
