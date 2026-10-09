//! Self を直接の対象にした完全修飾形の関連型について、別の impl を選ぶための構文情報。
//!
//! **選ぶのは、候補 impl の型変数を使用側の impl の型変数へ付け替えるだけで、
//! trait 引数と対象型が一致する impl だけ。** coherence により、そうした impl があれば
//! 他の impl は重ならないので、blanket impl を含む残りの候補を数えずに 1 つへ決められる。
//! 型変数が具体的な型に当たる候補・境界や where 句を持つ候補は選ばない（偽陰性側）。
//!
//! **型名の同一性はここでは決めない。** 対応する位置の型名の組を返し、
//! 宣言元の照合は `semantics::resolved_type` が両側のソースの位置から行う。

use super::projection::{ancestor_of_kind, has_attribute, projection_at, terminal_name_of};
use super::*;
use crate::source_position::SourcePosition;

/// 使用側の `<Self as Trait<Args>>::Name` と、直接囲む impl の文脈。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustQualifiedProjection {
    associated_name: String,
    trait_position: SourcePosition,
    target_name: String,
    parameters: Vec<String>,
    shapes: Vec<TypeShape>,
}

impl RustQualifiedProjection {
    /// 使用位置の投影を読む。次のどれかなら `None`:
    /// - 対象が `Self` 自体でない・短縮形（`Self::Name`）・入れ子の投影
    /// - 直接囲む impl が同じ trait パスの impl（直接の impl の経路が答える）
    /// - impl / メソッドの where 句に、左辺が型変数そのものでない述語がある（`Self` や
    ///   対象型を左辺に持つ param-env の候補は impl より優先され、rustc も投影を正規化しない）
    /// - impl が const 引数を持つ・trait 引数に関連型の束縛や `Self` がある
    pub(crate) fn from_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let projection = projection_at(&tree, source, position)?;
        let path = projection.child_by_field_name("path")?;
        if path.kind() != "bracketed_type" {
            return None;
        }
        let qualified = named_children_of(path).next()?;
        let direct_self = qualified.kind() == "qualified_type"
            && qualified
                .child_by_field_name("type")
                .is_some_and(|node| source.get(node.byte_range()) == Some(SELF_TYPE));
        if !direct_self {
            return None;
        }
        let projected_trait = qualified.child_by_field_name("alias")?;
        let function = ancestor_of_kind(projection, "function_item")?;
        let implementation = enclosing_impl_of(function)?;
        if signature_has_error(implementation) || signature_has_error(function) {
            return None;
        }
        let same_trait_as_enclosing =
            implementation
                .child_by_field_name("trait")
                .is_some_and(|implemented| {
                    projection_name_of(implemented, source)
                        == projection_name_of(projected_trait, source)
                });
        if same_trait_as_enclosing {
            return None;
        }
        let target = implementation.child_by_field_name("type")?;
        let parameters = type_parameter_names_of(implementation, source)?;
        let method_parameters = type_parameter_names_of(function, source)?;
        // Why: 左辺が型変数そのもの以外の述語は、Self や対象型を別の綴り（パス・エイリアス）で
        // 書いた param-env の候補でありうる。綴りの一覧で弾くと漏れが選ぶ側へ倒れるので、
        // 型変数そのものだけを通す許可リストにする。
        let only_variables_constrained = [function, implementation]
            .into_iter()
            .flat_map(where_predicate_lefts_of)
            .all(|left| {
                left.kind() == "type_identifier"
                    && source.get(left.byte_range()).is_some_and(|name| {
                        parameters
                            .iter()
                            .chain(&method_parameters)
                            .any(|parameter| parameter == name)
                    })
            });
        if !only_variables_constrained {
            return None;
        }
        let mut shapes = trait_argument_shapes_of(projected_trait, source)?;
        shapes.push(TypeShape::from_node(target, source)?);
        Some(Self {
            associated_name: source
                .get(projection.child_by_field_name("name")?.byte_range())?
                .to_owned(),
            trait_position: source_position_of(terminal_name_of(projected_trait)?, source)?,
            target_name: terminal_name_text_of(target, source)?,
            parameters,
            shapes,
        })
    }

    /// 投影の末尾の関連型の名前（`Item`）。
    pub(crate) fn associated_name(&self) -> &str {
        &self.associated_name
    }

    /// trait パスの末尾の名前。implementation と definition をここへ尋ねる。
    pub(crate) fn trait_position(&self) -> SourcePosition {
        self.trait_position
    }
}

