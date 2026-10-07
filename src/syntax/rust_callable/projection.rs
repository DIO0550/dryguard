//! 直接囲む trait impl の関連型定義を、宣言側から展開するための構文情報。

use super::*;
use crate::source_position::SourcePosition;

/// 直接囲む trait impl の関連型 RHS、束縛変数と宣言側の問い合わせ位置。
pub(crate) struct RustProjectionSource {
    definition: RustAssociatedDefinition,
    implemented_trait: TypeReference,
}

impl RustProjectionSource {
    /// 使用位置と直接の trait impl が対応する関連型定義を読む。
    /// 別 impl の選択（[`super::RustQualifiedProjection`]）、入れ子の投影、
    /// 条件付き・複数の定義は `None`。
    pub(crate) fn from_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let projection = projection_at(&tree, source, position)?;
        let function = ancestor_of_kind(projection, "function_item")?;
        let implementation = enclosing_impl_of(function)?;
        if signature_has_error(implementation) {
            return None;
        }
        let implemented = implementation.child_by_field_name("trait")?;
        let path = projection.child_by_field_name("path")?;
        if !is_direct_projection(path, implemented, source) {
            return None;
        }
        let name = source.get(projection.child_by_field_name("name")?.byte_range())?;
        let definition = RustAssociatedDefinition::from_nodes(implementation, name, source)?;
        let trait_name = terminal_name_of(implemented)?;
        Some(Self {
            definition,
            implemented_trait: TypeReference::new(
                collapsed(source.get(implemented.byte_range())?),
                source_position_of(trait_name, source)?,
            ),
        })
    }

    pub(crate) fn references(&self) -> &[TypeReference] {
        self.definition.references()
    }

    pub(crate) fn implemented_trait(&self) -> &TypeReference {
        &self.implemented_trait
    }

    /// 宣言側で解決した RHS と、使用側で代入する impl 型変数を束ねる。
    pub(crate) fn resolution_with(
        &self,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Option<RustTypeResolution> {
        self.definition.resolution_with(type_of)
    }
}

/// trait impl に書かれた関連型定義 1 つの RHS と、使用側で代入する impl 型変数。
///
/// **captures は使用側の型変数名。** 直接囲む impl では impl 自身の型変数名、
/// 別の impl を選んだときは束縛で写した使用側の名前（[`Self::with_captures`]）。
pub(crate) struct RustAssociatedDefinition {
    declaration: String,
    captures: Vec<String>,
    references: Vec<TypeReference>,
}

