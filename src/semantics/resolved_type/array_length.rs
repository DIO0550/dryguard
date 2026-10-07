//! 宣言側の位置から配列長を評価し、hover が落とした式をソースへ対応させる。

use std::collections::HashMap;
use std::path::PathBuf;

use super::{UnopenedReason, declared_spelling_of};
use crate::codebase::source_of;
use crate::lsp::{
    ClientError, DeclarationSite, DeclarationSiteOutcome, Session, SignatureText, SourceDocument,
    UniqueDeclarationSiteOutcome,
};
use crate::source_position::SourcePosition;
use crate::syntax::rust_callable::{
    ArithmeticOperator, ConstSyntaxError, RustArrayLengths, RustCallable, RustConstExpression,
    RustConstSource, primitive_spelling_of,
};
use crate::syntax::type_reference::TypeReference;

/// Why: usize の幅が最小のターゲットでも、中間演算のオーバーフローを起こさない範囲。
const MAXIMUM_ARRAY_LENGTH: u64 = u16::MAX as u64;
/// 定数連鎖の上限。循環と作業量の上限とは別に数える。
const MAXIMUM_CONSTANT_DEPTH: usize = 32;
/// Why: 小さい式を共有した定数 DAG も、連鎖全体の予算で止める。
const MAXIMUM_EVALUATION_STEPS: usize = 1024;

/// 関数ソースと hover の構造が対応するときだけ、元の式の値を差し込む。
///
/// # Errors
///
/// LSP の往復に失敗したとき。評価不能の理由は内側の Err に残す。
pub(crate) fn restored_array_spelling_of(
    session: &mut Session,
    document: &SourceDocument,
    position: SourcePosition,
    hover: &SignatureText,
    impl_header: Option<&str>,
) -> Result<Result<SignatureText, UnopenedReason>, ClientError> {
    let Some(answered) = RustArrayLengths::from_spelling(hover.as_str()) else {
        return Ok(Ok(hover.clone()));
    };
    if answered.len() == 0 {
        return Ok(Ok(hover.clone()));
    }
    let Some(source) = RustArrayLengths::from_function_source(document.source(), position) else {
        return Ok(Err(UnopenedReason::UnmatchedArrayLength));
    };
    if source.has_const_parameters() {
        return Ok(Ok(hover.clone()));
    }
    let correspondence = match ArrayLengthCorrespondence::new(source, answered, impl_header) {
        Ok(correspondence) => correspondence,
        Err(reason) => return Ok(Err(reason)),
    };
    let mut evaluator = ArrayLengthEvaluator::new(session);
    let values = match evaluator.values_of(document, &correspondence.source)? {
        Ok(values) => values,
        Err(reason) => return Ok(Err(reason)),
    };
    Ok(correspondence.restored_with(&values))
}

/// 長さの出現番号を持つ型構造が一致した、ソースと hover の組。
struct ArrayLengthCorrespondence {
    source: RustArrayLengths,
    hover: RustArrayLengths,
}

impl ArrayLengthCorrespondence {
    fn new(
        source: RustArrayLengths,
        hover: RustArrayLengths,
        impl_header: Option<&str>,
    ) -> Result<Self, UnopenedReason> {
        if !array_positions_match(&source, &hover, impl_header) {
            return Err(UnopenedReason::UnmatchedArrayLength);
        }
        Ok(Self { source, hover })
    }

    /// hover が具体的な長さを持つ場合は、復元する値との一致も確かめる。
    fn restored_with(&self, values: &[u64]) -> Result<SignatureText, UnopenedReason> {
        for (index, (expression, value)) in self.hover.expressions().zip(values).enumerate() {
            match expression {
                Ok(RustConstExpression::Integer(answered)) if answered != value => {
                    return Err(UnopenedReason::UnmatchedArrayLength);
                }
                Ok(RustConstExpression::Integer(_)) => {}
                Ok(RustConstExpression::Reference(_))
                | Ok(RustConstExpression::Binary { .. })
                | Err(_) => {
                    if !self.hover.is_hover_placeholder(index) {
                        return Err(UnopenedReason::UnmatchedArrayLength);
                    }
                }
            }
        }
        self.hover
            .with_values(values)
            .and_then(SignatureText::new)
            .ok_or(UnopenedReason::UnmatchedArrayLength)
    }
}

