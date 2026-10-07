//! 配列長の出現と、その元のソース位置を持つ。値の評価と LSP は semantics が担う。

use std::ops::Range;

use tree_sitter::Node;

use super::{FUNCTION_SIGNATURE_KIND, named_children_of};
use crate::source_position::SourcePosition;
use crate::syntax::tree::{Grammar, SyntaxTree, source_position_of};
use crate::syntax::type_reference::TypeReference;

/// 1 つの式から読み取るノード数と深さの上限。
const MAXIMUM_EXPRESSION_NODES: usize = 128;
const MAXIMUM_EXPRESSION_DEPTH: usize = 32;

/// 値として解釈する前の、対応できる整数定数式。
#[derive(Debug)]
pub(crate) enum RustConstExpression {
    Integer(u64),
    Reference(TypeReference),
    Binary {
        operator: ArithmeticOperator,
        left: Box<Self>,
        right: Box<Self>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ArithmeticOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstSyntaxError {
    Unsupported,
    Limit,
}

/// 1 つの宣言の綴りと、その綴り内の配列長。問い合わせ位置は元のファイルの位置。
pub(crate) struct RustArrayLengths {
    spelling: String,
    lengths: Vec<ArrayLength>,
    const_parameters: bool,
}

struct ArrayLength {
    range: Range<usize>,
    expression: Result<RustConstExpression, ConstSyntaxError>,
}

impl RustArrayLengths {
    /// 本体を含めず、名前の位置が一致する関数のヘッダーから読む。
    pub(crate) fn from_function_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let function = tree.named_descendants().into_iter().find(|node| {
            let is_function = matches!(node.kind(), "function_item" | FUNCTION_SIGNATURE_KIND);
            is_function
                && node
                    .child_by_field_name("name")
                    .and_then(|name| source_position_of(name, source))
                    == Some(position)
        })?;
        let end = function
            .child_by_field_name("body")
            .map_or(function.end_byte(), |body| body.start_byte());
        Self::from_node(function, source, end)
    }

    /// hover の綴りを読む。`{const}` の式を読めなくても置換範囲は残す。
    pub(crate) fn from_spelling(spelling: &str) -> Option<Self> {
        let source = format!("{};", spelling.trim_end_matches(';'));
        let tree = SyntaxTree::from_source(&source, Grammar::Rust).ok()?;
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == FUNCTION_SIGNATURE_KIND)?;
        let mut lengths = Self::from_node(function, &source, function.end_byte())?;
        // Why: impl の前置きや境界も含めて照合するため、hover 全体の綴りを保持する。
        let offset = function.start_byte();
        for length in &mut lengths.lengths {
            length.range.start += offset;
            length.range.end += offset;
        }
        lengths.spelling = spelling.to_owned();
        Some(lengths)
    }

    /// 宣言の指定範囲から読む。ソース範囲を読み取れなければ None。
    pub(super) fn from_node(node: Node<'_>, source: &str, end: usize) -> Option<Self> {
        let start = node.start_byte();
        let mut pending = vec![node];
        let mut lengths = Vec::new();
        let mut const_parameters = false;
        while let Some(current) = pending.pop() {
            if current.start_byte() >= end {
                continue;
            }
            const_parameters |= current.kind() == "const_parameter";
            if current.kind() == "array_type" {
                if let Some(length) = current.child_by_field_name("length") {
                    let mut budget = MAXIMUM_EXPRESSION_NODES;
                    lengths.push(ArrayLength {
                        range: length.start_byte() - start..length.end_byte() - start,
                        expression: expression_of(length, source, &mut budget, 0),
                    });
                }
                // Why: 長さの式内の型は、この配列型の構造上の配列長ではない。
                if let Some(element) = current.child_by_field_name("element") {
                    pending.push(element);
                }
                continue;
            }
            let children: Vec<_> = named_children_of(current).collect();
            pending.extend(children.into_iter().rev());
        }
        lengths.sort_by_key(|length| length.range.start);
        Some(Self {
            spelling: source.get(start..end)?.to_owned(),
            lengths,
            const_parameters,
        })
    }

    /// 元のファイルで位置が確定した配列長の式。未対応や上限も出現ごとに残す。
    pub(crate) fn expressions(
        &self,
    ) -> impl Iterator<Item = &Result<RustConstExpression, ConstSyntaxError>> {
        self.lengths.iter().map(|length| &length.expression)
    }

    /// この宣言に直接含まれる配列長の出現数。
    pub(crate) fn len(&self) -> usize {
        self.lengths.len()
    }

    /// 関数自身が const パラメータを宣言しているか。
    pub(crate) fn has_const_parameters(&self) -> bool {
        self.const_parameters
    }

