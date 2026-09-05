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
    /// 型名と、その宣言が置かれている場所から作る。
    pub fn new(name: String, site: DeclarationSite) -> Self {
        Self { name, site }
    }

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
/// `interface User` としか返さず、置き換える先の綴りが無い。それらは綴りのまま
/// 比較へ進むので、**同じ局所名で別の型を指す 2 つを見分けるのは綴りではなく
/// [`TypeDeclaration`] の側**（`semantics::type_signature` が宣言の場所で突き合わせる）。
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

    /// その型名の解決後の綴り。開けていなければ `None`。
    ///
    /// `None` は「エイリアスではなかった」と「開けなかった」の両方で返る。
    /// **分けるのは [`TracedTypeNames::unopened_reason_of`] の側**で、ここは
    /// 差し込む綴りがあるかだけを答える。
    pub fn resolved_of(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }
}

/// 型名 1 つを開けなかった理由。
///
/// **1 つにまとめない。** どれなのかで**利用者が次にすることが違う**（サーバを替える /
/// そのファイルをプロジェクトに入れる / そのファイルを読めるようにする /
/// そのチャンクを諦める / dryguard 側の穴）(`rules/coding.md`
/// 「エラー型は原因ごとにバリアントを分ける」)。
///
/// **止まった段ではなく、次にすることで分ける。** typeDefinition の段だけで 3 通りの
/// 対処に分かれるので、段でまとめると直す先が読めなくなる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnopenedReason {
    /// サーバが typeDefinition を提供していない。
    TypeDefinitionNotProvided,
    /// サーバが宣言の場所を答えなかった。
    ///
    /// **宣言が無いとは限らない。** そのファイルをプロジェクトとして見ていないときにも
    /// 空が返る（`lsp::TypeDefinitionOutcome::NoAnswer`）ので、サーバの答えとして読まない。
    NoDeclarationSite,
    /// 宣言の場所は返ったが、パスとして読めない URI だった。
    UnreadableTypeDefinition,
    /// 宣言のファイルを読めず、サーバに開かせられなかった。
    UnreadableDeclaringDocument,
    /// サーバが宣言の位置に綴りを持たなかった。
    NoSpellingAtDeclaration,
    /// 宣言の位置の hover の応答を `lsp` が読めなかった。
    UnreadableDeclarationHover,
    /// サーバが hover を提供していない。
    HoverNotProvided,
}

/// 開けなかった型名 1 つと、その理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnopenedTypeName {
    name: String,
    reason: UnopenedReason,
}

impl UnopenedTypeName {
    /// 開けなかった型名と、その理由から作る。
    pub fn new(name: String, reason: UnopenedReason) -> Self {
        Self { name, reason }
    }

    /// ソースに書かれた型名の綴り。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 開けなかった理由。
    pub fn reason(&self) -> UnopenedReason {
        self.reason
    }
}

/// シグネチャに書かれた型名を、宣言まで辿った結果。
///
/// 型名 1 つは、宣言の場所が取れた（[`TracedTypeNames::declared`]）・開いた綴りが取れた
/// （[`TracedTypeNames::resolved`]）・開けなかった（[`TracedTypeNames::unopened_reason_of`]）の
/// どれかに入る。
///
/// **3 つを 1 つの値で持つ。** 開けた型名は差し込みで綴りから消え、開けなかった型名は
/// 綴りに残る。**比較に残る綴りに現れたのがどちらだったか**を同じ 1 つの入力から引けないと、
/// 開けなかったことを答えに出すかどうかが決められない
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TracedTypeNames {
    declared: Vec<TypeDeclaration>,
    resolved: ResolvedTypes,
    unopened: Vec<UnopenedTypeName>,
}

impl TracedTypeNames {
    /// 宣言の場所が取れた型名と、そこで開けなかった型名から作る。
    pub fn new(declared: Vec<TypeDeclaration>, unopened: Vec<UnopenedTypeName>) -> Self {
        Self {
            declared,
            resolved: ResolvedTypes::default(),
            unopened,
        }
    }