fn array_positions_match(
    source: &RustArrayLengths,
    hover: &RustArrayLengths,
    impl_header: Option<&str>,
) -> bool {
    if source.len() != hover.len() {
        return false;
    }
    let indices: Vec<_> = (0..source.len() as u64).collect();
    let Some(source) = source.with_values(&indices) else {
        return false;
    };
    let Some(hover) = hover.with_values(&indices) else {
        return false;
    };
    let source = RustCallable::from_spelling(&source, &|_| None, impl_header);
    let hover = RustCallable::from_spelling(&hover, &|_| None, impl_header);
    source.is_some() && source == hover
}

/// エイリアスの配列長は、その宣言の元の位置で評価する。
///
/// # Errors
///
/// LSP の往復に失敗したとき。評価不能の理由は内側の Err に残す。
pub(crate) fn evaluated_array_spelling_of(
    session: &mut Session,
    document: &SourceDocument,
    lengths: &RustArrayLengths,
) -> Result<Result<String, UnopenedReason>, ClientError> {
    let mut evaluator = ArrayLengthEvaluator::new(session);
    let values = match evaluator.values_of(document, lengths)? {
        Ok(values) => values,
        Err(reason) => return Ok(Err(reason)),
    };
    Ok(lengths
        .with_values(&values)
        .ok_or(UnopenedReason::UnmatchedArrayLength))
}

struct ArrayLengthEvaluator<'session> {
    session: &'session mut Session,
    sources: HashMap<PathBuf, Result<String, UnopenedReason>>,
    active: Vec<DeclarationSite>,
    remaining_steps: usize,
}

impl<'session> ArrayLengthEvaluator<'session> {
    fn new(session: &'session mut Session) -> Self {
        Self {
            session,
            sources: HashMap::new(),
            active: Vec::new(),
            remaining_steps: MAXIMUM_EVALUATION_STEPS,
        }
    }

    fn values_of(
        &mut self,
        document: &SourceDocument,
        lengths: &RustArrayLengths,
    ) -> Result<Result<Vec<u64>, UnopenedReason>, ClientError> {
        let mut values = Vec::new();
        for expression in lengths.expressions() {
            let expression = match parsed_expression_of(expression) {
                Ok(expression) => expression,
                Err(reason) => return Ok(Err(reason)),
            };
            match self.value_of(document, expression)? {
                Ok(value) => values.push(value),
                Err(reason) => return Ok(Err(reason)),
            }
        }
        Ok(Ok(values))
    }

    fn value_of(
        &mut self,
        document: &SourceDocument,
        expression: &RustConstExpression,
    ) -> Result<Result<u64, UnopenedReason>, ClientError> {
        let Some(remaining) = self.remaining_steps.checked_sub(1) else {
            return Ok(Err(UnopenedReason::ArrayLengthEvaluationLimit));
        };
        self.remaining_steps = remaining;
        match expression {
            RustConstExpression::Integer(value) => Ok(bounded_value_of(*value)),
            RustConstExpression::Reference(reference) => self.reference_of(document, reference),
            RustConstExpression::Binary {
                operator,
                left,
                right,
            } => {
                let left = match self.value_of(document, left)? {
                    Ok(value) => value,
                    Err(reason) => return Ok(Err(reason)),
                };
                let right = match self.value_of(document, right)? {
                    Ok(value) => value,
                    Err(reason) => return Ok(Err(reason)),
                };
                Ok(arithmetic_value_of(*operator, left, right))
            }
        }
    }