/// implementation が返した impl 1 つの、照合に使う構文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustImplCandidate {
    target_position: SourcePosition,
    target_name: String,
    parameters: Vec<String>,
    shapes: Vec<TypeShape>,
}

impl RustImplCandidate {
    /// 1 つのファイルにある trait impl のうち、照合できるものを読む。
    ///
    /// **選べない impl は入れない。** 型パラメータの境界・where 句・const 引数・負の impl・
    /// 属性（`cfg` など）・`default impl`・対象が型名で終わらない（blanket impl の `T` / 参照）・
    /// 構文エラーのどれかがある impl。
    /// 境界の充足を確かめずに選ぶと、境界を満たさない使用側にも RHS を当てはめてしまう。
    pub(crate) fn candidates_of(source: &str) -> Vec<Self> {
        let Ok(tree) = SyntaxTree::from_source(source, Grammar::Rust) else {
            return Vec::new();
        };
        tree.named_descendants()
            .into_iter()
            .filter(|node| node.kind() == "impl_item")
            .filter_map(|implementation| Self::from_node(implementation, source))
            .collect()
    }

    fn from_node(implementation: Node<'_>, source: &str) -> Option<Self> {
        if signature_has_error(implementation) {
            return None;
        }
        let mut cursor = implementation.walk();
        let negative = implementation
            .children(&mut cursor)
            .any(|child| child.kind() == "!");
        let constrained = where_predicate_lefts_of(implementation).next().is_some()
            || implementation
                .child_by_field_name("type_parameters")
                .is_some_and(|parameters| {
                    named_children_of(parameters)
                        .any(|parameter| parameter.child_by_field_name("bounds").is_some())
                });
        // Why: rust-analyzer は cfg(test) も有効にして解析するので、属性付きの impl は
        // 実際のビルドに無い候補でありうる。`default impl` は specialization で上書きされうる。
        // tree-sitter-rust は `default` を impl の外（直前の兄弟）に置く。
        let specialized = implementation
            .prev_sibling()
            .and_then(|previous| source.get(previous.byte_range()))
            .is_some_and(|text| text.trim_end().ends_with("default"));
        let conditional = has_attribute(implementation) || specialized;
        if negative || constrained || conditional {
            return None;
        }
        let implemented = implementation.child_by_field_name("trait")?;
        let target = implementation.child_by_field_name("type")?;
        let parameters = type_parameter_names_of(implementation, source)?;
        let target_name = terminal_name_text_of(target, source)?;
        let blanket = parameters.contains(&target_name) && target.kind() == "type_identifier";
        if blanket {
            return None;
        }
        let mut shapes = trait_argument_shapes_of(implemented, source)?;
        shapes.push(TypeShape::from_node(target, source)?);
        Some(Self {
            target_position: source_position_of(target, source)?,
            target_name,
            parameters,
            shapes,
        })
    }

    /// implementation が返す位置（対象型の始まり）。
    pub(crate) fn target_position(&self) -> SourcePosition {
        self.target_position
    }

    /// 対象型の末尾の名前が使用側と同じか。違えば LSP に尋ねずに飛ばしてよい。
    ///
    /// **別名（`use Holder as H`）で書かれた本物の候補も飛ばす。** 選べずに
    /// `UnresolvedAssociatedType` へ倒れるだけで、別の impl を選ぶことはない
    /// （coherence により、本物と重なる候補は他に無い）。
    pub(crate) fn may_match(&self, projection: &RustQualifiedProjection) -> bool {
        self.target_name == projection.target_name
    }