    /// 後の段で開けなかった型名を足す。
    pub fn with_unopened(mut self, unopened: Vec<UnopenedTypeName>) -> Self {
        self.unopened.extend(unopened);
        self
    }

    /// 開けた型名の綴りを持たせる。**足すのではなく置き換える**（開くのは 1 度だけ）。
    pub fn with_resolved(self, resolved: ResolvedTypes) -> Self {
        Self { resolved, ..self }
    }

    /// 宣言の場所が取れた型名。
    pub fn declared(&self) -> &[TypeDeclaration] {
        &self.declared
    }

    /// 開けた型名の、解決後の綴り。
    pub fn resolved(&self) -> &ResolvedTypes {
        &self.resolved
    }

    /// その型名を開けなかった理由。開けていれば `None`。
    pub fn unopened_reason_of(&self, name: &str) -> Option<UnopenedReason> {
        self.unopened
            .iter()
            .find(|unopened| unopened.name() == name)
            .map(UnopenedTypeName::reason)
    }
}

/// シグネチャに書かれた型名の宣言が、どこにあるかを尋ねる。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`type_references` は
/// `Chunk::type_references` が集めた型名。
///
/// **宣言まで届かなかった型名を、ここでは落とさない。** 落とすかどうかは
/// **比較に残る綴りに現れるか**で決まり、それが分かるのは正規化した後
/// （`semantics::type_signature`）(`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」)。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
pub fn traced_type_names_of(
    session: &mut Session,
    document: &SourceDocument,
    type_references: &[TypeReference],
) -> Result<TracedTypeNames, ClientError> {
    let mut declared = Vec::new();
    let mut unopened = Vec::new();

    for reference in type_references {
        let name = reference.name().to_owned();

        match session.type_definition(document, reference.position())? {
            TypeDefinitionOutcome::Answered(site) => {
                declared.push(TypeDeclaration::new(name, site));
            }
            TypeDefinitionOutcome::NoAnswer => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::NoDeclarationSite,
                ));
            }
            TypeDefinitionOutcome::Unreadable { .. } => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::UnreadableTypeDefinition,
                ));
            }
            TypeDefinitionOutcome::NotSupported => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::TypeDefinitionNotProvided,
                ));
            }
        }
    }

    Ok(TracedTypeNames::new(declared, unopened))
}

/// 宣言の位置へ hover を送り、型エイリアスの右辺を開く。
///
/// `traced` は [`traced_type_names_of`] が返した結果。**宣言のファイルは先に
/// 呼び出し側が開かせておく**（開かせていないファイルへの hover は綴りを持たない
/// 応答になる）。
///
/// **エイリアスでなかった型名は落とす。** `interface` / `class` に右辺が無いのは
/// **サーバの答え**で、開けなかったのとは別物（綴りのまま比べてよい）。
///
/// # Errors
///
/// 往復が失敗したとき。
pub fn opened_type_names_of(
    session: &mut Session,
    traced: TracedTypeNames,
) -> Result<TracedTypeNames, ClientError> {
    let mut resolutions = Vec::new();
    let mut unopened = Vec::new();

    for declaration in traced.declared() {
        let name = declaration.name().to_owned();

        let declared = match session.hover_at_declaration(declaration.site())? {
            HoverOutcome::Answered(declared) => declared,
            HoverOutcome::NoAnswer => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::NoSpellingAtDeclaration,
                ));
                continue;
            }
            HoverOutcome::Unreadable => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::UnreadableDeclarationHover,
                ));
                continue;
            }
            HoverOutcome::NotSupported => {
                unopened.push(UnopenedTypeName::new(
                    name,
                    UnopenedReason::HoverNotProvided,
                ));
                continue;
            }
        };

        let Some(resolved) = resolved_type_of(declared.as_str()) else {
            continue;
        };

        resolutions.push((name, resolved));
    }

    Ok(traced
        .with_resolved(ResolvedTypes::new(resolutions))
        .with_unopened(unopened))
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