    fn reference_of(
        &mut self,
        document: &SourceDocument,
        reference: &TypeReference,
    ) -> Result<Result<u64, UnopenedReason>, ClientError> {
        // Why: hover が安定してから definition を尋ねる。
        if let Err(reason) =
            declared_spelling_of(self.session.hover(document, reference.position())?)
        {
            return Ok(Err(reason));
        }
        let site = match self
            .session
            .unique_definition(document, reference.position())?
        {
            UniqueDeclarationSiteOutcome::Ambiguous => {
                return Ok(Err(UnopenedReason::UnevaluableArrayLength));
            }
            UniqueDeclarationSiteOutcome::Unambiguous(outcome) => match outcome {
                DeclarationSiteOutcome::Answered(site) => site,
                DeclarationSiteOutcome::NoAnswer => {
                    return Ok(Err(UnopenedReason::NoDefinitionSite));
                }
                DeclarationSiteOutcome::Unreadable { .. } => {
                    return Ok(Err(UnopenedReason::UnreadableDefinition));
                }
                DeclarationSiteOutcome::NotSupported => {
                    return Ok(Err(UnopenedReason::DefinitionNotProvided));
                }
            },
        };
        if self.active.contains(&site) {
            return Ok(Err(UnopenedReason::CyclicArrayLength));
        }
        if self.active.len() >= MAXIMUM_CONSTANT_DEPTH {
            return Ok(Err(UnopenedReason::ArrayLengthEvaluationLimit));
        }
        let source = self
            .sources
            .entry(site.path().to_owned())
            .or_insert_with(|| {
                source_of(site.path()).map_err(|_| UnopenedReason::UnreadableDeclaringDocument)
            })
            .clone();
        let source = match source {
            Ok(source) => source,
            Err(reason) => return Ok(Err(reason)),
        };
        let Some(constant) = RustConstSource::from_source(&source, site.position()) else {
            return Ok(Err(UnopenedReason::UnevaluableArrayLength));
        };
        let Ok(declaration) = SourceDocument::new(site.path(), source) else {
            return Ok(Err(UnopenedReason::UnreadableDeclaringDocument));
        };
        self.session.open_document(&declaration)?;
        let primitive = match declared_spelling_of(
            self.session
                .hover(&declaration, constant.primitive.position())?,
        ) {
            Ok(primitive) => primitive,
            Err(reason) => return Ok(Err(reason)),
        };
        if primitive_spelling_of(primitive.as_str()).as_deref() != Some("usize") {
            return Ok(Err(UnopenedReason::UnevaluableArrayLength));
        }
        let expression = match parsed_expression_of(&constant.expression) {
            Ok(expression) => expression,
            Err(reason) => return Ok(Err(reason)),
        };
        self.active.push(site.clone());
        let result = self.value_of(&declaration, expression);
        self.active.pop();
        let value = match result? {
            Ok(value) => value,
            Err(reason) => return Ok(Err(reason)),
        };
        let hover = match declared_spelling_of(self.session.hover_at_declaration(&site)?) {
            Ok(hover) => hover,
            Err(reason) => return Ok(Err(reason)),
        };
        Ok(validate_constant_hover(&hover, value))
    }
}

fn validate_constant_hover(hover: &SignatureText, value: u64) -> Result<u64, UnopenedReason> {
    let Some((_, right)) = hover.as_str().split_once('=') else {
        return Err(UnopenedReason::UnevaluableArrayLength);
    };
    let right = right.trim().trim_end_matches(';').trim();
    if let Ok(answered) = right.parse::<u64>() {
        if answered != value {
            return Err(UnopenedReason::UnevaluableArrayLength);
        }
    }
    Ok(value)
}

fn parsed_expression_of(
    expression: &Result<RustConstExpression, ConstSyntaxError>,
) -> Result<&RustConstExpression, UnopenedReason> {
    expression.as_ref().map_err(|reason| match reason {
        ConstSyntaxError::Unsupported => UnopenedReason::UnevaluableArrayLength,
        ConstSyntaxError::Limit => UnopenedReason::ArrayLengthEvaluationLimit,
    })
}

fn bounded_value_of(value: u64) -> Result<u64, UnopenedReason> {
    if value > MAXIMUM_ARRAY_LENGTH {
        return Err(UnopenedReason::ArrayLengthEvaluationLimit);
    }
    Ok(value)
}

