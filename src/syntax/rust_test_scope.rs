//! Rust の参照位置がテスト専用の構文範囲にあるかを調べる。

use std::ops::Range;

use tree_sitter::Node;

use super::chunk::{has_attribute, has_test_attribute};
use super::tree::{Grammar, SyntaxTree};
use crate::source_position::SourcePosition;

/// ソース内で、テスト属性または単純な `cfg(test)` が覆う半開区間。
#[derive(Debug)]
pub(crate) struct RustTestScopes {
    source: String,
    ranges: Vec<Range<usize>>,
    lines: Vec<Range<usize>>,
}

impl RustTestScopes {
    /// Rust ソースを解析する。構文を読み切れなければ `None`。
    pub(crate) fn from_source(source: String) -> Option<Self> {
        let tree = SyntaxTree::from_source(&source, Grammar::Rust).ok()?;
        if tree.has_error() {
            return None;
        }
        let ranges = tree
            .named_descendants()
            .into_iter()
            .filter_map(|node| {
                let test_function =
                    node.kind() == "function_item" && has_test_attribute(node, &source);
                let test_scope = test_function
                    || has_attribute(node, &source, is_cfg_test)
                    || has_inner_cfg_test(node, &source);
                test_scope.then(|| node.byte_range())
            })
            .collect();
        let mut start = 0;
        let lines = source
            .split_inclusive('\n')
            .map(|line| {
                let range = start..start + line.len();
                start = range.end;
                range
            })
            .collect();
        Some(Self {
            source,
            ranges,
            lines,
        })
    }

    /// 参照開始位置がテスト範囲内か。行・UTF-16 列が文字を指さなければ `None`。
    pub(crate) fn contains(&self, position: SourcePosition) -> Option<bool> {
        let range = self.lines.get(position.line().to_index())?;
        let line_start = range.start;
        let line = &self.source[range.clone()];
        let mut utf16_column = 0;
        for (byte_column, character) in line.char_indices() {
            if utf16_column == position.character() {
                if matches!(character, '\r' | '\n') {
                    return None;
                }
                let offset = line_start + byte_column;
                return Some(self.ranges.iter().any(|range| range.contains(&offset)));
            }
            utf16_column += character.len_utf16();
        }
        None
    }
}

/// 内部属性は、その属性が属するソース・モジュール本体を丸ごと覆う。
fn has_inner_cfg_test(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "inner_attribute_item" && is_cfg_test(child, source))
}

/// `cfg(test)` の構文だけを認める。複合 cfg / cfg_attr は評価しない。
fn is_cfg_test(item: Node<'_>, source: &str) -> bool {
    let Some(attribute) = item.named_child(0) else {
        return false;
    };
    let Some(name) = attribute.named_child(0) else {
        return false;
    };
    if source.get(name.byte_range()) != Some("cfg") {
        return false;
    }
    let Some(arguments) = attribute.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = arguments.walk();
    let tokens: Vec<_> = arguments
        .children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "line_comment" | "block_comment"))
        .filter_map(|child| source.get(child.byte_range()))
        .collect();
    matches!(
        tokens.as_slice(),
        ["(", "test", ")"] | ["(", "test", ",", ")"]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_position::SourcePosition;
    use crate::test_support::line;

    /// 同じソース内の target の出現をすべて照合し、残す側も一緒に検証する。
    fn assert_test_flags(source: &str, expected: &[bool]) {
        let scopes = RustTestScopes::from_source(source.to_owned()).expect("正しい Rust");
        let actual: Vec<_> = source
            .match_indices("target")
            .map(|(offset, _)| {
                let preceding = &source[..offset];
                let line_number = preceding.bytes().filter(|byte| *byte == b'\n').count() + 1;
                let line_start = preceding.rfind('\n').map_or(0, |index| index + 1);
                let position = SourcePosition::from_preceding_text(
                    line(line_number),
                    &source[line_start..offset],
                );
                scopes.contains(position).expect("文字の先頭を指す")
            })
            .collect();
        assert!(!actual.is_empty(), "検証する参照がある");
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rust_test_scope_excludes_helpers_and_nested_bodies_in_cfg_test_modules() {
        assert_test_flags(
            r#"
#[cfg(test)]
mod checks {
    fn helper() { target(); }
    mod nested { fn helper() { let run = || target(); } }
}
fn production() { target(); }
"#,
            &[true, true, false],
        );
    }

    #[test]
    fn test_rust_test_scope_recognizes_inner_cfg_in_modules_and_files() {
        assert_test_flags(
            "mod checks { #![cfg(test)] fn helper() { target(); } } fn prod() { target(); }",
            &[true, false],
        );
        assert_test_flags("#![cfg(test)]\nfn helper() { target(); }", &[true]);
        assert_test_flags(
            "#![cfg(not(test))]\nfn production() { target(); }",
            &[false],
        );
    }

    #[test]
    fn test_rust_test_scope_recognizes_scoped_attributes_and_nested_test_functions() {
        assert_test_flags(
            r#"
#[tokio::test]
// keep the attribute attached
#[ignore]
async fn check() { fn helper() { target(); } target(); }
fn production() { #[test] fn check() { target(); } target(); }
"#,
            &[true, true, true, false],
        );
    }

    #[test]
    fn test_rust_test_scope_does_not_treat_strings_comments_or_cfg_negation_as_attributes() {
        assert_test_flags(
            r##"
// #[test]
fn production() { let text = "#[cfg(test)] target"; target(); }
#[cfg(not(test))]
mod production { fn helper() { target(); } }
#[cfg(any(test, feature = "shipping"))]
fn either() { target(); }
"##,
            &[false, false, false, false],
        );
    }

    #[test]
    fn test_rust_test_scope_accepts_comments_and_trailing_comma_in_cfg_test() {
        assert_test_flags(
            "#[cfg(/* reason */ test,)] mod checks { fn helper() { target(); } } fn prod() { target(); }",
            &[true, false],
        );
    }

    #[test]
    fn test_rust_test_scope_counts_utf16_and_handles_crlf_and_scope_end() {
        assert_test_flags(
            "const TEXT: &str = \"日本😀\"; #[test] fn check() { target(); } fn prod() { target(); }\r\nfn other() { target(); }",
            &[true, false, false],
        );
    }

    #[test]
    fn test_rust_test_scope_rejects_invalid_utf16_positions_and_syntax() {
        let scopes = RustTestScopes::from_source("fn f() { let text = \"😀\"; }".to_owned())
            .expect("正しい Rust");
        for position in [
            lsp_types::Position::new(9, 0),
            lsp_types::Position::new(0, 999),
            lsp_types::Position::new(0, 22),
        ] {
            assert_eq!(
                scopes.contains(SourcePosition::from_lsp_position(position)),
                None
            );
        }
        assert!(RustTestScopes::from_source("#[test] fn f( {".to_owned()).is_none());
    }

    #[test]
    fn test_rust_test_scope_distinguishes_production_and_test_on_the_same_line() {
        let source = "#[test] fn check() { target(); } fn production() { target(); }";
        let scopes = RustTestScopes::from_source(source.to_owned()).expect("正しい Rust");
        let positions: Vec<_> = source
            .match_indices("target")
            .map(|(offset, _)| SourcePosition::from_preceding_text(line(1), &source[..offset]))
            .collect();
        assert_eq!(scopes.contains(positions[0]), Some(true));
        assert_eq!(scopes.contains(positions[1]), Some(false));
    }
}
