//! 短縮形の投影（`Self::Item`）が supertrait の関連型を指すときに、supertrait の閉包を
//! 辿るための trait 宣言の構文情報。
//!
//! **supertrait の同一性はここでは決めない。** 書かれた位置を返し、宣言元は
//! `semantics::resolved_type` が definition で辿る（別名・パスの綴りで比べない）。

use super::projection::terminal_name_of;
use super::*;
use crate::source_position::SourcePosition;

/// trait 宣言 1 つの、supertrait と宣言している関連型の名前。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustTraitDeclaration {
    supertraits: Vec<RustSupertrait>,
    associated_types: Vec<String>,
}

/// trait 宣言に書かれた supertrait 1 つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RustSupertrait {
    position: SourcePosition,
    has_arguments: bool,
}

impl RustTraitDeclaration {
    /// 名前が `position` にある trait 宣言を読む。supertrait は `trait Sub: Show` の境界と、
    /// 左辺が `Self` の where 述語（`where Self: Show`）の両方から集める。
    ///
    /// 宣言が無い・宣言の頭に構文エラーがある・ライフタイムと `?Sized` 以外に読めない境界
    /// （`for<'a> Trait<'a>` など）があれば `None`。
    pub(crate) fn from_source(source: &str, position: SourcePosition) -> Option<Self> {
        let tree = SyntaxTree::from_source(source, Grammar::Rust).ok()?;
        let declaration = tree.named_descendants().into_iter().find(|node| {
            node.kind() == "trait_item"
                && node
                    .child_by_field_name("name")
                    .and_then(|name| source_position_of(name, source))
                    == Some(position)
        })?;
        if signature_has_error(declaration) {
            return None;
        }
        let self_bounds = named_children_of(declaration)
            .filter(|child| child.kind() == "where_clause")
            .flat_map(named_children_of)
            .filter(|predicate| {
                predicate
                    .child_by_field_name("left")
                    .is_some_and(|left| source.get(left.byte_range()) == Some(SELF_TYPE))
            })
            .filter_map(|predicate| predicate.child_by_field_name("bounds"));
        let mut supertraits = Vec::new();
        for bounds in declaration
            .child_by_field_name("bounds")
            .into_iter()
            .chain(self_bounds)
        {
            for bound in named_children_of(bounds) {
                let has_arguments = match bound.kind() {
                    "type_identifier" | "scoped_type_identifier" => false,
                    "generic_type" => true,
                    // Why: ライフタイムと `?Sized` は関連型を持ち込まない
                    "lifetime" | "removed_trait_bound" => continue,
                    _ => return None,
                };
                supertraits.push(RustSupertrait {
                    position: source_position_of(terminal_name_of(bound)?, source)?,
                    has_arguments,
                });
            }
        }
        let associated_types = declaration
            .child_by_field_name("body")
            .into_iter()
            .flat_map(named_children_of)
            .filter(|member| member.kind() == "associated_type")
            .filter_map(|member| {
                Some(
                    source
                        .get(member.child_by_field_name("name")?.byte_range())?
                        .to_owned(),
                )
            })
            .collect();
        Some(Self {
            supertraits,
            associated_types,
        })
    }

    /// 書かれた順の supertrait。
    pub(crate) fn supertraits(&self) -> &[RustSupertrait] {
        &self.supertraits
    }

    /// `name` の関連型を宣言しているか。
    pub(crate) fn declares(&self, name: &str) -> bool {
        self.associated_types
            .iter()
            .any(|declared| declared == name)
    }
}

impl RustSupertrait {
    /// trait パスの末尾の名前。definition と implementation をここへ尋ねる。
    pub(crate) fn position(&self) -> SourcePosition {
        self.position
    }

    /// 型引数を書いた supertrait（`Convert<u8>`）か。
    pub(crate) fn has_arguments(&self) -> bool {
        self.has_arguments
    }
}