fn arithmetic_value_of(
    operator: ArithmeticOperator,
    left: u64,
    right: u64,
) -> Result<u64, UnopenedReason> {
    let value = match operator {
        ArithmeticOperator::Add => left.checked_add(right),
        ArithmeticOperator::Subtract => left.checked_sub(right),
        ArithmeticOperator::Multiply => left.checked_mul(right),
        ArithmeticOperator::Divide => left.checked_div(right),
        ArithmeticOperator::Remainder => left.checked_rem(right),
    }
    .ok_or(UnopenedReason::UnevaluableArrayLength)?;
    bounded_value_of(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_hover_accepts_matching_value_with_flexible_whitespace() {
        for spelling in [
            "const COUNT: usize = 4",
            "const COUNT: usize = 4 ; ",
            "const COUNT: usize=4",
            "const COUNT: usize\t=\t4;",
            "const COUNT: usize\n=\n 4 ; \n",
        ] {
            let hover = SignatureText::new(spelling.to_owned()).expect("定数の hover");
            assert_eq!(validate_constant_hover(&hover, 4), Ok(4), "{spelling}");
        }
    }

    #[test]
    fn test_constant_hover_rejects_different_value_with_flexible_whitespace() {
        for spelling in [
            "const COUNT: usize = 5",
            "const COUNT: usize = 5 ; ",
            "const COUNT: usize=5",
            "const COUNT: usize\t=\t5;",
            "const COUNT: usize\n=\n 5 ; \n",
        ] {
            let hover = SignatureText::new(spelling.to_owned()).expect("定数の hover");
            assert_eq!(
                validate_constant_hover(&hover, 4),
                Err(UnopenedReason::UnevaluableArrayLength),
                "{spelling}"
            );
        }
    }

    #[test]
    fn test_constant_hover_without_initializer_is_unevaluable() {
        let hover = SignatureText::new("const COUNT: usize".to_owned()).expect("定数の hover");
        assert_eq!(
            validate_constant_hover(&hover, 4),
            Err(UnopenedReason::UnevaluableArrayLength)
        );
    }

    #[test]
    fn test_constant_hover_with_symbolic_initializer_keeps_evaluated_source_value() {
        let hover =
            SignatureText::new("const COUNT: usize=BASE + 2".to_owned()).expect("定数の hover");
        assert_eq!(validate_constant_hover(&hover, 4), Ok(4));
    }

    #[test]
    fn test_array_correspondence_restores_each_parameter_and_return_occurrence() {
        let source = "fn f(a: ([u8; COUNT], [u8; 2])) -> [[u8; 3]; OTHER] { [a.0; 4] }";
        let position = SourcePosition::from_preceding_text(
            crate::line_number::LineNumber::from_index(0),
            "fn ",
        );
        let source = RustArrayLengths::from_function_source(source, position).unwrap();
        let hover = RustArrayLengths::from_spelling(
            "fn f(a: ([u8; {const}], [u8; {const}])) -> [[u8; {const}]; {const}]",
        )
        .unwrap();
        let correspondence = ArrayLengthCorrespondence::new(source, hover, None)
            .ok()
            .unwrap();
        assert_eq!(
            correspondence
                .restored_with(&[4, 2, 3, 5])
                .unwrap()
                .as_str(),
            "fn f(a: ([u8; 4], [u8; 2])) -> [[u8; 3]; 5]"
        );
    }

    #[test]
    fn test_array_correspondence_rejects_different_counts_or_positions() {
        for hover in [
            "fn f(a: [u8; {const}]) -> u8",
            "fn f(a: u8) -> ([u8; {const}], [u8; {const}])",
            "fn f(a: [u8; {const}]) -> [u64; {const}]",
        ] {
            let source =
                RustArrayLengths::from_spelling("fn f(a: [u8; COUNT]) -> [u8; OTHER]").unwrap();
            let hover = RustArrayLengths::from_spelling(hover).unwrap();
            assert!(matches!(
                ArrayLengthCorrespondence::new(source, hover, None),
                Err(UnopenedReason::UnmatchedArrayLength)
            ));
        }
    }

    #[test]
    fn test_array_correspondence_does_not_treat_unknown_hover_expressions_as_placeholders() {
        for spelling in [
            "fn f(a: [u8; OTHER])",
            "fn f(a: [u8; 2 + 2])",
            "fn f(a: [u8; unknown()])",
            "fn f(a: [u8; { co nst }])",
        ] {
            let source = RustArrayLengths::from_spelling("fn f(a: [u8; COUNT])").unwrap();
            let hover = RustArrayLengths::from_spelling(spelling).unwrap();
            let correspondence = ArrayLengthCorrespondence::new(source, hover, None)
                .ok()
                .unwrap();
            assert_eq!(
                correspondence.restored_with(&[4]),
                Err(UnopenedReason::UnmatchedArrayLength)
            );
        }
    }

    #[test]
    fn test_array_correspondence_checks_concrete_hover_lengths_before_replacing_them() {
        let source = RustArrayLengths::from_spelling("fn f(a: [u8; COUNT])").unwrap();
        let hover = RustArrayLengths::from_spelling("fn f(a: [u8; 5])").unwrap();
        let correspondence = ArrayLengthCorrespondence::new(source, hover, None)
            .ok()
            .unwrap();
        assert_eq!(
            correspondence.restored_with(&[4]),
            Err(UnopenedReason::UnmatchedArrayLength)
        );
        assert_eq!(
            correspondence.restored_with(&[5]).unwrap().as_str(),
            "fn f(a: [u8; 5])"
        );
    }
}