    /// 型変数の付け替えだけで使用側と一致するかを構文で確かめ、束縛と型名の組を返す。
    ///
    /// 一致しなければ `None`。候補の型変数が具体的な型・メソッドの型変数に当たる、
    /// 束縛が食い違う、束縛されない型変数が残る、構文の形が違う、のどれか。
    /// **型名の綴りは比べない** — 宣言元は呼び出し側が組ごとに照合する。
    pub(crate) fn binding_with(
        &self,
        projection: &RustQualifiedProjection,
    ) -> Option<RustImplBinding> {
        if self.shapes.len() != projection.shapes.len() {
            return None;
        }
        let mut walk = BindingWalk {
            candidate_parameters: &self.parameters,
            use_parameters: &projection.parameters,
            bound: vec![None; self.parameters.len()],
            names: Vec::new(),
        };
        for (candidate, used) in self.shapes.iter().zip(&projection.shapes) {
            walk.walk(candidate, used)?;
        }
        let captures = walk.bound.into_iter().collect::<Option<Vec<_>>>()?;
        Some(RustImplBinding {
            captures,
            names: walk.names,
        })
    }
}

/// 候補 impl と使用側の対応。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustImplBinding {
    captures: Vec<String>,
    names: Vec<(TypeReference, TypeReference)>,
}

impl RustImplBinding {
    /// 候補 impl の型変数を宣言順に並べたときの、束縛先の使用側の型変数名。
    pub(crate) fn captures(&self) -> &[String] {
        &self.captures
    }

    /// 同じ位置にあった型名の組（候補側, 使用側）。どちらも自分のソースの問い合わせ位置を持つ。
    pub(crate) fn names(&self) -> &[(TypeReference, TypeReference)] {
        &self.names
    }
}

/// 型の構文を、照合に要る形だけ持ち出したもの。
///
/// **型名はパスごと 1 つの葉にする。** `super::Holder` と `Holder` は綴りが違っても
/// 同じ宣言を指しうるので、形ではなく宣言元で比べる。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeShape {
    /// 型名（パスを含む）・プリミティブ。宣言元で比べる。
    Name(TypeReference),
    /// ライフタイム。`'static` だけを区別する。
    Lifetime { is_static: bool },
    /// 子を持たない、その他の字句（`mut` など）。綴りで比べる。
    Token { kind: String, text: String },
    /// 子を持つ構文。種別と子の並びで比べる。
    Node {
        kind: String,
        children: Vec<TypeShape>,
    },
}

impl TypeShape {
    /// 読めない構文（`Self`・関連型の束縛・マクロ・エラー）を含めば `None`。
    fn from_node(node: Node<'_>, source: &str) -> Option<Self> {
        let text = source.get(node.byte_range())?;
        match node.kind() {
            "type_identifier" | "scoped_type_identifier" if text == SELF_TYPE => None,
            "type_identifier" | "primitive_type" => Some(Self::Name(TypeReference::new(
                text.to_owned(),
                source_position_of(node, source)?,
            ))),
            "scoped_type_identifier" => {
                let path = node.child_by_field_name("path")?;
                let simple_path = matches!(
                    path.kind(),
                    "identifier" | "scoped_identifier" | "super" | "crate" | "self"
                );
                if !simple_path {
                    return None;
                }
                Some(Self::Name(TypeReference::new(
                    collapsed(text),
                    source_position_of(node.child_by_field_name("name")?, source)?,
                )))
            }
            "lifetime" => Some(Self::Lifetime {
                is_static: text == STATIC_LIFETIME,
            }),
            "type_binding" | "macro_invocation" | "ERROR" | "qualified_type" | "bracketed_type" => {
                None
            }
            _ if node.named_child_count() == 0 => Some(Self::Token {
                kind: node.kind().to_owned(),
                text: text.to_owned(),
            }),
            _ => Some(Self::Node {
                kind: node.kind().to_owned(),
                children: named_children_of(node)
                    .filter(|child| !COMMENT_KINDS.contains(&child.kind()))
                    .map(|child| Self::from_node(child, source))
                    .collect::<Option<_>>()?,
            }),
        }
    }
}

struct BindingWalk<'a> {
    candidate_parameters: &'a [String],
    use_parameters: &'a [String],
    bound: Vec<Option<String>>,
    names: Vec<(TypeReference, TypeReference)>,
}

