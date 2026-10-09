//! Self を直接の対象にした完全修飾形の関連型について、別の impl を選ぶための構文情報。
//!
//! **選ぶのは、候補 impl の型変数に使用側の型を当てはめるだけで、trait 引数と対象型が
//! 一致し、境界も where 句も持たない impl だけ。** 当てはめる型は使用側の impl の型変数か、
//! 具体的な型（型変数を含む部分型を含む）。coherence により、そうした impl があれば
//! 他の impl は重ならないので、残りの候補を数えずに 1 つへ決められる。
//! 境界の無い blanket impl（`impl<T> Trait for T`）も同じ理由で選ぶ。
//!
//! **境界や where 句を持つ候補は選ばない（偽陰性側）。** 境界を満たさない型には別の impl が
//! 共存できるので、coherence だけでは 1 つに決まらず、境界の充足を確かめる trait 解決が要る。
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
    method_parameters: Vec<String>,
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
        // Why: 対象が型変数そのもの（`impl<T: Other> Show for T`）なら、その型変数への境界は
        // Self への param-env の候補で、supertrait 経由でも投影の trait を含みうる。
        // rustc は impl より param-env を優先して正規化しないので、境界の名前では絞らない。
        if let Some(variable) = variable_target_of(target, &parameters, source) {
            let inline_bound = implementation
                .child_by_field_name("type_parameters")
                .is_some_and(|declared| {
                    named_children_of(declared).any(|parameter| {
                        parameter.child_by_field_name("bounds").is_some()
                            && parameter
                                .child_by_field_name("name")
                                .and_then(|name| source.get(name.byte_range()))
                                == Some(variable)
                    })
                });
            let where_bound = [function, implementation]
                .into_iter()
                .flat_map(where_predicate_lefts_of)
                .any(|left| source.get(left.byte_range()) == Some(variable));
            if inline_bound || where_bound {
                return None;
            }
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
            method_parameters,
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
    /// 対象が型変数そのもの（`impl<T> Trait for T`）か。
    blanket: bool,
    parameters: Vec<String>,
    shapes: Vec<TypeShape>,
}

impl RustImplCandidate {
    /// 1 つのファイルにある trait impl のうち、照合できるものを読む。
    ///
    /// **選べない impl は入れない。** 型パラメータの境界・where 句・const 引数・負の impl・
    /// 属性（`cfg` など）・`default impl`・対象が型名で終わらない（参照・タプル）・
    /// 構文エラーのどれかがある impl。境界の無い blanket impl は入れる。
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
        let mut shapes = trait_argument_shapes_of(implemented, source)?;
        shapes.push(TypeShape::from_node(target, source)?);
        Some(Self {
            target_position: source_position_of(target, source)?,
            target_name,
            blanket,
            parameters,
            shapes,
        })
    }

    /// implementation が返す位置（対象型の始まり）。
    pub(crate) fn target_position(&self) -> SourcePosition {
        self.target_position
    }

    /// 対象型の末尾の名前が使用側と同じか、対象が型変数そのものか。
    /// どちらでもなければ LSP に尋ねずに飛ばしてよい。
    ///
    /// **別名（`use Holder as H`）で書かれた本物の候補も飛ばす。** 型変数だけの当てはめなら
    /// 選べずに `UnresolvedAssociatedType` へ倒れるだけ（coherence により、本物と重なる候補は
    /// 他に無い）。具体的な型を当てはめるときは、unsized な型に共存する別名の impl を
    /// 飛ばして別の impl を選びうる緩みが残る（`rules/architecture.md`）。
    pub(crate) fn may_match(&self, projection: &RustQualifiedProjection) -> bool {
        self.blanket || self.target_name == projection.target_name
    }

    /// 候補の型変数に使用側の型を当てはめるだけで一致するかを構文で確かめ、
    /// 束縛と型名の組を返す。
    ///
    /// 一致しなければ `None`。束縛が食い違う、束縛されない型変数が残る、構文の形が違う、
    /// 候補の型変数がライフタイム・メソッドの型変数を含む型・型変数を先頭に持つパスに当たる、
    /// のどれか。**同じ候補の型変数が 2 回以上現れ、どちらかが具体的な型に当たる**ときも
    /// 一致にしない — 2 つが同じ型かを確かめるには使用側どうしの宣言元の照合が要り、
    /// 綴りで比べると別モジュールの同名の型を取り違える（偽陰性側）。
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
            method_parameters: &projection.method_parameters,
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
            use_parameters: projection.parameters.clone(),
        })
    }
}

