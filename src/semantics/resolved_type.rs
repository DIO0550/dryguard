//! シグネチャに書かれた型名を、それが指す型の綴りへ解決する。
//!
//! hover が返す綴りは**ソースに書かれた型名のまま**で、型エイリアスは展開されない
//! （`function applyDiscount(amount: Amount, rate: number): number`）。輸入した名前は
//! 使用側の位置へ hover を送っても `import Amount` としか返らないので、
//! **宣言の場所まで辿ってから尋ね直す**（typescript-language-server 6.0.0 で実測）。
//!
//! ```text
//! 型名の位置 --typeDefinition--> 宣言の場所 --hover--> `type Amount = number`
//! ```
//!
//! **綴りを読む部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::HashMap;

use crate::lsp::{
    ClientError, DeclarationSite, HoverOutcome, Session, SourceDocument, TypeDefinitionOutcome,
};
use crate::syntax::type_reference::TypeReference;

/// 型エイリアスの宣言を導く語。前後の空白ごと見て、`typeof` のような綴りと分ける。
const ALIAS_KEYWORD: &str = "type ";

/// 型エイリアスの右辺を導く印。
///
/// 前後の空白ごと見るのは、関数型の `=>` と分けるため（そちらは `=` の後ろが `>`）。
const ALIAS_MARKER: &str = " = ";

/// 型引数を取る宣言の始まり。
const TYPE_ARGUMENTS_START: char = '<';

/// 型名 1 つと、その宣言が置かれている場所。
///
/// **名前と場所を組で持つ。** 解決した綴りを差し込む先は綴りなので、
/// どの名前についての宣言だったかを落とすと差し込めない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    name: String,
    site: DeclarationSite,
}

impl TypeDeclaration {
    /// ソースに書かれた型名の綴り。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// その型が宣言されている場所。
    pub fn site(&self) -> &DeclarationSite {
        &self.site
    }
}

/// 型名から、解決後の綴りへの対応。
///
/// **入るのは型エイリアスだけ。** `interface` / `class` / `enum` は hover が
/// `interface User` としか返さず、置き換える先の綴りが無い。
/// それらを綴りのまま比べると、同じ局所名で別の型を指す 2 つが単一化可能に出る
/// （定義の場所で見分ける話は Issue #131）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedTypes {
    by_name: HashMap<String, String>,
}

impl ResolvedTypes {
    /// 型名と解決後の綴りの組から作る。
    pub fn new(resolutions: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            by_name: resolutions.into_iter().collect(),
        }
    }

    /// その型名の解決後の綴り。解決できていなければ `None`。
    ///
    /// `None` は「エイリアスではなかった」と「宣言まで届かなかった」の両方で返る。
    /// **どちらでも綴りをそのまま比べる**ので、呼び出し側が分ける材料は要らない。
    pub fn resolved_of(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }
}

/// 型名の宣言を尋ねた結果。
///
/// **サーバが typeDefinition を提供していないことを、宣言が無いのと同じにしない。**
/// 前者では**どの型名も開けない**ので、綴りのまま比べた結果を「単一化不能」として
/// 出すと、確かめられなかったことを確かめた答えとして出すことになる
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDeclarationsOutcome {
    /// 尋ね終えた。宣言の場所が返らなかった型名は入らない。
    Located(Vec<TypeDeclaration>),
    /// サーバが typeDefinition を提供していない。
    TypeDefinitionNotProvided,
}

/// シグネチャに書かれた型名の宣言が、どこにあるかを尋ねる。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`type_references` は
/// `Chunk::type_references` が集めた型名。
///
/// **宣言の場所が返らなかった型名は落とす。** その型名が解決できないだけで、
/// 残りは解決できるので、綴りのまま比較へ進む（今までと同じ形）。
///
/// **サーバが typeDefinition を提供していないときだけは落とさない。** そのときは
/// **どの型名も開けない**ので、取れなかったこととして返す。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
pub fn type_declarations_of(
    session: &mut Session,
    document: &SourceDocument,
    type_references: &[TypeReference],
) -> Result<TypeDeclarationsOutcome, ClientError> {
    let mut declarations = Vec::new();

    for reference in type_references {
        match session.type_definition(document, reference.position())? {
            TypeDefinitionOutcome::Answered(site) => declarations.push(TypeDeclaration {
                name: reference.name().to_owned(),
                site,
            }),
            TypeDefinitionOutcome::NotSupported => {
                return Ok(TypeDeclarationsOutcome::TypeDefinitionNotProvided);
            }
            TypeDefinitionOutcome::NoAnswer | TypeDefinitionOutcome::Unreadable { .. } => continue,
        }
    }

    Ok(TypeDeclarationsOutcome::Located(declarations))
}