impl BindingWalk<'_> {
    fn walk(&mut self, candidate: &TypeShape, used: &TypeShape) -> Option<()> {
        let used_parameter = match used {
            TypeShape::Name(name) => self
                .use_parameters
                .iter()
                .any(|parameter| parameter == name.name())
                .then(|| name.name().to_owned()),
            _ => None,
        };
        if let TypeShape::Name(name) = candidate {
            if let Some(index) = self
                .candidate_parameters
                .iter()
                .position(|parameter| parameter == name.name())
            {
                // 具体的な型に当たる束縛は #318 の対象。ここでは使用側の型変数だけを受ける。
                let used_parameter = used_parameter?;
                return match &self.bound[index] {
                    Some(bound) if *bound != used_parameter => None,
                    _ => {
                        self.bound[index] = Some(used_parameter);
                        Some(())
                    }
                };
            }
        }
        if used_parameter.is_some() {
            return None;
        }
        match (candidate, used) {
            (TypeShape::Name(candidate), TypeShape::Name(used)) => {
                self.names.push((candidate.clone(), used.clone()));
                Some(())
            }
            (
                TypeShape::Lifetime {
                    is_static: candidate,
                },
                TypeShape::Lifetime { is_static: used },
            ) => (candidate == used).then_some(()),
            (TypeShape::Token { .. }, TypeShape::Token { .. }) => (candidate == used).then_some(()),
            (
                TypeShape::Node {
                    kind: candidate_kind,
                    children: candidate_children,
                },
                TypeShape::Node {
                    kind: used_kind,
                    children: used_children,
                },
            ) => {
                let same_shape =
                    candidate_kind == used_kind && candidate_children.len() == used_children.len();
                if !same_shape {
                    return None;
                }
                for (candidate, used) in candidate_children.iter().zip(used_children) {
                    self.walk(candidate, used)?;
                }
                Some(())
            }
            _ => None,
        }
    }
}

/// trait パスの型引数の形。型引数が無ければ空。
fn trait_argument_shapes_of(implemented: Node<'_>, source: &str) -> Option<Vec<TypeShape>> {
    if implemented.kind() != "generic_type" {
        return Some(Vec::new());
    }
    let arguments = implemented.child_by_field_name("type_arguments")?;
    named_children_of(arguments)
        .filter(|child| !COMMENT_KINDS.contains(&child.kind()))
        .map(|argument| TypeShape::from_node(argument, source))
        .collect()
}

/// impl の型パラメータ名。ライフタイムは除き、const 引数があれば `None`。
fn type_parameter_names_of(implementation: Node<'_>, source: &str) -> Option<Vec<String>> {
    let Some(parameters) = implementation.child_by_field_name("type_parameters") else {
        return Some(Vec::new());
    };
    named_children_of(parameters)
        .filter(|parameter| parameter.kind() != "lifetime_parameter")
        .map(|parameter| {
            if parameter.kind() != "type_parameter" {
                return None;
            }
            Some(
                source
                    .get(parameter.child_by_field_name("name")?.byte_range())?
                    .to_owned(),
            )
        })
        .collect()
}

/// 本体を除いた where 句の述語の左辺。
fn where_predicate_lefts_of(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_children_of(node)
        .filter(|child| child.kind() == "where_clause")
        .flat_map(named_children_of)
        .filter_map(|predicate| predicate.child_by_field_name("left"))
}