impl RustAssociatedDefinition {
    /// 対象型が `target_position` から始まる impl の、`name` の関連型定義を読む。
    /// 見つからない・複数・属性付き・本体に構文エラー（`default type` を含む）は `None`。
    pub(crate) fn from_impl_at(
        source: &str,
        target_position: SourcePosition,
        name: &str,
    ) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let implementation = tree.named_descendants().into_iter().find(|node| {
            node.kind() == "impl_item"
                && node
                    .child_by_field_name("type")
                    .and_then(|target| source_position_of(target, source))
                    == Some(target_position)
        })?;
        if implementation.child_by_field_name("body")?.has_error() {
            return None;
        }
        Self::from_nodes(implementation, name, source)
    }

    fn from_nodes(implementation: Node<'_>, name: &str, source: &str) -> Option<Self> {
        let body = implementation.child_by_field_name("body")?;
        let mut candidates = named_children_of(body).filter(|node| {
            node.kind() == "type_item"
                && node
                    .child_by_field_name("name")
                    .is_some_and(|candidate| source.get(candidate.byte_range()) == Some(name))
        });
        let alias = candidates.next()?;
        let unsupported_definition =
            candidates.next().is_some() || alias.has_error() || has_attribute(alias);
        if unsupported_definition {
            return None;
        }

        let mut spelling = Spelling::new(source);
        let mut parameter_texts = Vec::new();
        if let Some(parameters) = implementation.child_by_field_name("type_parameters") {
            spelling.declared_type_parameters_of(parameters)?;
            parameter_texts.extend(parameter_names_of(parameters, source, false)?);
        }
        let captures = spelling.declared.clone();
        if let Some(parameters) = alias.child_by_field_name("type_parameters") {
            spelling.declared_type_parameters_of(parameters)?;
            parameter_texts.extend(parameter_names_of(parameters, source, true)?);
        }
        let right = alias.child_by_field_name("type")?;
        spelling.spelling_of(right)??;
        let parameters = match parameter_texts.is_empty() {
            true => String::new(),
            false => format!("<{}>", parameter_texts.join(", ")),
        };
        let declaration = format!(
            "type Projection{parameters} = {};",
            source.get(right.byte_range())?
        );
        Some(Self {
            declaration,
            captures,
            references: spelling.references,
        })
    }

    /// impl 型変数の代わりに、使用側の型変数名を宣言順に代入する。
    /// 個数が impl の型変数と揃わなければ `None`。
    pub(crate) fn with_captures(self, captures: Vec<String>) -> Option<Self> {
        (captures.len() == self.captures.len()).then_some(Self { captures, ..self })
    }

    /// RHS に書かれた型名と、その宣言側ソースでの問い合わせ位置。
    pub(crate) fn references(&self) -> &[TypeReference] {
        &self.references
    }

    /// 宣言側で解決した RHS と、使用側で代入する impl 型変数を束ねる。
    pub(crate) fn resolution_with(
        &self,
        type_of: &dyn Fn(&str) -> Option<RustTypeResolution>,
    ) -> Option<RustTypeResolution> {
        let tree = SyntaxTree::from_source(&self.declaration, Grammar::Rust).ok()?;
        if tree.has_error() {
            return None;
        }
        let alias = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "type_item")?;
        let Some(parameters) = alias.child_by_field_name("type_parameters") else {
            let resolution = RustTypeResolution::from_spelling_with(&self.declaration, type_of);
            let RustTypeResolution::Opened(right) = resolution else {
                return None;
            };
            return Some(RustTypeResolution::Associated {
                alias: RustGenericAlias {
                    parameters: Vec::new(),
                    right,
                },
                captures: Vec::new(),
            });
        };
        let right = alias.child_by_field_name("type")?;
        let alias =
            RustGenericAlias::from_nodes_with(parameters, right, &self.declaration, type_of)?;
        Some(RustTypeResolution::Associated {
            alias,
            captures: self.captures.clone(),
        })
    }
}

/// `position` に名前を持つ投影（`scoped_type_identifier`）。
pub(super) fn projection_at<'tree>(
    tree: &'tree SyntaxTree<'_>,
    source: &str,
    position: SourcePosition,
) -> Option<Node<'tree>> {
    tree.named_descendants().into_iter().find(|node| {
        node.kind() == "scoped_type_identifier"
            && node
                .child_by_field_name("name")
                .and_then(|name| source_position_of(name, source))
                == Some(position)
    })
}

/// trait の関連型宣言を持つ trait 名の位置。impl の定義や自由な alias は `None`。
pub(crate) fn associated_owner_position_of(
    source: &str,
    position: SourcePosition,
) -> Option<SourcePosition> {
    let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
    let member = tree.named_descendants().into_iter().find(|node| {
        matches!(node.kind(), "associated_type" | "type_item")
            && node
                .child_by_field_name("name")
                .and_then(|name| source_position_of(name, source))
                == Some(position)
    })?;
    let body = member.parent()?;
    let owner = body.parent()?;
    let is_trait_member = body.kind() == "declaration_list" && owner.kind() == "trait_item";
    if !is_trait_member {
        return None;
    }
    source_position_of(owner.child_by_field_name("name")?, source)
}

pub(super) fn ancestor_of_kind<'tree>(mut node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == kind {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn is_direct_projection(path: Node<'_>, implemented: Node<'_>, source: &str) -> bool {
    if source.get(path.byte_range()) == Some(SELF_TYPE) {
        return true;
    }
    if path.kind() != "bracketed_type" {
        return false;
    }
    let Some(qualified) = named_children_of(path)
        .next()
        .filter(|node| node.kind() == "qualified_type")
    else {
        return false;
    };
    let direct_self = qualified
        .child_by_field_name("type")
        .is_some_and(|node| source.get(node.byte_range()) == Some(SELF_TYPE));
    let matching_trait_arguments = qualified.child_by_field_name("alias").is_some_and(|node| {
        projection_name_of(node, source) == projection_name_of(implemented, source)
    });
    direct_self && matching_trait_arguments
}

pub(super) fn terminal_name_of(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "generic_type" => terminal_name_of(node.child_by_field_name("type")?),
        "scoped_type_identifier" => node.child_by_field_name("name"),
        "type_identifier" => Some(node),
        _ => None,
    }
}