/// implementation が返した場所にある impl の対象型。候補として読めない impl も含む。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustImplTarget {
    position: SourcePosition,
    /// 対象型の末尾の名前。型名で終わらない（参照・タプルなど）なら `None`。
    name: Option<String>,
    /// 対象が型変数そのもの（blanket impl）か。
    blanket: bool,
}

impl RustImplTarget {
    /// 1 つのファイルにある impl の対象型。境界・属性・構文エラーの有無を問わない。
    pub(crate) fn targets_of(source: &str) -> Vec<Self> {
        let Ok(tree) = SyntaxTree::from_source(source, Grammar::Rust) else {
            return Vec::new();
        };
        tree.named_descendants()
            .into_iter()
            .filter(|node| node.kind() == "impl_item")
            .filter_map(|implementation| {
                let target = implementation.child_by_field_name("type")?;
                let declared: Vec<String> = implementation
                    .child_by_field_name("type_parameters")
                    .into_iter()
                    .flat_map(named_children_of)
                    .filter_map(|parameter| {
                        let name = parameter.child_by_field_name("name")?;
                        Some(source.get(name.byte_range())?.to_owned())
                    })
                    .collect();
                Some(Self {
                    position: source_position_of(target, source)?,
                    name: terminal_name_text_of(target, source),
                    blanket: variable_target_of(target, &declared, source).is_some(),
                })
            })
            .collect()
    }

    /// implementation が返す位置（対象型の始まり）。
    pub(crate) fn position(&self) -> SourcePosition {
        self.position
    }

    /// 使用側の Self に当てはまりうるか。対象型の末尾の名前が同じ・blanket impl・
    /// 型名で終わらない対象のどれか（名前で言い切れないものは当てはまりうる側へ倒す）。
    pub(crate) fn may_apply_to(&self, projection: &RustQualifiedProjection) -> bool {
        self.blanket
            || self
                .name
                .as_ref()
                .is_none_or(|name| *name == projection.target_name)
    }
}

/// 候補 impl と使用側の対応。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustImplBinding {
    captures: Vec<BoundType>,
    names: Vec<(TypeReference, TypeReference)>,
    use_parameters: Vec<String>,
}

/// 候補 impl の型変数 1 つに当たった、使用側の型。
#[derive(Debug, Clone, PartialEq, Eq)]
enum BoundType {
    /// 使用側の impl の型変数。
    Variable(String),
    /// 具体的な型。使用側のソースの綴りと、その中の型名（使用側の impl の型変数を除く）。
    Concrete {
        text: String,
        references: Vec<TypeReference>,
    },
}

impl RustImplBinding {
    /// 候補の型変数のどれかが、使用側の型変数でない具体的な型に当たったか。
    pub(crate) fn binds_concrete_types(&self) -> bool {
        self.captures
            .iter()
            .any(|bound| matches!(bound, BoundType::Concrete { .. }))
    }

    /// 具体的な型に書かれた型名。どれも使用側のソースの問い合わせ位置を持つ。
    pub(crate) fn capture_references(&self) -> impl Iterator<Item = &TypeReference> {
        self.captures.iter().flat_map(|bound| match bound {
            BoundType::Variable(_) => [].iter(),
            BoundType::Concrete { references, .. } => references.iter(),
        })
    }