/// 宣言の位置へ hover を送り、型エイリアスの右辺を集める。
///
/// `declarations` は [`type_declarations_of`] が返した宣言。**そのファイルは先に
/// 呼び出し側が開かせておく**（開かせていないファイルへの hover は綴りを持たない
/// 応答になり、その型名は解決されないまま残る）。
///
/// # Errors
///
/// 往復が失敗したとき。
pub fn resolved_types_of(
    session: &mut Session,
    declarations: &[TypeDeclaration],
) -> Result<ResolvedTypes, ClientError> {
    let mut resolutions = Vec::new();

    for declaration in declarations {
        let HoverOutcome::Answered(declared) = session.hover_at_declaration(declaration.site())?
        else {
            continue;
        };
        let Some(resolved) = resolved_type_of(&declared) else {
            continue;
        };

        resolutions.push((declaration.name().to_owned(), resolved));
    }

    Ok(ResolvedTypes::new(resolutions))
}

/// hover が返した宣言の綴りから、その名前が指す型の綴りを読む。
///
/// `declared` は宣言の位置へ hover を送って返った綴り（`type Amount = number` /
/// `interface User` / `class Invoice`）。型エイリアスでなければ `None`。
///
/// **型引数を取るエイリアスは開かない。** `type Box<T> = { value: T }` の右辺を
/// `Box<string>` の位置へ差し込むと `{ value: T }<string>` になり、綴りとして壊れる。
/// 型引数の当てはめには型そのものの構文解析が要る。
///
/// **1 段しか開かない。** `type A = B` の右辺は `B` のままになる。2 段目を開くには
/// `B` が書かれている位置が要り、それは宣言のあるファイルの中にあるので、
/// **もう一度そのファイルを構文木にするところから始まる**。
fn resolved_type_of(declared: &str) -> Option<String> {
    let aliased = declared.trim().strip_prefix(ALIAS_KEYWORD)?;
    let (name, resolved) = aliased.split_once(ALIAS_MARKER)?;

    if name.contains(TYPE_ARGUMENTS_START) {
        return None;
    }

    let resolved = resolved.trim();
    if resolved.is_empty() {
        return None;
    }

    Some(resolved.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_type_of_an_alias_declaration_is_the_spelling_on_its_right() {
        assert_eq!(
            resolved_type_of("type Amount = number"),
            Some("number".to_owned())
        );
    }

    #[test]
    fn test_resolved_type_of_an_alias_of_a_function_type_keeps_the_whole_type() {
        // `=>` を右辺の区切りと取り違えると、`(value: string)` だけが残る
        assert_eq!(
            resolved_type_of("type Handler = (value: string) => number"),
            Some("(value: string) => number".to_owned())
        );
    }

    #[test]
    fn test_resolved_type_of_an_alias_written_over_lines_keeps_every_line() {
        // オブジェクト型はサーバが複数行に展開して返す。1 行目で打ち切ると型が変わる
        assert_eq!(
            resolved_type_of("type Shape = {\n    id: string;\n}"),
            Some("{\n    id: string;\n}".to_owned())
        );
    }

    #[test]
    fn test_resolved_type_of_an_interface_declaration_is_not_read_as_an_alias() {
        // 対照は最初のテスト。`interface` には右辺が無いので、開く先が無い
        assert_eq!(resolved_type_of("interface User"), None);
    }

    #[test]
    fn test_resolved_type_of_a_class_declaration_is_not_read_as_an_alias() {
        assert_eq!(resolved_type_of("class Invoice"), None);
    }

    #[test]
    fn test_resolved_type_of_an_alias_taking_type_arguments_is_not_opened() {
        // 右辺を `Box<string>` の位置へ差し込むと `{ value: T; }<string>` になる。
        // 型引数の当てはめには型そのものの構文解析が要る
        assert_eq!(resolved_type_of("type Box<T> = { value: T; }"), None);
    }

    #[test]
    fn test_resolved_type_of_a_name_that_only_starts_with_the_keyword_is_not_read_as_an_alias() {
        // `type` を語として見ないと、`typeof` で始まる綴りを開いてしまう
        assert_eq!(resolved_type_of("typeof rate = number"), None);
    }

    #[test]
    fn test_resolved_type_of_an_alias_without_a_right_hand_side_is_not_read() {
        // 空の綴りを解決結果にすると、差し込んだ先の型が消える
        assert_eq!(resolved_type_of("type Amount = "), None);
    }

    #[test]
    fn test_resolved_types_answer_only_for_the_names_they_were_given() {
        let resolved = ResolvedTypes::new([("Amount".to_owned(), "number".to_owned())]);

        assert_eq!(resolved.resolved_of("Amount"), Some("number"));
        assert_eq!(resolved.resolved_of("Total"), None);
    }
}
