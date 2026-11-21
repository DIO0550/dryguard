//! Swift の参照元ファイルが、テストのためだけにあるファイルかを import から調べる。
//!
//! **範囲ではなくファイルで決める。** Swift のテストは `XCTestCase` を継承したクラスの
//! `extension`（別のファイルに書けるので、継承を構文から確かめられない）や、ファイルの直下の
//! `private struct`（テストが解析させるコマンドの定義）からも本番のコードを参照する。
//! Rust の `rust_test_scope` のように構文の範囲で囲むと、それらを本番の参照として数える。
//! テストフレームワークを import するのはテストのターゲットとテスト支援のライブラリなので、
//! **import がファイル全体の印になる**。本番のモジュールが `#if canImport(XCTest)` の中で import すると、
//! そのファイルの参照も除く（除く側へ倒れる）。

use crate::line_number::LineNumber;
use crate::syntax::import::{swift_import_outcomes_of, swift_module_segments_of};
use crate::syntax::tree::{Grammar, SyntaxTree};

/// テストフレームワークのモジュール名。`import` の綴りの先頭の要素を、バッククォートを外して見る
/// （`import class XCTest.XCTestCase` / `` import `XCTest` `` も当たる）。
///
/// **名前の前方一致で見ない。** `XCTestDynamicOverlay` のように本番のコードが import する
/// モジュールが、テストフレームワークの名前で始まることがある。一覧に無いフレームワーク
/// （Quick など）だけを import するファイルは本番の参照として数える（除外しない側へ倒れる）。
const SWIFT_TEST_FRAMEWORK_MODULES: [&str; 2] = ["XCTest", "Testing"];

/// テストフレームワークを import しているかを言えなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwiftImportsUnreadable {
    /// tree-sitter が構文木を返さなかった。
    NoTree,
    /// import の宣言を読み切れなかった。その宣言の行。
    Declaration { line: LineNumber },
}

/// Swift ソースがテストフレームワークを import しているか。
///
/// `#if canImport(…)` の中の import も、条件に関わらず数える。
///
/// # Errors
/// 構文木を作れないとき。読めた import にテストフレームワークが無く、読み切れなかった
/// import の宣言があるとき（その宣言がテストフレームワークかを言えない。最初のその行）。
/// **読めた import にテストフレームワークがあれば、読めない宣言があっても答える** —
/// 欠けた宣言が何であっても答えは変わらない（`rules/architecture.md`
/// 「どこまでを「取れなかった」に数えるか」）。import 以外の構文エラーでも同じく落とさない。
pub(crate) fn imports_swift_test_framework(source: &str) -> Result<bool, SwiftImportsUnreadable> {
    let tree = SyntaxTree::from_source(source, Grammar::Swift)
        .map_err(|_| SwiftImportsUnreadable::NoTree)?;
    let outcomes = swift_import_outcomes_of(&tree);
    let imports_test_framework = outcomes.iter().flatten().any(|specifier| {
        swift_module_segments_of(specifier)
            .next()
            .is_some_and(|module| SWIFT_TEST_FRAMEWORK_MODULES.contains(&module))
    });
    if imports_test_framework {
        return Ok(true);
    }
    match outcomes.into_iter().find_map(Result::err) {
        Some(line) => Err(SwiftImportsUnreadable::Declaration { line }),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swift_source_importing_xctest_or_testing_is_a_test_source() {
        for source in [
            "import XCTest\nfinal class CartTests: XCTestCase {}\n",
            "@testable import Shop\nimport Testing\n",
            "import class XCTest.XCTestCase\n",
            "#if canImport(Testing)\nimport Testing\n#endif\n",
            // tree-sitter-swift の `import_declaration` は `modifiers? import _import_kind? identifier`。
            // 修飾子（属性・アクセス修飾子）と種類の指定、要素のバッククォートを並べる
            "@_exported import XCTest\n",
            "@preconcurrency import XCTest\n",
            "public import Testing\n",
            "import struct Testing.Test\n",
            "import `XCTest`\n",
        ] {
            assert_eq!(imports_swift_test_framework(source), Ok(true), "{source}");
        }
    }

    #[test]
    fn test_swift_source_importing_only_production_modules_is_not_a_test_source() {
        // 対照は上のテスト。テストフレームワークの名前で始まる別のモジュールと、
        // import の無いファイル・`@testable` だけのファイルは本番として数える
        for source in [
            "import Foundation\nimport XCTestDynamicOverlay\n",
            "struct Fixture {}\n",
            "@testable import Shop\n",
        ] {
            assert_eq!(imports_swift_test_framework(source), Ok(false), "{source}");
        }
    }

    #[test]
    fn test_swift_source_with_an_unreadable_import_and_no_test_framework_says_which_line() {
        assert_eq!(
            imports_swift_test_framework("import Foundation\nimport func Swift.+\n"),
            Err(SwiftImportsUnreadable::Declaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_swift_source_importing_a_test_framework_is_answered_beside_an_unreadable_import() {
        // 対照は上のテスト。読めない宣言が何であっても、XCTest を import している答えは変わらない
        assert_eq!(
            imports_swift_test_framework("import func Swift.+\nimport XCTest\n"),
            Ok(true)
        );
    }

    #[test]
    fn test_swift_source_with_syntax_errors_outside_imports_is_still_answered() {
        assert_eq!(
            imports_swift_test_framework("import XCTest\nfunc broken( {\n"),
            Ok(true)
        );
    }
}