    /// 候補 impl の型変数を宣言順に並べたときの、代入するもの。
    /// 具体的な型の型名は `type_of`（使用側の位置から辿った解決）で差し込む。
    ///
    /// 具体的な型をテンプレートにできなければ `None`（辿れない型名・ライフタイム・未知の構文）。
    pub(crate) fn captures_with(
        &self,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Option<Vec<RustCapture>> {
        self.captures
            .iter()
            .map(|bound| match bound {
                BoundType::Variable(name) => Some(RustCapture::Variable(name.clone())),
                BoundType::Concrete { text, .. } => {
                    RustGenericAlias::captured_of(&self.use_parameters, text, type_of)
                        .map(RustCapture::Concrete)
                }
            })
            .collect()
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
    /// 子を持つ構文。種別と子の並びで比べる。`text` は具体的な型として代入するときの綴り。
    Node {
        kind: String,
        text: String,
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
                text: text.to_owned(),
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
    method_parameters: &'a [String],
    bound: Vec<Option<BoundType>>,
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
                let bound = match used_parameter {
                    Some(parameter) => BoundType::Variable(parameter),
                    None => self.concrete_of(used)?,
                };
                return match &self.bound[index] {
                    None => {
                        self.bound[index] = Some(bound);
                        Some(())
                    }
                    Some(BoundType::Variable(previous))
                        if bound == BoundType::Variable(previous.clone()) =>
                    {
                        Some(())
                    }
                    Some(_) => None,
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
                    ..
                },
                TypeShape::Node {
                    kind: used_kind,
                    children: used_children,
                    ..
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

impl BindingWalk<'_> {
    /// 候補の型変数に当たった、使用側の型変数でない型。
    ///
    /// ライフタイム、メソッドの型変数を含む型（使用側の impl のスコープでは綴れない）、
    /// 型変数を先頭に持つパス（`T::Out`。関連型の投影で、型名として辿れない）は `None`。
    fn concrete_of(&self, used: &TypeShape) -> Option<BoundType> {
        let text = match used {
            TypeShape::Name(name) => name.name().to_owned(),
            TypeShape::Token { text, .. } | TypeShape::Node { text, .. } => text.clone(),
            TypeShape::Lifetime { .. } => return None,
        };
        let mut references = Vec::new();
        self.concrete_references_of(used, &mut references)?;
        Some(BoundType::Concrete { text, references })
    }

    fn concrete_references_of(
        &self,
        used: &TypeShape,
        references: &mut Vec<TypeReference>,
    ) -> Option<()> {
        match used {
            TypeShape::Name(name) => {
                let head = name.name().split("::").next().unwrap_or_default();
                let is_variable = |parameter: &String| parameter == head;
                if self.method_parameters.iter().any(is_variable) {
                    return None;
                }
                if self.use_parameters.iter().any(is_variable) {
                    // 型変数そのものはテンプレートの引数になる。先頭だけが型変数なら投影
                    return (name.name() == head).then_some(());
                }
                references.push(name.clone());
                Some(())
            }
            TypeShape::Node { children, .. } => children
                .iter()
                .try_for_each(|child| self.concrete_references_of(child, references)),
            TypeShape::Token { .. } | TypeShape::Lifetime { .. } => Some(()),
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

/// 対象が impl の型変数そのものなら、その名前。
fn variable_target_of<'source>(
    target: Node<'_>,
    parameters: &[String],
    source: &'source str,
) -> Option<&'source str> {
    if target.kind() != "type_identifier" {
        return None;
    }
    source
        .get(target.byte_range())
        .filter(|name| parameters.iter().any(|parameter| parameter == name))
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
        assert_eq!(
            binding.captures_with(&|_| None),
            Some(variables(&["B", "A"]))
        );
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
                .captures_with(&|_| None),
            Some(variables(&["A", "A"]))
        );
    }

    fn variables(names: &[&str]) -> Vec<RustCapture> {
        names
            .iter()
            .map(|name| RustCapture::Variable((*name).to_owned()))
            .collect()
    }

    /// 選んだ impl の `Item` を、束縛した captures で展開した使用側のシグネチャ。
    ///
    /// 型名は `resolved` で辿った扱いにする（使用側・候補側で同じ解決を返す）。
    fn expanded_with(
        used_source: &str,
        candidate_source: &str,
        hover: &str,
        resolved: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Option<RustCallable> {
        let used = qualified_of(used_source).expect("使用側は読める");
        let candidate = only_candidate_of(candidate_source).expect("候補を読める");
        assert!(candidate.may_match(&used), "{candidate_source}");
        let binding = candidate.binding_with(&used)?;
        let captures = binding.captures_with(resolved)?;
        let definition = RustAssociatedDefinition::from_impl_at(
            candidate_source,
            candidate.target_position(),
            "Item",
        )?
        .with_captures(captures)?;
        let resolution = definition.resolution_with(resolved)?;
        let header = used_source.split_once(" {").map(|(header, _)| header);
        RustCallable::from_spelling(
            hover,
            &|name| match name.contains("Other") {
                true => Some(resolution.clone()),
                false => resolved(name),
            },
            header,
        )
    }

    fn declared_or_primitive(name: &str) -> Option<RustTypeResolution> {
        match name {
            "u8" | "u16" => Some(RustTypeResolution::Opened(name.to_owned())),
            "Vec" | "Holder" | "model::Amount" => {
                Some(RustTypeResolution::Declared(format!("@{name}")))
            }
            _ => None,
        }
    }

    #[test]
    fn test_binding_substitutes_concrete_use_site_types_for_candidate_variables() {
        for (used, explicit) in [
            (
                "impl Holder<u8> { fn f(x: <Self as Other>::Item) {} }",
                "fn f(x: (u8, u16))",
            ),
            (
                "impl<T> Holder<Vec<T>> { fn f(x: <Self as Other>::Item) {} }",
                "fn f(x: (Vec<T>, u16))",
            ),
            (
                "impl Holder<model::Amount> { fn f(x: <Self as Other>::Item) {} }",
                "fn f(x: (model::Amount, u16))",
            ),
            (
                "impl Holder<()> { fn f(x: <Self as Other>::Item) {} }",
                "fn f(x: ((), u16))",
            ),
        ] {
            let selected = expanded_with(
                used,
                "impl<X> Other for Holder<X> { type Item = (X, u16); }",
                "fn f(x: <Self as Other>::Item)",
                &declared_or_primitive,
            )
            .unwrap_or_else(|| panic!("具体的な型を代入して展開する: {used}"));
            let header = used.split_once(" {").map(|(header, _)| header);
            let explicit =
                RustCallable::from_spelling(explicit, &declared_or_primitive, header).unwrap();
            assert_eq!(selected, explicit, "{used}");
        }
    }

    #[test]
    fn test_binding_substitutes_the_whole_target_for_an_unbounded_blanket_impl() {
        let selected = expanded_with(
            "impl Holder<u8> { fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Other for T { type Item = (T,); }",
            "fn f(x: <Self as Other>::Item)",
            &declared_or_primitive,
        )
        .expect("Self 全体を代入して展開する");
        let explicit = RustCallable::from_spelling(
            "fn f(x: (Holder<u8>,))",
            &declared_or_primitive,
            Some("impl Holder<u8>"),
        )
        .unwrap();
        assert_eq!(selected, explicit);
    }

    #[test]
    fn test_binding_captures_without_traced_concrete_names_are_not_built() {
        let used =
            qualified_of("impl Holder<Amount> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        let candidate =
            only_candidate_of("impl<X> Other for Holder<X> { type Item = X; }").unwrap();
        let binding = candidate.binding_with(&used).unwrap();
        assert_eq!(
            binding
                .capture_references()
                .map(TypeReference::name)
                .collect::<Vec<_>>(),
            ["Amount"]
        );
        assert_eq!(binding.captures_with(&|_| None), None);
        assert!(
            binding
                .captures_with(&|_| Some(RustTypeResolution::Declared("@amount".to_owned())))
                .is_some(),
            "対照: 辿れていれば組み立てる"
        );
    }

    #[test]
    fn test_binding_rejects_concrete_types_that_the_use_site_impl_cannot_spell() {
        let candidate =
            only_candidate_of("impl<A, X> Other<A> for Holder<X> { type Item = A; }").unwrap();
        for used in [
            // メソッドの型変数を含む
            "impl<T> Holder<T> { fn f<U>(x: <Self as Other<Vec<U>>>::Item) {} }",
            // 型変数を先頭に持つパス（関連型の投影）
            "impl<T> Holder<T> { fn f(x: <Self as Other<T::Out>>::Item) {} }",
        ] {
            let used = qualified_of(used).unwrap_or_else(|| panic!("使用側は読める: {used}"));
            assert_eq!(candidate.binding_with(&used), None, "{used:?}");
        }
        let spelled =
            qualified_of("impl<T> Holder<T> { fn f(x: <Self as Other<Vec<T>>>::Item) {} }")
                .unwrap();
        assert!(candidate.binding_with(&spelled).is_some(), "対照");
    }

    #[test]
    fn test_binding_rejects_a_candidate_variable_bound_twice_to_concrete_types() {
        let candidate =
            only_candidate_of("impl<X> Other for Pair<X, X> { type Item = X; }").unwrap();
        for used in [
            "impl Pair<u8, u8> { fn f(x: <Self as Other>::Item) {} }",
            "impl<A> Pair<A, u8> { fn f(x: <Self as Other>::Item) {} }",
            "impl<A> Pair<u8, A> { fn f(x: <Self as Other>::Item) {} }",
        ] {
            let used = qualified_of(used).unwrap();
            assert_eq!(candidate.binding_with(&used), None, "{used:?}");
        }
        let variables =
            qualified_of("impl<A> Pair<A, A> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        assert!(candidate.binding_with(&variables).is_some(), "対照");
    }

    #[test]
    fn test_binding_captures_with_lifetimes_inside_concrete_types_are_not_built() {
        let used =
            qualified_of("impl<'a> Holder<&'a u8> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        let candidate =
            only_candidate_of("impl<X> Other for Holder<X> { type Item = X; }").unwrap();
        let binding = candidate.binding_with(&used).expect("型としては当てはまる");
        assert!(binding.binds_concrete_types());
        assert_eq!(binding.captures_with(&declared_or_primitive), None);
        let used =
            qualified_of("impl Holder<&'static u8> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        let binding = candidate.binding_with(&used).unwrap();
        assert!(
            binding.captures_with(&declared_or_primitive).is_some(),
            "対照: 'static は残せる"
        );
    }

    #[test]
    fn test_binding_rejects_a_candidate_variable_bound_to_a_lifetime() {
        let candidate =
            only_candidate_of("impl<X> Other for Holder<X> { type Item = X; }").unwrap();
        let used =
            qualified_of("impl<'a> Holder<'a> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        assert_eq!(candidate.binding_with(&used), None);
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
        for source in [
            "impl<T: Other> Show for T { fn f(x: <Self as Other>::Item) {} }",
            "impl<T: Sub> Show for T { fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Show for T where T: Other { fn f(x: <Self as Other>::Item) {} }",
            "impl<T> Show for T { fn f(x: <Self as Other>::Item) where T: Sub {} }",
        ] {
            assert_eq!(
                qualified_of(source),
                None,
                "Self が境界つきの型変数: {source}"
            );
        }
        assert!(
            qualified_of("impl<T> Show for T { fn f(x: <Self as Other>::Item) {} }").is_some(),
            "対照: 境界の無い型変数の Self"
        );
        assert!(
            qualified_of("impl<T: Other> Show for Holder<T> { fn f(x: <Self as Other>::Item) {} }")
                .is_some(),
            "対照: 境界は Self ではなく型引数への候補"
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
    fn test_candidates_skip_bounded_impls_and_targets_other_than_type_names_or_variables() {
        let source = "impl<T: Clone> Other for Holder<T> { type Item = T; }
impl<T> Other for Pair<T> where T: Copy { type Item = T; }
impl<T> Other for T { type Item = T; }
impl<T: Clone> Other for T { type Item = T; }
impl<T> Other for T where T: Copy { type Item = T; }
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
            ["T", "Single"],
            "境界の無い blanket impl は入れ、境界・where 句のある blanket impl は入れない"
        );
        let line = source.lines().count() - 1;
        assert_eq!(
            candidates[1].target_position(),
            SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::from_index(line),
                "impl<T> Other for "
            )
        );
    }

    #[test]
    fn test_impl_targets_may_apply_to_the_use_site_unless_their_target_name_differs() {
        let used = qualified_of("impl Holder<u8> { fn f(x: <Self as Other>::Item) {} }").unwrap();
        let source = "impl<T: Clone> Other for Holder<T> { type Item = T; }
impl<T: Clone> Other for Pair<T> { type Item = T; }
impl<T: Clone> Other for T { type Item = T; }
impl<T: Clone> Other for &Pair<T> { type Item = T; }";
        let applying: Vec<_> = RustImplTarget::targets_of(source)
            .iter()
            .map(|target| target.may_apply_to(&used))
            .collect();
        assert_eq!(applying, [true, false, true, true]);
    }

    #[test]
    fn test_associated_definition_of_a_selected_impl_takes_use_site_captures() {
        let source = "impl<Y, X> Other<Y> for Pair<X, Y> { type Item = (X, Y); }";
        let candidate = only_candidate_of(source).unwrap();
        let definition =
            RustAssociatedDefinition::from_impl_at(source, candidate.target_position(), "Item")
                .unwrap()
                .with_captures(variables(&["B", "A"]))
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