    /// rust-analyzer が失った式を表す既知の印か。未知の式は印として扱わない。
    pub(crate) fn is_hover_placeholder(&self, index: usize) -> bool {
        self.lengths
            .get(index)
            .and_then(|length| self.spelling.get(length.range.clone()))
            .is_some_and(|spelling| {
                spelling
                    .trim()
                    .strip_prefix('{')
                    .and_then(|spelling| spelling.strip_suffix('}'))
                    .is_some_and(|spelling| spelling.trim() == "const")
            })
    }

    /// すべての出現へ値を差し込む。個数が合わなければ `None`。
    pub(crate) fn with_values(&self, values: &[u64]) -> Option<String> {
        if values.len() != self.lengths.len() {
            return None;
        }
        let mut spelling = self.spelling.clone();
        for (length, value) in self.lengths.iter().zip(values).rev() {
            spelling.replace_range(length.range.clone(), &value.to_string());
        }
        Some(spelling)
    }
}

/// definition が指した自由な usize 定数。関連定数とジェネリックな囲みからは作らない。
pub(crate) struct RustConstSource {
    pub(crate) expression: Result<RustConstExpression, ConstSyntaxError>,
    pub(crate) primitive: TypeReference,
}

impl RustConstSource {
    /// 宣言位置から読む。usize 以外・関連定数・ジェネリックな囲み・壊れた宣言は None。
    pub(crate) fn from_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let constant = tree.named_descendants().into_iter().find(|node| {
            node.kind() == "const_item"
                && node
                    .child_by_field_name("name")
                    .and_then(|name| source_position_of(name, source))
                    == Some(position)
        })?;
        if constant.has_error() {
            return None;
        }
        let mut parent = constant.parent();
        while let Some(node) = parent {
            let dependent_scope = matches!(node.kind(), "impl_item" | "trait_item")
                || node.child_by_field_name("type_parameters").is_some();
            if dependent_scope {
                return None;
            }
            parent = node.parent();
        }
        let primitive = constant.child_by_field_name("type")?;
        if source.get(primitive.byte_range()) != Some("usize") {
            return None;
        }
        let mut budget = MAXIMUM_EXPRESSION_NODES;
        Some(Self {
            expression: expression_of(
                constant.child_by_field_name("value")?,
                source,
                &mut budget,
                0,
            ),
            primitive: TypeReference::new(
                "usize".to_owned(),
                source_position_of(primitive, source)?,
            ),
        })
    }
}

fn expression_of(
    node: Node<'_>,
    source: &str,
    budget: &mut usize,
    depth: usize,
) -> Result<RustConstExpression, ConstSyntaxError> {
    if depth >= MAXIMUM_EXPRESSION_DEPTH {
        return Err(ConstSyntaxError::Limit);
    }
    *budget = budget.checked_sub(1).ok_or(ConstSyntaxError::Limit)?;
    if node.has_error() {
        return Err(ConstSyntaxError::Unsupported);
    }
    let text = source
        .get(node.byte_range())
        .ok_or(ConstSyntaxError::Unsupported)?;
    match node.kind() {
        "integer_literal" => integer_of(text).map(RustConstExpression::Integer),
        "identifier" | "scoped_identifier" => {
            let query = node.child_by_field_name("name").unwrap_or(node);
            let position =
                source_position_of(query, source).ok_or(ConstSyntaxError::Unsupported)?;
            Ok(RustConstExpression::Reference(TypeReference::new(
                text.to_owned(),
                position,
            )))
        }
        "binary_expression" => {
            let operator = node
                .child_by_field_name("operator")
                .and_then(|operator| source.get(operator.byte_range()))
                .ok_or(ConstSyntaxError::Unsupported)?;
            let operator = match operator {
                "+" => ArithmeticOperator::Add,
                "-" => ArithmeticOperator::Subtract,
                "*" => ArithmeticOperator::Multiply,
                "/" => ArithmeticOperator::Divide,
                "%" => ArithmeticOperator::Remainder,
                _ => return Err(ConstSyntaxError::Unsupported),
            };
            let left = node
                .child_by_field_name("left")
                .ok_or(ConstSyntaxError::Unsupported)?;
            let right = node
                .child_by_field_name("right")
                .ok_or(ConstSyntaxError::Unsupported)?;
            Ok(RustConstExpression::Binary {
                operator,
                left: Box::new(expression_of(left, source, budget, depth + 1)?),
                right: Box::new(expression_of(right, source, budget, depth + 1)?),
            })
        }
        "parenthesized_expression" | "block" | "const_block" => {
            let children: Vec<_> = named_children_of(node).collect();
            let [inner] = children.as_slice() else {
                return Err(ConstSyntaxError::Unsupported);
            };
            expression_of(*inner, source, budget, depth + 1)
        }
        _ => Err(ConstSyntaxError::Unsupported),
    }
}