/// 対象型の末尾の名前の綴り。型名で終わらない（参照・タプルなど）なら `None`。
fn terminal_name_text_of(target: Node<'_>, source: &str) -> Option<String> {
    Some(
        source
            .get(terminal_name_of(target)?.byte_range())?
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使用側のソースの、最初の完全修飾形の投影を読む。
    fn qualified_of(source: &str) -> Option<RustQualifiedProjection> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let reference = type_references_of(function, source)
            .into_iter()
            .find(|reference| reference.name().starts_with("<Self"))
            .expect("投影を尋ねる");
        RustQualifiedProjection::from_source(source, reference.position())
    }

    fn only_candidate_of(source: &str) -> Option<RustImplCandidate> {
        let mut candidates = RustImplCandidate::candidates_of(source);
        assert!(candidates.len() <= 1, "{source}");
        candidates.pop()
    }

    #[test]
    fn test_binding_maps_candidate_variables_to_use_site_impl_variables() {
        let used =
            qualified_of("impl<A, B> Pair<A, B> { fn f(x: <Self as Other<B, u8>>::Item) {} }")
                .unwrap();
        let candidate =
            only_candidate_of("impl<Y, X> Other<Y, u8> for Pair<X, Y> { type Item = X; }").unwrap();
        let binding = candidate.binding_with(&used).expect("付け替えで一致する");
        assert_eq!(binding.captures(), ["B", "A"]);
        let names: Vec<_> = binding
            .names()
            .iter()
            .map(|(candidate, used)| (candidate.name(), used.name()))
            .collect();
        assert_eq!(names, [("u8", "u8"), ("Pair", "Pair")]);
    }

    #[test]
    fn test_binding_accepts_two_candidate_variables_bound_to_the_same_use_site_variable() {
        let used =
            qualified_of("impl<A> Pair<A, A> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        let candidate =
            only_candidate_of("impl<X, Y> Other for Pair<X, Y> { type Item = X; }").unwrap();
        assert_eq!(
            candidate
                .binding_with(&used)
                .expect("一般化した候補も当てはまる")
                .captures(),
            ["A", "A"]
        );
    }

    #[test]
    fn test_binding_rejects_candidate_variables_bound_to_concrete_types() {
        let candidate =
            only_candidate_of("impl<X> Other for Holder<X> { type Item = X; }").unwrap();
        let concrete = qualified_of("impl Holder<u8> { fn f(x: <Self as Other>::Item) {} }")
            .expect("使用側は読める");
        assert_eq!(candidate.binding_with(&concrete), None);
        let generic = qualified_of("impl<T> Holder<T> { fn f(x: <Self as Other>::Item) {} }")
            .expect("使用側は読める");
        assert!(candidate.binding_with(&generic).is_some(), "対照");
    }

    #[test]
    fn test_binding_rejects_inconsistent_variables_and_concrete_candidates_for_variables() {
        let used =
            qualified_of("impl<A, B> Pair<A, B> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        for candidate in [
            "impl<X> Other for Pair<X, X> { type Item = X; }",
            "impl<X> Other for Pair<X, u8> { type Item = X; }",
            "impl<X, Y> Other for Pair<X, (Y,)> { type Item = X; }",
        ] {
            let candidate = only_candidate_of(candidate).unwrap();
            assert_eq!(candidate.binding_with(&used), None, "{candidate:?}");
        }
        let matching =
            only_candidate_of("impl<X, Y> Other for Pair<X, Y> { type Item = X; }").unwrap();
        assert!(matching.binding_with(&used).is_some(), "対照");
    }

    #[test]
    fn test_binding_rejects_method_variables_in_trait_arguments() {
        let used =
            qualified_of("impl<T> Holder<T> { fn f<U>(x: <Self as Other<U>>::Item) {} }").unwrap();
        let candidate =
            only_candidate_of("impl<A, X> Other<A> for Holder<X> { type Item = A; }").unwrap();
        assert_eq!(candidate.binding_with(&used), None);
    }

    #[test]
    fn test_binding_compares_written_paths_as_name_pairs() {
        let used =
            qualified_of("impl<T> Holder<T> { fn f(x: <Self as Other<model::Amount>>::Item) {} }")
                .unwrap();
        let candidate =
            only_candidate_of("impl<X> Other<Amount> for super::Holder<X> { type Item = X; }")
                .unwrap();
        let binding = candidate.binding_with(&used).unwrap();
        let names: Vec<_> = binding
            .names()
            .iter()
            .map(|(candidate, used)| (candidate.name(), used.name()))
            .collect();
        assert_eq!(
            names,
            [("Amount", "model::Amount"), ("super::Holder", "Holder")]
        );
    }

    #[test]
    fn test_qualified_projection_rejects_forms_that_impl_selection_does_not_decide() {
        for source in [
            "impl<T> Holder<T> { fn f(x: Self::Item) {} }",
            "impl<T> Holder<T> { fn f(x: <<Self as Other>::Item as Next>::Out) {} }",
            "impl<T> Holder<T> { fn f(x: <Self as Other>::Item) where Self: Other {} }",
            "impl<T> Holder<T> where Holder<T>: Other { fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Holder<T> where crate::Holder<T>: Other { fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Holder<T> { fn f(x: <Self as Other>::Item) where Holder<T,>: Other {} }",
            "impl<T> Other for Holder<T> { type Item = T; fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Holder<T> { fn f(x: <Self as Other<Self>>::Item) {} }",
            "impl<T> Holder<T> { fn f(x: <Self as Other<Item = u8>>::Item) {} }",
        ] {
            let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
            let function = tree
                .named_descendants()
                .into_iter()
                .find(|node| node.kind() == "function_item")
                .unwrap();
            let reference = type_references_of(function, source)
                .into_iter()
                .find(|reference| reference.name().contains("Self"))
                .unwrap_or_else(|| panic!("投影を尋ねる: {source}"));
            assert_eq!(
                RustQualifiedProjection::from_source(source, reference.position()),
                None,
                "{source}"
            );
        }
        assert!(
            qualified_of("impl<T> Show for Holder<T> { fn f(x: <Self as Other>::Item) {} }")
                .is_some(),
            "対照: 別の trait の impl からは選ぶ"
        );
        assert!(
            qualified_of(
                "impl<T> Holder<T> where T: Clone { fn f<U>(x: <Self as Other>::Item) where U: Copy {} }"
            )
            .is_some(),
            "対照: 型変数そのものへの境界は param-env の候補にならない"
        );
    }

    #[test]
    fn test_candidates_skip_impls_whose_application_needs_bounds_or_is_not_a_named_target() {
        let source = "impl<T: Clone> Other for Holder<T> { type Item = T; }
impl<T> Other for Pair<T> where T: Copy { type Item = T; }
impl<T> Other for T { type Item = T; }
impl<T> Other for &Holder<T> { type Item = T; }
impl !Other for Plain {}
impl<T, const N: usize> Other for Arr<T> { type Item = T; }
#[cfg(test)]
impl<T> Other for Tested<T> { type Item = T; }
default impl<T> Other for Special<T> { type Item = T; }
impl<T> Other for Single<T> { type Item = T; }";
        let candidates = RustImplCandidate::candidates_of(source);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.target_name.as_str())
                .collect::<Vec<_>>(),
            ["Single"]
        );
        let line = source.lines().count() - 1;
        assert_eq!(
            candidates[0].target_position(),
            SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::from_index(line),
                "impl<T> Other for "
            )
        );
    }

    #[test]
    fn test_associated_definition_of_a_selected_impl_takes_use_site_captures() {
        let source = "impl<Y, X> Other<Y> for Pair<X, Y> { type Item = (X, Y); }";
        let candidate = only_candidate_of(source).unwrap();
        let definition =
            RustAssociatedDefinition::from_impl_at(source, candidate.target_position(), "Item")
                .unwrap()
                .with_captures(vec!["B".to_owned(), "A".to_owned()])
                .unwrap();
        let resolution = definition.resolution_with(&|_| None).unwrap();
        let header = Some("impl<A, B> Pair<A, B>");
        let selected = RustCallable::from_spelling(
            "fn f(x: <Self as Other<B>>::Item)",
            &|name| name.contains("Other").then(|| resolution.clone()),
            header,
        )
        .unwrap();
        let explicit = RustCallable::from_spelling("fn f(x: (A, B))", &|_| None, header).unwrap();
        assert_eq!(selected, explicit);
        for missing in [
            "impl<X> Other for Pair<X> { type Other = X; }",
            "impl<X> Other for Pair<X> { default type Item = X; }",
            "impl<X> Other for Pair<X> { #[cfg(test)] type Item = X; }",
        ] {
            let position = SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::from_index(0),
                "impl<X> Other for ",
            );
            assert!(
                RustAssociatedDefinition::from_impl_at(missing, position, "Item").is_none(),
                "{missing}"
            );
        }
    }
}