fn parameter_names_of(
    parameters: Node<'_>,
    source: &str,
    include_lifetimes: bool,
) -> Option<Vec<String>> {
    named_children_of(parameters)
        .filter(|parameter| include_lifetimes || parameter.kind() != "lifetime_parameter")
        .map(|parameter| {
            if parameter.kind() != "type_parameter" {
                return Some(source.get(parameter.byte_range())?.to_owned());
            }
            Some(
                source
                    .get(parameter.child_by_field_name("name")?.byte_range())?
                    .to_owned(),
            )
        })
        .collect()
}

fn has_attribute(node: Node<'_>) -> bool {
    let mut preceding = node.prev_named_sibling();
    while let Some(previous) = preceding {
        if previous.kind() == "attribute_item" {
            return true;
        }
        if !COMMENT_KINDS.contains(&previous.kind()) {
            return false;
        }
        preceding = previous.prev_named_sibling();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_associated_gat_captures_impl_parameters_without_capturing_method_parameters() {
        let source = "impl<T> Project < T > for Holder<T> { type Wrap<U> = (U, T); fn f<V>(x: Self::Wrap<V>) -> <Self /* scope */ as Project<T>> :: Wrap<V> {} }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let references = type_references_of(function, source);
        let projected = references
            .iter()
            .find(|reference| reference.name() == "Self::Wrap")
            .expect("GAT の先頭を辿る");
        let qualified = references
            .iter()
            .find(|reference| reference.name() == "<Self as Project<T>>::Wrap")
            .expect("修飾パスの空白とコメントは正規化する");
        assert!(RustProjectionSource::from_source(source, qualified.position()).is_some());
        let definition = RustProjectionSource::from_source(source, projected.position()).unwrap();
        let resolution = definition.resolution_with(&|_| None).unwrap();
        let projected = RustCallable::from_spelling(
            "fn f<V>(x: Self::Wrap<V>) -> <Self as Project<T>>::Wrap<V>",
            &|name| name.contains("Wrap").then(|| resolution.clone()),
            Some("impl<T> Project<T> for Holder<T>"),
        )
        .unwrap();
        let explicit = RustCallable::from_spelling(
            "fn f<V>(x: (V,T)) -> (V,T)",
            &|_| None,
            Some("impl<T> Project<T> for Holder<T>"),
        )
        .unwrap();
        assert_eq!(projected, explicit);
        assert!(!projected.refers_to_self());
        for spelling in [
            "fn f(x: Self::Wrap)",
            "fn f(x: Self::Wrap<u8,u64>)",
            "fn f(x: Self::Wrap<'static>)",
        ] {
            let invalid = RustCallable::from_spelling(
                spelling,
                &|name| name.contains("Wrap").then(|| resolution.clone()),
                Some("impl<T> Project<T> for Holder<T>"),
            )
            .unwrap();
            assert!(invalid.has_unopenable_associated_type(), "{spelling}");
        }
    }

    #[test]
    fn test_nongeneric_associated_type_keeps_its_error_kind_in_a_lifetime_impl() {
        let source = "impl<'a> Project for Holder<'a> { type Item = u8; fn f(x: Self::Item) {} }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let references = type_references_of(function, source);
        let reference = references
            .iter()
            .find(|reference| reference.name() == "Self::Item")
            .unwrap();
        let projection = RustProjectionSource::from_source(source, reference.position()).unwrap();
        let resolution = projection.resolution_with(&|_| None).unwrap();
        let header = Some("impl<'a> Project for Holder<'a>");
        let actual = RustCallable::from_spelling(
            "fn f(x: Self::Item)",
            &|name| (name == "Self::Item").then(|| resolution.clone()),
            header,
        )
        .unwrap();
        let expected = RustCallable::from_spelling("fn f(x: u8)", &|_| None, header).unwrap();
        assert_eq!(actual, expected);
        for spelling in ["fn f(x: Self::Item<u8>)", "fn f(x: Self::Item<'static>)"] {
            let invalid = RustCallable::from_spelling(
                spelling,
                &|name| (name == "Self::Item").then(|| resolution.clone()),
                header,
            )
            .unwrap();
            assert!(invalid.has_unopenable_associated_type(), "{spelling}");
        }
    }

    #[test]
    fn test_associated_definition_selection_rejects_other_traits_arguments_and_ambiguous_definitions()
     {
        for source in [
            "impl<T> Project<T> for Holder<T> { type Item = T; fn f<U>(x: <Self as Project<U>>::Item) {} }",
            "impl<T> Project<T> for Holder<T> { type Item = T; fn f(x: <Self as Other<T>>::Item) {} }",
            "impl<T> Project<T> for Holder<T> { type Item = T; fn f(x: Self::Item::Nested) {} }",
            "impl Project for Holder { type Item = u8; type Item = u64; fn f(x: Self::Item) {} }",
            "impl Project for Holder { #[cfg(feature=\"small\")] type Item = u8; fn f(x: Self::Item) {} }",
            "impl Holder { fn f(x: <Self as Project>::Item) {} }",
        ] {
            let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
            let function = tree
                .named_descendants()
                .into_iter()
                .find(|node| node.kind() == "function_item")
                .unwrap();
            let references = type_references_of(function, source);
            let projection = references
                .iter()
                .find(|reference| reference.name().contains("Self"))
                .expect("未解決の投影も尋ねる");
            assert!(
                RustProjectionSource::from_source(source, projection.position()).is_none(),
                "{source}"
            );
        }
    }

    #[test]
    fn test_associated_rhs_substitutes_impl_variables_inside_type_arguments_of_declared_types() {
        let source = "impl<T> Named for Holder<T> { type Item = Vec<T>; fn f(x: Self::Item) {} }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let references = type_references_of(function, source);
        let reference = references
            .iter()
            .find(|reference| reference.name() == "Self::Item")
            .unwrap();
        let projection = RustProjectionSource::from_source(source, reference.position()).unwrap();
        let declared =
            |name: &str| (name == "Vec").then(|| RustTypeResolution::Declared("@vec".to_owned()));
        let resolution = projection.resolution_with(&declared).unwrap();
        let header = Some("impl<T> Named for Holder<T>");
        let projected = RustCallable::from_spelling(
            "fn f(x: Self::Item)",
            &|name| (name == "Self::Item").then(|| resolution.clone()),
            header,
        )
        .unwrap();
        let explicit = RustCallable::from_spelling("fn f(x: Vec<T>)", &declared, header).unwrap();
        assert_eq!(projected, explicit);
    }

    #[test]
    fn test_associated_rhs_names_keep_their_declaration_scope() {
        let source = "impl<T> Project for Holder<T> { type Item = Payload; fn f<Payload>(x: Self::Item) {} }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let references = type_references_of(function, source);
        let reference = references
            .iter()
            .find(|reference| reference.name() == "Self::Item")
            .unwrap();
        let projection = RustProjectionSource::from_source(source, reference.position()).unwrap();
        assert_eq!(projection.references()[0].name(), "Payload");
        let resolution = projection
            .resolution_with(&|name| {
                (name == "Payload").then(|| RustTypeResolution::Declared("@declaration".to_owned()))
            })
            .unwrap();
        let actual = RustCallable::from_spelling(
            "fn f<Payload>(x: Self::Item)",
            &|name| (name == "Self::Item").then(|| resolution.clone()),
            Some("impl<T> Project for Holder<T>"),
        )
        .unwrap();
        let expected = RustCallable::from_spelling(
            "fn f<U>(x: Payload)",
            &|name| {
                (name == "Payload").then(|| RustTypeResolution::Declared("@declaration".to_owned()))
            },
            Some("impl<T> Project for Holder<T>"),
        )
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_associated_projection_references_point_at_the_member_in_utf16() {
        let source = "impl Project for Holder { type Item = u8; /*🦀*/ fn f(x: Self::Item) {} }";
        let tree = SyntaxTree::from_source(source, Grammar::Rust).unwrap();
        let function = tree
            .named_descendants()
            .into_iter()
            .find(|node| node.kind() == "function_item")
            .unwrap();
        let references = type_references_of(function, source);
        let reference = references
            .iter()
            .find(|reference| reference.name() == "Self::Item")
            .unwrap();
        let prefix = source.split("Self::Item").next().unwrap().to_owned() + "Self::";
        assert_eq!(
            reference.position(),
            SourcePosition::from_preceding_text(
                crate::line_number::LineNumber::from_index(0),
                &prefix
            )
        );
        assert!(RustProjectionSource::from_source(source, reference.position()).is_some());
    }
}