pub(super) fn integer_of(text: &str) -> Result<u64, ConstSyntaxError> {
    let text = text.strip_suffix("usize").unwrap_or(text).replace('_', "");
    let (radix, digits) = [("0x", 16), ("0o", 8), ("0b", 2)]
        .into_iter()
        .find_map(|(prefix, radix)| text.strip_prefix(prefix).map(|digits| (radix, digits)))
        .unwrap_or((10, text.as_str()));
    if !digits.chars().all(|digit| digit.is_digit(radix)) {
        return Err(ConstSyntaxError::Unsupported);
    }
    u64::from_str_radix(digits, radix).map_err(|_| ConstSyntaxError::Limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_number::LineNumber;

    #[test]
    fn test_array_source_keeps_utf16_reference_positions_and_excludes_the_body() {
        let source = "/*🦀*/ fn f(a: [u8; counts::COUNT]) -> [u8; 2] { let a: [u8; BODY] = a; a }";
        let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), "/*🦀*/ fn ");
        let arrays = RustArrayLengths::from_function_source(source, position).unwrap();
        assert_eq!(arrays.len(), 2);
        let Ok(RustConstExpression::Reference(reference)) = arrays.expressions().next().unwrap()
        else {
            panic!("定数の名前と位置が取れる");
        };
        assert_eq!(reference.name(), "counts::COUNT");
        assert_eq!(
            reference.position(),
            SourcePosition::from_preceding_text(
                LineNumber::from_index(0),
                "/*🦀*/ fn f(a: [u8; counts::"
            )
        );
        assert_eq!(
            arrays.with_values(&[4, 3]).unwrap(),
            "fn f(a: [u8; 4]) -> [u8; 3] "
        );
    }

    #[test]
    fn test_array_source_preserves_unsupported_lengths_as_errors() {
        for length in [
            "COUNT as usize",
            "count()",
            "-1",
            "4u8",
            "{ let n = 4; n }",
            "1 << 2",
        ] {
            let source = format!("fn f(a: [u8; {length}]) {{}}");
            let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), "fn ");
            let arrays = RustArrayLengths::from_function_source(&source, position).unwrap();
            assert!(
                matches!(
                    arrays.expressions().next().unwrap(),
                    Err(ConstSyntaxError::Unsupported)
                ),
                "{length}"
            );
        }
    }

    #[test]
    fn test_array_source_reads_the_supported_integer_representations() {
        for (length, value) in [
            ("0xF_Fusize", 255),
            ("0o377", 255),
            ("0b1111_1111", 255),
            ("255", 255),
        ] {
            let source = format!("fn f(a: [u8; {length}]) {{}}");
            let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), "fn ");
            let arrays = RustArrayLengths::from_function_source(&source, position).unwrap();
            assert!(
                matches!(arrays.expressions().next().unwrap(), Ok(RustConstExpression::Integer(actual)) if *actual == value),
                "{length}"
            );
        }
    }

    #[test]
    fn test_array_source_expression_limits_keep_the_allowed_depth_readable() {
        for (wrappers, limited) in [(31, false), (32, true)] {
            let length = format!("{}1{}", "(".repeat(wrappers), ")".repeat(wrappers));
            let source = format!("fn f(a: [u8; {length}]) {{}}");
            let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), "fn ");
            let arrays = RustArrayLengths::from_function_source(&source, position).unwrap();
            assert_eq!(
                matches!(
                    arrays.expressions().next().unwrap(),
                    Err(ConstSyntaxError::Limit)
                ),
                limited,
                "{wrappers}"
            );
        }
    }

    #[test]
    fn test_array_source_limits_wide_expressions_and_overflowing_integer_literals() {
        let mut length = "1".to_owned();
        for _ in 0..6 {
            length = format!("({length} + {length})");
        }
        for length in [length.as_str(), "18446744073709551616"] {
            let source = format!("fn f(a: [u8; {length}]) {{}}");
            let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), "fn ");
            let arrays = RustArrayLengths::from_function_source(&source, position).unwrap();
            assert!(matches!(
                arrays.expressions().next().unwrap(),
                Err(ConstSyntaxError::Limit)
            ));
        }
    }

    #[test]
    fn test_constant_source_rejects_associated_or_generic_declarations() {
        for prefix in [
            "impl Model { const ",
            "trait Model { const ",
            "fn outer<T>() { const ",
        ] {
            let source = format!("{prefix}COUNT: usize = 4; }}");
            let position = SourcePosition::from_preceding_text(LineNumber::from_index(0), prefix);
            assert!(
                RustConstSource::from_source(&source, position).is_none(),
                "{source}"
            );
        }
        let source = "mod counts { const COUNT: usize = 4; }";
        let position =
            SourcePosition::from_preceding_text(LineNumber::from_index(0), "mod counts { const ");
        assert!(RustConstSource::from_source(source, position).is_some());
    }
}
