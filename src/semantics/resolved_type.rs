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
//! **Rust は definition で宣言を辿り、宣言 hover の右辺を Rust の構文で読む。**
//! 型引数の当てはめを支え、引数のないエイリアスの右辺の名前は宣言側ソースから辿る。
//! Self を含む関連型は、直接囲む trait impl の定義と trait の宣言元を照合して開く。
//! 直接囲む impl が別の trait の impl・inherent impl なら、完全修飾形の trait の impl を
//! implementation で集め、型変数に使用側の型を当てはめるだけで対象型と trait 引数が
//! 一致する 1 つを選ぶ。
//!
//! **綴りを読む部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::HashMap;

use crate::codebase::source_of;
use crate::lsp::{
    ClientError, DeclarationSite, DeclarationSiteOutcome, HoverOutcome, ImplementationOutcome,
    Session, SignatureText, SourceDocument,
};
use crate::source_position::SourcePosition;
use crate::syntax::rust_callable::{
    RustAliasSource, RustAssociatedDefinition, RustImplBinding, RustImplCandidate,
    RustProjectionSource, RustQualifiedProjection, RustTypeResolution,
    associated_owner_position_of, primitive_references_of_alias, primitive_spelling_of,
};
use crate::syntax::type_reference::TypeReference;

mod array_length;
use array_length::evaluated_array_spelling_of;
pub(crate) use array_length::restored_array_spelling_of;

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
/// **TypeScript に入るのは型エイリアスだけ。** `interface` / `class` / `enum` は hover が
/// `interface User` としか返さず、置き換える先の綴りが無い。それらは綴りのまま
/// 比較へ進むので、**同じ局所名で別の型を指す 2 つを見分けるのは綴りではなく
/// [`TypeDeclaration`] の側**（`semantics::type_signature` が宣言の場所で突き合わせる）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedTypes {
    by_name: HashMap<String, String>,
    rust_types: HashMap<String, RustTypeResolution>,
}

impl ResolvedTypes {
    /// 型名と解決後の綴りの組から作る。
    pub fn new(resolutions: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            by_name: resolutions.into_iter().collect(),
            rust_types: HashMap::new(),
        }
    }

    /// Rust の解決結果。具体的な宣言元、開いた右辺、型引数を当てはめるテンプレートを返す。
    pub(crate) fn rust_type_of(&self, name: &str) -> Option<RustTypeResolution> {
        if let Some(alias) = self.rust_types.get(name) {
            return Some(alias.clone());
        }
        self.resolved_of(name)
            .map(|right| RustTypeResolution::Opened(right.to_owned()))
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
    /// サーバが definition を提供していない。
    ///
    /// **typeDefinition と分ける。** どちらを尋ねるかは言語で決まり、提供していない
    /// 問い合わせの名前が違えば、利用者が確かめるサーバの設定も違う。
    DefinitionNotProvided,
    /// サーバが宣言の場所を答えなかった。
    ///
    /// **宣言が無いとは限らない。** そのファイルをプロジェクトとして見ていないときにも
    /// 空が返る（`lsp::DeclarationSiteOutcome::NoAnswer`）ので、サーバの答えとして読まない。
    NoDeclarationSite,
    /// サーバが definition に宣言の場所を答えなかった。
    ///
    /// **[`UnopenedReason::NoDeclarationSite`] と分ける。** definition を尋ねるのは Rust で、
    /// **rust-analyzer は rust-src が無いと std / core の名前にも空を返す**
    /// （1.94.1 で実測）。利用者が次に試すこと（rust-src を入れる）が増えるので、
    /// TypeScript の利用者にその案内を出さないよう問い合わせで分ける。
    NoDefinitionSite,
    /// typeDefinition の宣言の場所は返ったが、パスとして読めない URI だった。
    UnreadableTypeDefinition,
    /// definition の宣言の場所は返ったが、パスとして読めない URI だった。
    UnreadableDefinition,
    /// 宣言のファイルを読めず、サーバに開かせられなかった。
    UnreadableDeclaringDocument,
    /// サーバが宣言の位置に綴りを持たなかった。
    NoSpellingAtDeclaration,
    /// 宣言の位置の hover の応答を `lsp` が読めなかった。
    UnreadableDeclarationHover,
    /// 尋ねるたびにサーバが作業を始めるので、宣言の位置の hover が落ち着かなかった。
    ///
    /// **途中の綴りを右辺として差し込まない。** 読み込み前の hover は推論された型に
    /// `any` を綴るので、差し込むと**開けていない型名を開けたことにする**
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    ServerStillWorking,
    /// サーバが hover を提供していない。
    HoverNotProvided,
    /// 宣言は型エイリアスだが、右辺を差し込める形にできなかった。
    ///
    /// **サーバは右辺を返している。** 開けないのは dryguard 側の都合（型引数の当てはめが
    /// 要るライフタイム・const 引数や未対応の型構文など）なので、開く先が無い `interface` / `class` とは
    /// 分けて出す。
    UnopenableAlias,
    /// 同じ宣言元へ戻る型エイリアスの連鎖。
    CyclicAlias,
    /// 型エイリアスの連鎖の深さ、または展開した右辺の大きさが上限に達した。
    AliasExpansionLimit,
    /// 関連型の具体的な impl 定義を一意に選べなかった。
    UnresolvedAssociatedType,
    /// サーバが implementation を提供していない。
    ///
    /// **definition と分ける。** 別の impl の関連型を選ぶのに要る問い合わせで、
    /// 利用者が確かめるサーバの設定が違う。
    ImplementationNotProvided,
    /// サーバが trait の impl を 1 件も返さなかった。
    ///
    /// **impl が無いという答えとして読まない。** プロジェクトの読み込み前にも空が返る
    /// （[`UnopenedReason::NoDeclarationSite`] と同じ理由）。
    NoImplementationSite,
    /// implementation が返した場所がパスとして読めない URI で、
    /// 読めた候補からは impl を選べなかった。
    UnreadableImplementation,
    /// 選んだ関連型の RHS または型引数を展開できなかった。
    UnopenableAssociatedType,
    /// 配列長が未対応の式、無効な演算、または一意に選べない定数だった。
    UnevaluableArrayLength,
    /// 配列長の定数連鎖が同じ宣言へ戻った。
    CyclicArrayLength,
    /// 配列長の値・式・連鎖・評価全体の作業量が上限に達した。
    ArrayLengthEvaluationLimit,
    /// ソースの配列長と hover の型構造・値を対応させられなかった。
    UnmatchedArrayLength,
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
    DeclarationQuery::TypeDefinition.traced_type_names_of(session, document, type_references)
}

/// Rust のシグネチャに書かれた型名・トレイト名の宣言が、どこにあるかを尋ねる。
///
/// 引数と、宣言まで届かなかった型名を落とさないのは [`traced_type_names_of`] と同じ。
///
/// **typeDefinition ではなく definition を尋ねる。** rust-analyzer の typeDefinition は
/// 名前ではなく**その名前が指す型**の宣言を返すので、型エイリアス（`type Amount = u64`）には
/// 空を返し、ジェネリックな型（`Option<User>`）には型引数の宣言まで並べる。definition は
/// 書かれた名前の宣言を 1 件返す（rust-analyzer 1.94.1 と 2026-09-21 版で実測）。
///
/// **宣言 hover の右辺を Rust の構文で読む。** 型引数は使用側で当てはめる。
/// 引数のないエイリアスの右辺の型名は、宣言側ソースの位置から再帰的に辿る。
/// プリミティブ表記も hover で確認し、別の型を指すなら definition で辿る。
/// 非ジェネリック宣言の配列長は元の式を宣言側で評価する。
/// ジェネリック宣言の自由な名前と定数式は `UnopenableAlias` のままにする。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
pub(crate) fn rust_traced_type_names_of(
    session: &mut Session,
    document: &SourceDocument,
    type_references: &[TypeReference],
) -> Result<TracedTypeNames, ClientError> {
    let mut resolver = RustTypeResolver::new(session);
    let mut traced = TracedTypeNames::default();
    for reference in type_references {
        let resolution = resolver.reference_of(document, reference)?;
        match resolution {
            Ok(alias) => {
                traced
                    .resolved
                    .rust_types
                    .insert(reference.name().to_owned(), alias);
            }
            Err(reason) => {
                // Why: プリミティブ表記の失敗も、型名として理由を照合する位置に残す。
                traced
                    .resolved
                    .rust_types
                    .insert(reference.name().to_owned(), RustTypeResolution::Unresolved);
                traced
                    .unopened
                    .push(UnopenedTypeName::new(reference.name().to_owned(), reason));
            }
        }
    }
    Ok(traced)
}

/// 再帰展開できるエイリアスの宣言数。循環はこの上限とは別に検出する。
const MAXIMUM_ALIAS_DEPTH: usize = 32;

/// 宣言側の問い合わせ、ソースと再帰を含まない終端のキャッシュ。
struct RustTypeResolver<'session> {
    session: &'session mut Session,
    sources: HashMap<std::path::PathBuf, Result<String, UnopenedReason>>,
    candidates: HashMap<std::path::PathBuf, Vec<RustImplCandidate>>,
    terminals: Vec<(DeclarationSite, RustTypeResolution)>,
    active: Vec<DeclarationSite>,
}

impl<'session> RustTypeResolver<'session> {
    fn new(session: &'session mut Session) -> Self {
        Self {
            session,
            sources: HashMap::new(),
            candidates: HashMap::new(),
            terminals: Vec::new(),
            active: Vec::new(),
        }
    }

    /// ソースの位置から辿る。本物のプリミティブは宣言元を持たないので hover で確定する。
    ///
    /// # Errors
    ///
    /// hover / definition の往復が失敗したとき。
    fn reference_of(
        &mut self,
        document: &SourceDocument,
        reference: &TypeReference,
    ) -> Result<Result<RustTypeResolution, UnopenedReason>, ClientError> {
        if reference
            .name()
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .any(|name| name == "Self")
        {
            return self.projection_of(document, reference);
        }
        let hover = match declared_spelling_of(self.session.hover(document, reference.position())?)
        {
            Ok(hover) => hover,
            Err(reason) => return Ok(Err(reason)),
        };
        if let Some(primitive) = primitive_spelling_of(hover.as_str()) {
            return Ok(Ok(RustTypeResolution::Opened(primitive)));
        }
        match DeclarationQuery::Definition.site_of(self.session, document, reference.position())? {
            Ok(site) => self.at_declaration(&site),
            Err(reason) => Ok(Err(reason)),
        }
    }

    /// trait 宣言の同一性を確認してから、関連型 RHS を開く。
    ///
    /// 直接囲む trait impl の定義を先に試し、使えない形なら完全修飾形の trait の impl を選ぶ
    /// （[`RustTypeResolver::selected_projection_of`]）。
    ///
    /// # Errors
    ///
    /// definition / hover の往復が失敗したとき。
    fn projection_of(
        &mut self,
        document: &SourceDocument,
        reference: &TypeReference,
    ) -> Result<Result<RustTypeResolution, UnopenedReason>, ClientError> {
        let Some(projection) =
            RustProjectionSource::from_source(document.source(), reference.position())
        else {
            return match RustQualifiedProjection::from_source(
                document.source(),
                reference.position(),
            ) {
                Some(qualified) => self.selected_projection_of(document, reference, &qualified),
                None => Ok(Err(UnopenedReason::UnresolvedAssociatedType)),
            };
        };
        if let Err(reason) = self.owner_matches(
            document,
            reference.position(),
            projection.implemented_trait().position(),
        )? {
            return Ok(Err(reason));
        }
        let mut resolved = HashMap::new();
        for reference in projection.references() {
            match self.reference_of(document, reference)? {
                Ok(resolution) => {
                    resolved.insert(reference.name().to_owned(), resolution);
                }
                Err(reason) => return Ok(Err(reason)),
            }
        }
        Ok(projection
            .resolution_with(&|name| resolved.get(name).cloned())
            .ok_or(UnopenedReason::UnopenableAssociatedType))
    }

    /// 関連型の名前（`member`）の宣言が、`trait_position` に書かれた trait のメンバーか確かめる。
    ///
    /// 同名の別 trait や supertrait の同名メンバーを、書かれた trait の関連型として扱わない。
    ///
    /// # Errors
    ///
    /// definition の往復が失敗したとき。
    fn owner_matches(
        &mut self,
        document: &SourceDocument,
        member: SourcePosition,
        trait_position: SourcePosition,
    ) -> Result<Result<(), UnopenedReason>, ClientError> {
        let query = DeclarationQuery::Definition;
        let member = match query.site_of(self.session, document, member)? {
            Ok(site) => site,
            Err(reason) => return Ok(Err(reason)),
        };
        let source = match self.source_of(member.path()) {
            Ok(source) => source,
            Err(reason) => return Ok(Err(reason)),
        };
        let Some(owner) = associated_owner_position_of(&source, member.position()) else {
            return Ok(Err(UnopenedReason::UnresolvedAssociatedType));
        };
        let implemented = match query.site_of(self.session, document, trait_position)? {
            Ok(site) => site,
            Err(reason) => return Ok(Err(reason)),
        };
        let same_trait = implemented.path() == member.path() && implemented.position() == owner;
        Ok(match same_trait {
            true => Ok(()),
            false => Err(UnopenedReason::UnresolvedAssociatedType),
        })
    }

    /// 完全修飾形の trait の impl から、型変数に使用側の型を当てはめるだけで一致する 1 つを
    /// 選んで RHS を開く。
    ///
    /// 対応する位置の型名は、候補側はそのソースの位置から、使用側は使用位置から辿り、
    /// **具体的な宣言元か、確かめたプリミティブ・開いた右辺が等しい**ときだけ一致とする。
    /// 綴りでは比べない（辿れなかった `Holder` 同士を一致にしない）。
    /// 候補の型変数に当たった使用側の具体的な型も、その型名を使用位置から辿って代入する。
    ///
    /// 選べなかったときの理由は [`UnselectedImpls::reason_with`] が決める。選んだ impl に
    /// 関連型定義が無い（`default type`・属性付き・複数を含む）ときは `UnresolvedAssociatedType`。
    /// 代入する具体的な型の型名を辿れなければその理由、テンプレートにできなければ
    /// `UnopenableAssociatedType`。
    ///
    /// # Errors
    ///
    /// definition / implementation / hover / didOpen の往復が失敗したとき。
    fn selected_projection_of(
        &mut self,
        document: &SourceDocument,
        reference: &TypeReference,
        projection: &RustQualifiedProjection,
    ) -> Result<Result<RustTypeResolution, UnopenedReason>, ClientError> {
        if let Err(reason) =
            self.owner_matches(document, reference.position(), projection.trait_position())?
        {
            return Ok(Err(reason));
        }
        let (sites, has_unreadable) = match self
            .session
            .implementation(document, projection.trait_position())?
        {
            ImplementationOutcome::Answered {
                sites,
                has_unreadable,
            } => (sites, has_unreadable),
            ImplementationOutcome::NoAnswer => {
                return Ok(Err(UnopenedReason::NoImplementationSite));
            }
            ImplementationOutcome::NotSupported => {
                return Ok(Err(UnopenedReason::ImplementationNotProvided));
            }
        };
        let mut unselected = UnselectedImpls {
            has_unreadable_site: has_unreadable,
            ..UnselectedImpls::default()
        };
        let mut used_names = Vec::new();
        let mut selected = Vec::new();
        for site in &sites {
            let source = match site.path() == document.path() {
                true => Ok(document.source().to_owned()),
                false => self.source_of(site.path()),
            };
            let source = match source {
                Ok(source) => source,
                Err(_) => {
                    unselected.has_unreadable_file = true;
                    continue;
                }
            };
            let candidate = self
                .candidates
                .entry(site.path().to_owned())
                .or_insert_with(|| RustImplCandidate::candidates_of(&source))
                .iter()
                .find(|candidate| candidate.target_position() == site.position())
                .cloned();
            if candidate.is_none() {
                unselected.has_skipped_impl = true;
            }
            let candidate = candidate.filter(|candidate| candidate.may_match(projection));
            let Some((binding, candidate)) = candidate
                .and_then(|candidate| Some((candidate.binding_with(projection)?, candidate)))
            else {
                continue;
            };
            let Ok(candidate_document) = SourceDocument::new(site.path(), source.clone()) else {
                unselected.has_unreadable_file = true;
                continue;
            };
            self.session.open_document(&candidate_document)?;
            match self.names_agree(document, &candidate_document, &binding, &mut used_names)? {
                NamesAgree::Yes => selected.push((binding, candidate, candidate_document)),
                NamesAgree::No => {}
                NamesAgree::Untraced(reason) => {
                    unselected.untraced.get_or_insert(reason);
                }
                NamesAgree::UseSiteUntraced(reason) => return Ok(Err(reason)),
            }
        }
        let (binding, candidate, candidate_document) = match <[_; 1]>::try_from(selected) {
            Ok([selected]) => selected,
            Err(selected) => return Ok(Err(unselected.reason_with(selected.len()))),
        };
        // Why: 境界の無い impl にも暗黙の `Sized` があり、unsized な型には別の impl が共存できる。
        // 使用側の型変数は Sized なので穴にならないが、具体的な型を当てはめるなら
        // 読めなかった・照合しなかった impl が残っている限り 1 つに決まらない。
        if binding.binds_concrete_types() && !unselected.is_complete() {
            return Ok(Err(unselected.reason_with(0)));
        }
        let Some(definition) = RustAssociatedDefinition::from_impl_at(
            candidate_document.source(),
            candidate.target_position(),
            projection.associated_name(),
        ) else {
            return Ok(Err(UnopenedReason::UnresolvedAssociatedType));
        };
        let mut captured = HashMap::new();
        for reference in binding.capture_references() {
            let cached = used_names
                .iter()
                .find(|(position, _)| *position == reference.position())
                .map(|(_, resolution)| resolution.clone());
            let resolution = match cached {
                Some(resolution) => resolution,
                None => self.reference_of(document, reference)?,
            };
            match resolution {
                Ok(resolution) => {
                    captured.insert(reference.name().to_owned(), resolution);
                }
                Err(reason) => return Ok(Err(reason)),
            }
        }
        let Some(definition) = binding
            .captures_with(&|name| captured.get(name).cloned())
            .and_then(|captures| definition.with_captures(captures))
        else {
            return Ok(Err(UnopenedReason::UnopenableAssociatedType));
        };
        let mut resolved = HashMap::new();
        for reference in definition.references() {
            match self.reference_of(&candidate_document, reference)? {
                Ok(resolution) => {
                    resolved.insert(reference.name().to_owned(), resolution);
                }
                Err(reason) => return Ok(Err(reason)),
            }
        }
        Ok(definition
            .resolution_with(&|name| resolved.get(name).cloned())
            .ok_or(UnopenedReason::UnopenableAssociatedType))
    }

    /// 束縛が対応付けた型名の組が、すべて同じ型を指すか。
    ///
    /// **比べてよいのは具体的な宣言元（`Declared`）と、確かめたプリミティブ・開いた右辺
    /// （`Opened`）だけ。** ジェネリックなエイリアスなどに当たった組は、当てはめる前の
    /// テンプレートどうしになるので一致にしない（偽陰性側）。
    ///
    /// # Errors
    ///
    /// hover / definition の往復が失敗したとき。
    fn names_agree(
        &mut self,
        document: &SourceDocument,
        candidate_document: &SourceDocument,
        binding: &RustImplBinding,
        used_names: &mut Vec<(SourcePosition, Result<RustTypeResolution, UnopenedReason>)>,
    ) -> Result<NamesAgree, ClientError> {
        for (candidate, used) in binding.names() {
            let cached = used_names
                .iter()
                .find(|(position, _)| *position == used.position())
                .map(|(_, resolution)| resolution.clone());
            let used_resolution = match cached {
                Some(resolution) => resolution,
                None => {
                    let resolution = self.reference_of(document, used)?;
                    used_names.push((used.position(), resolution.clone()));
                    resolution
                }
            };
            let used_resolution = match used_resolution {
                Ok(resolution) => resolution,
                Err(reason) => return Ok(NamesAgree::UseSiteUntraced(reason)),
            };
            let candidate_resolution = match self.reference_of(candidate_document, candidate)? {
                Ok(resolution) => resolution,
                Err(reason) => return Ok(NamesAgree::Untraced(reason)),
            };
            let comparable = matches!(
                used_resolution,
                RustTypeResolution::Declared(_) | RustTypeResolution::Opened(_)
            );
            if !comparable || candidate_resolution != used_resolution {
                return Ok(NamesAgree::No);
            }
        }
        Ok(NamesAgree::Yes)
    }

    /// 宣言側のソース。読めなかった理由もファイルごとに 1 回だけ覚える。
    fn source_of(&mut self, path: &std::path::Path) -> Result<String, UnopenedReason> {
        self.sources
            .entry(path.to_owned())
            .or_insert_with(|| {
                source_of(path).map_err(|_| UnopenedReason::UnreadableDeclaringDocument)
            })
            .clone()
    }

    /// 宣言元をキーに循環を検出する。展開済みの連鎖をキャッシュしないので上限は順序に依存しない。
    ///
    /// # Errors
    ///
    /// 宣言 hover / definition / didOpen の往復が失敗したとき。
    fn at_declaration(
        &mut self,
        site: &DeclarationSite,
    ) -> Result<Result<RustTypeResolution, UnopenedReason>, ClientError> {
        if self.active.contains(site) {
            return Ok(Err(UnopenedReason::CyclicAlias));
        }
        if let Some((_, terminal)) = self.terminals.iter().find(|(cached, _)| cached == site) {
            let alias_at_limit = matches!(terminal, RustTypeResolution::Generic(_))
                && self.active.len() >= MAXIMUM_ALIAS_DEPTH;
            if alias_at_limit {
                return Ok(Err(UnopenedReason::AliasExpansionLimit));
            }
            return Ok(Ok(terminal.clone()));
        }
        let hover = match declared_spelling_of(self.session.hover_at_declaration(site)?) {
            Ok(hover) => hover,
            Err(reason) => return Ok(Err(reason)),
        };
        let alias = RustTypeResolution::from_spelling(hover.as_str());
        if alias == RustTypeResolution::NotAnAlias {
            let identity = format!(
                "@type({:?}, {}, {})",
                site.path(),
                site.position().line(),
                site.position().character()
            );
            let terminal = RustTypeResolution::Declared(identity);
            self.terminals.push((site.clone(), terminal.clone()));
            return Ok(Ok(terminal));
        }
        if self.active.len() >= MAXIMUM_ALIAS_DEPTH {
            return Ok(Err(UnopenedReason::AliasExpansionLimit));
        }
        let source = match self.source_of(site.path()) {
            Ok(source) => source,
            Err(reason) => return Ok(Err(reason)),
        };
        if let RustTypeResolution::Generic(_) = alias {
            let declaration = TypeDeclaration::new(String::new(), site.clone());
            if let Some(reason) =
                rust_alias_unopened_reason(self.session, &declaration, &source, &alias)?
            {
                return Ok(Err(reason));
            }
            self.terminals.push((site.clone(), alias.clone()));
            return Ok(Ok(alias));
        }
        let Some(body) = RustAliasSource::from_source(&source, site.position()) else {
            return Ok(Err(UnopenedReason::UnopenableAlias));
        };
        let Ok(document) = SourceDocument::new(site.path(), source) else {
            return Ok(Err(UnopenedReason::UnreadableDeclaringDocument));
        };
        self.session.open_document(&document)?;
        self.active.push(site.clone());
        let result = self.body_of(&document, &body);
        self.active.pop();
        result
    }

    /// 右辺の名前を宣言側で解決し、解決済みの綴りだけを構文側へ渡す。
    ///
    /// # Errors
    ///
    /// 名前の問い合わせの往復が失敗したとき。
    fn body_of(
        &mut self,
        document: &SourceDocument,
        body: &RustAliasSource,
    ) -> Result<Result<RustTypeResolution, UnopenedReason>, ClientError> {
        let mut resolved = HashMap::new();
        for reference in body.references() {
            match self.reference_of(document, reference)? {
                Ok(alias) => {
                    resolved.insert(reference.name().to_owned(), alias);
                }
                Err(reason) => return Ok(Err(reason)),
            }
        }
        let spelling =
            match evaluated_array_spelling_of(self.session, document, body.array_lengths())? {
                Ok(spelling) => spelling,
                Err(reason) => return Ok(Err(reason)),
            };
        let alias =
            RustTypeResolution::from_spelling_with(&spelling, &|name| resolved.get(name).cloned());
        match alias {
            RustTypeResolution::Opened(_) => Ok(Ok(alias)),
            RustTypeResolution::ExpansionLimit => Ok(Err(UnopenedReason::AliasExpansionLimit)),
            RustTypeResolution::Declared(_)
            | RustTypeResolution::Generic(_)
            | RustTypeResolution::Associated { .. }
            | RustTypeResolution::NotAnAlias
            | RustTypeResolution::Unresolved
            | RustTypeResolution::Unopenable => Ok(Err(UnopenedReason::UnopenableAlias)),
        }
    }
}

/// impl を 1 つに選べなかったときに、その理由を決める材料。
#[derive(Debug, Default)]
struct UnselectedImpls {
    /// 候補側の型名を辿れなかった最初の理由。
    untraced: Option<UnopenedReason>,
    /// 候補のファイルを読めなかった、またはドキュメントにできなかったか。
    has_unreadable_file: bool,
    /// implementation が返した場所に、パスとして読めない URI があったか。
    has_unreadable_site: bool,
    /// implementation が返した場所に、候補として読めない impl（境界・属性・マクロ生成など）が
    /// あったか。
    has_skipped_impl: bool,
}

impl UnselectedImpls {
    /// 一致した候補の数から理由を決める。
    ///
    /// **2 つ以上一致したら、材料に関わらず `UnresolvedAssociatedType`。** coherence が
    /// 成り立つなら起きない形で、辿れなかった候補を直しても 1 つに決まらない。
    /// **0 件なら、照合まで進めたが辿れなかった候補を先に出す。** 読めないファイルは
    /// 対象型の名前で絞る前に数えるので、無関係な impl のファイルも含みうる。
    /// implementation が返した impl を、すべて読んで照合し終えたか。
    ///
    /// 対象型の末尾の名前で飛ばした候補は照合し終えた側に数える（別名で書いた本物の候補を
    /// 取りこぼす緩みは残る）。
    fn is_complete(&self) -> bool {
        let incomplete = self.untraced.is_some()
            || self.has_unreadable_file
            || self.has_unreadable_site
            || self.has_skipped_impl;
        !incomplete
    }

    fn reason_with(self, matched: usize) -> UnopenedReason {
        if matched > 0 {
            return UnopenedReason::UnresolvedAssociatedType;
        }
        if let Some(reason) = self.untraced {
            return reason;
        }
        if self.has_unreadable_file {
            return UnopenedReason::UnreadableDeclaringDocument;
        }
        if self.has_unreadable_site {
            return UnopenedReason::UnreadableImplementation;
        }
        UnopenedReason::UnresolvedAssociatedType
    }
}

/// 候補 impl の型名の組を、使用側と照合した結果。
enum NamesAgree {
    /// すべての組が同じ型を指した。
    Yes,
    /// 違う型を指す組があった。
    No,
    /// 候補側の型名を辿れなかった。他の候補が一致すれば選べるので、理由だけ覚える。
    Untraced(UnopenedReason),
    /// 使用側の型名を辿れなかった。どの候補とも照合できないので、その理由で止まる。
    UseSiteUntraced(UnopenedReason),
}

/// 右辺のプリミティブ表記が、その宣言側で同じプリミティブを指すか確認する。
/// 型や import で shadow されていたら、展開できない理由を返す。
///
/// # Errors
///
/// ドキュメントの通知・hover の往復が失敗したとき。
fn rust_alias_unopened_reason(
    session: &mut Session,
    declaration: &TypeDeclaration,
    source: &str,
    alias: &RustTypeResolution,
) -> Result<Option<UnopenedReason>, ClientError> {
    let Some(primitives) =
        primitive_references_of_alias(source, declaration.site().position(), alias)
    else {
        return Ok(Some(UnopenedReason::UnopenableAlias));
    };
    let Ok(document) = SourceDocument::new(declaration.site().path(), source.to_owned()) else {
        return Ok(Some(UnopenedReason::UnreadableDeclaringDocument));
    };
    session.open_document(&document)?;
    for primitive in primitives {
        let spelling = match declared_spelling_of(session.hover(&document, primitive.position())?) {
            Ok(spelling) => spelling,
            Err(reason) => return Ok(Some(reason)),
        };
        // typeDefinition は別のプリミティブへのエイリアスにも空を返すため、
        // 右辺の名前そのものへの hover で、元のプリミティブ表記と一致するか見る。
        if spelling.as_str() != primitive.name() {
            return Ok(Some(UnopenedReason::UnopenableAlias));
        }
    }
    Ok(None)
}

/// 宣言の場所を尋ねる問い合わせ。
///
/// **届かなかった理由の語彙を問い合わせと一緒に持つ。** 尋ねた問い合わせと
/// 別の名前で「提供していない」と出すと、利用者が確かめる相手を取り違える。
#[derive(Debug, Clone, Copy)]
enum DeclarationQuery {
    /// `textDocument/typeDefinition`。TypeScript で使う。
    TypeDefinition,
    /// `textDocument/definition`。Rust で使う。
    Definition,
}

impl DeclarationQuery {
    /// その位置の宣言の場所を尋ねる。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき。
    fn ask(
        self,
        session: &mut Session,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<DeclarationSiteOutcome, ClientError> {
        match self {
            Self::TypeDefinition => session.type_definition(document, position),
            Self::Definition => session.definition(document, position),
        }
    }

    /// その位置の宣言の場所を尋ね、届かなければこの問い合わせの理由を返す。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき。
    fn site_of(
        self,
        session: &mut Session,
        document: &SourceDocument,
        position: SourcePosition,
    ) -> Result<Result<DeclarationSite, UnopenedReason>, ClientError> {
        Ok(match self.ask(session, document, position)? {
            DeclarationSiteOutcome::Answered(site) => Ok(site),
            DeclarationSiteOutcome::NoAnswer => Err(self.no_answer()),
            DeclarationSiteOutcome::Unreadable { .. } => Err(self.unreadable()),
            DeclarationSiteOutcome::NotSupported => Err(self.not_provided()),
        })
    }

    /// サーバが宣言の場所を答えなかったときの理由。
    fn no_answer(self) -> UnopenedReason {
        match self {
            Self::TypeDefinition => UnopenedReason::NoDeclarationSite,
            Self::Definition => UnopenedReason::NoDefinitionSite,
        }
    }

    /// サーバがこの問い合わせを提供していなかったときの理由。
    fn not_provided(self) -> UnopenedReason {
        match self {
            Self::TypeDefinition => UnopenedReason::TypeDefinitionNotProvided,
            Self::Definition => UnopenedReason::DefinitionNotProvided,
        }
    }

    /// 返った場所をパスとして読めなかったときの理由。
    fn unreadable(self) -> UnopenedReason {
        match self {
            Self::TypeDefinition => UnopenedReason::UnreadableTypeDefinition,
            Self::Definition => UnopenedReason::UnreadableDefinition,
        }
    }

    /// 型名 1 つずつにこの問い合わせを尋ね、宣言の場所が取れたものと届かなかったものに分ける。
    ///
    /// # Errors
    ///
    /// そのドキュメントを開かせていないとき、往復が失敗したとき。
    fn traced_type_names_of(
        self,
        session: &mut Session,
        document: &SourceDocument,
        type_references: &[TypeReference],
    ) -> Result<TracedTypeNames, ClientError> {
        let mut declared = Vec::new();
        let mut unopened = Vec::new();

        for reference in type_references {
            let name = reference.name().to_owned();

            match self.ask(session, document, reference.position())? {
                DeclarationSiteOutcome::Answered(site) => {
                    declared.push(TypeDeclaration::new(name, site));
                }
                DeclarationSiteOutcome::NoAnswer => {
                    unopened.push(UnopenedTypeName::new(name, self.no_answer()));
                }
                DeclarationSiteOutcome::Unreadable { .. } => {
                    unopened.push(UnopenedTypeName::new(name, self.unreadable()));
                }
                DeclarationSiteOutcome::NotSupported => {
                    unopened.push(UnopenedTypeName::new(name, self.not_provided()));
                }
            }
        }

        Ok(TracedTypeNames::new(declared, unopened))
    }
}

/// 宣言の位置へ hover を送り、型エイリアスの右辺を開く。
///
/// `traced` は [`traced_type_names_of`] が返した結果。**宣言のファイルは先に
/// 呼び出し側が開かせておく**（開かせていないファイルへの hover は綴りを持たない
/// 応答になる）。
///
/// **エイリアスでなかった型名は落とす。** `interface` / `class` に右辺が無いのは
/// **サーバの答え**で、開けなかったのとは別物（綴りのまま比べてよい）。
/// **右辺はあるのに差し込めなかった型名は落とさない**（[`DeclaredAlias`]）。
///
/// # Errors
///
/// 往復が失敗したとき。
pub fn opened_type_names_of(
    session: &mut Session,
    traced: TracedTypeNames,
) -> Result<TracedTypeNames, ClientError> {
    opened_type_names_with(session, traced, declared_type_of)
}

/// 言語ごとの宣言の読み方を使い、開けた綴りと開けなかった理由を残す。
///
/// # Errors
///
/// 宣言 hover の往復が失敗したとき。
fn opened_type_names_with(
    session: &mut Session,
    traced: TracedTypeNames,
    type_of: impl Fn(&str) -> DeclaredAlias,
) -> Result<TracedTypeNames, ClientError> {
    let mut resolutions = Vec::new();
    let mut unopened = Vec::new();

    for declaration in traced.declared() {
        let name = declaration.name().to_owned();

        let declared = match declared_spelling_of(session.hover_at_declaration(declaration.site())?)
        {
            Ok(declared) => declared,
            Err(reason) => {
                unopened.push(UnopenedTypeName::new(name, reason));
                continue;
            }
        };

        match type_of(declared.as_str()) {
            DeclaredAlias::Opened(resolved) => resolutions.push((name, resolved)),
            // 開く先が無いのはサーバの答え。綴りのまま比べてよい。
            DeclaredAlias::NotAnAlias => continue,
            DeclaredAlias::Unopenable => {
                unopened.push(UnopenedTypeName::new(name, UnopenedReason::UnopenableAlias))
            }
        }
    }

    Ok(traced
        .with_resolved(ResolvedTypes::new(resolutions))
        .with_unopened(unopened))
}

/// 宣言側の hover 応答を綴りか、開けなかった理由に分ける。
fn declared_spelling_of(outcome: HoverOutcome) -> Result<SignatureText, UnopenedReason> {
    match outcome {
        HoverOutcome::Answered(spelling) => Ok(spelling),
        HoverOutcome::NoAnswer => Err(UnopenedReason::NoSpellingAtDeclaration),
        HoverOutcome::Unreadable => Err(UnopenedReason::UnreadableDeclarationHover),
        HoverOutcome::ServerStillWorking => Err(UnopenedReason::ServerStillWorking),
        HoverOutcome::NotSupported => Err(UnopenedReason::HoverNotProvided),
    }
}

/// 宣言の綴りを読んだ結果。
///
/// **「エイリアスではない」と「エイリアスだが開けない」を分ける。** 前者は
/// **開く先が無いというサーバの答え**なので綴りのまま比べてよく、後者は
/// **サーバは右辺を返しているのに dryguard が差し込めていない**ので、
/// 綴りのまま比べた結果を答えにできない
/// (`rules/architecture.md`「どこまでを「取れなかった」に数えるか」)。
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclaredAlias {
    /// 型エイリアスで、差し込める右辺があった。
    Opened(String),
    /// 型エイリアスではなかった（`interface` / `class` / `enum`）。
    NotAnAlias,
    /// 型エイリアスだが、右辺を差し込める形にできなかった。
    Unopenable,
}

/// hover が返した宣言の綴りから、その名前が指す型の綴りを読む。
///
/// `declared` は宣言の位置へ hover を送って返った綴り（`type Amount = number` /
/// `interface User` / `class Invoice`）。
///
/// **型引数を取るエイリアスは開かない。** `type Box<T> = { value: T }` の右辺を
/// `Box<string>` の位置へ差し込むと `{ value: T }<string>` になり、綴りとして壊れる。
/// 型引数の当てはめには型そのものの構文解析が要る。**開かないだけで、開く先はある**ので
/// [`DeclaredAlias::Unopenable`] を返す。
///
/// **1 段しか開かない。** `type A = B` の右辺は `B` のままになる。2 段目を開くには
/// `B` が書かれている位置が要り、それは宣言のあるファイルの中にあるので、
/// **もう一度そのファイルを構文木にするところから始まる**。
fn declared_type_of(declared: &str) -> DeclaredAlias {
    let Some(aliased) = declared.trim().strip_prefix(ALIAS_KEYWORD) else {
        return DeclaredAlias::NotAnAlias;
    };
    // ここから先は `type` の宣言。右辺を読み取れなくても、**開く先が無いのとは別**。
    let Some((name, resolved)) = aliased.split_once(ALIAS_MARKER) else {
        return DeclaredAlias::Unopenable;
    };

    if name.contains(TYPE_ARGUMENTS_START) {
        return DeclaredAlias::Unopenable;
    }

    let resolved = resolved.trim();
    if resolved.is_empty() {
        return DeclaredAlias::Unopenable;
    }

    DeclaredAlias::Opened(resolved.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unselected_impls_prefer_the_untraced_candidate_over_unreadable_files() {
        let unselected = UnselectedImpls {
            untraced: Some(UnopenedReason::NoDefinitionSite),
            has_unreadable_file: true,
            has_unreadable_site: true,
            has_skipped_impl: true,
        };
        assert_eq!(unselected.reason_with(0), UnopenedReason::NoDefinitionSite);
    }

    #[test]
    fn test_unselected_impls_report_unreadable_files_before_unreadable_sites() {
        let unselected = UnselectedImpls {
            has_unreadable_file: true,
            has_unreadable_site: true,
            ..UnselectedImpls::default()
        };
        assert_eq!(
            unselected.reason_with(0),
            UnopenedReason::UnreadableDeclaringDocument
        );
        let unselected = UnselectedImpls {
            has_unreadable_site: true,
            ..UnselectedImpls::default()
        };
        assert_eq!(
            unselected.reason_with(0),
            UnopenedReason::UnreadableImplementation
        );
    }

    #[test]
    fn test_unselected_impls_are_complete_only_without_any_impl_left_unmatched() {
        assert!(UnselectedImpls::default().is_complete(), "対照");
        for incomplete in [
            UnselectedImpls {
                untraced: Some(UnopenedReason::UnopenableAlias),
                ..UnselectedImpls::default()
            },
            UnselectedImpls {
                has_unreadable_file: true,
                ..UnselectedImpls::default()
            },
            UnselectedImpls {
                has_unreadable_site: true,
                ..UnselectedImpls::default()
            },
            UnselectedImpls {
                has_skipped_impl: true,
                ..UnselectedImpls::default()
            },
        ] {
            assert!(!incomplete.is_complete(), "{incomplete:?}");
        }
    }

    #[test]
    fn test_unselected_impls_with_two_matches_are_unresolved_even_with_untraced_candidates() {
        let unselected = UnselectedImpls {
            untraced: Some(UnopenedReason::NoDefinitionSite),
            ..UnselectedImpls::default()
        };
        assert_eq!(
            unselected.reason_with(2),
            UnopenedReason::UnresolvedAssociatedType
        );
        assert_eq!(
            UnselectedImpls::default().reason_with(0),
            UnopenedReason::UnresolvedAssociatedType
        );
    }

    #[test]
    fn test_declared_type_of_an_alias_declaration_is_the_spelling_on_its_right() {
        assert_eq!(
            declared_type_of("type Amount = number"),
            DeclaredAlias::Opened("number".to_owned())
        );
    }

    #[test]
    fn test_declared_type_of_an_alias_of_a_function_type_keeps_the_whole_type() {
        // `=>` を右辺の区切りと取り違えると、`(value: string)` だけが残る
        assert_eq!(
            declared_type_of("type Handler = (value: string) => number"),
            DeclaredAlias::Opened("(value: string) => number".to_owned())
        );
    }

    #[test]
    fn test_declared_type_of_an_alias_written_over_lines_keeps_every_line() {
        // オブジェクト型はサーバが複数行に展開して返す。1 行目で打ち切ると型が変わる
        assert_eq!(
            declared_type_of("type Shape = {\n    id: string;\n}"),
            DeclaredAlias::Opened("{\n    id: string;\n}".to_owned())
        );
    }

    #[test]
    fn test_declared_type_of_an_interface_declaration_is_not_read_as_an_alias() {
        // 対照は最初のテスト。`interface` には右辺が無いので、開く先が無い
        assert_eq!(
            declared_type_of("interface User"),
            DeclaredAlias::NotAnAlias
        );
    }

    #[test]
    fn test_declared_type_of_a_class_declaration_is_not_read_as_an_alias() {
        assert_eq!(declared_type_of("class Invoice"), DeclaredAlias::NotAnAlias);
    }

    #[test]
    fn test_declared_type_of_an_alias_taking_type_arguments_cannot_be_opened() {
        // 対照は 1 つ上のテスト（`interface`）。**右辺はある**ので、開けなかったのと
        // 開く先が無いのを同じにすると、綴りのまま比べた結果を答えとして出してしまう。
        // 右辺を `Box<string>` の位置へ差し込むと `{ value: T; }<string>` になるので、
        // 型引数の当てはめには型そのものの構文解析が要る
        assert_eq!(
            declared_type_of("type Box<T> = { value: T; }"),
            DeclaredAlias::Unopenable
        );
    }

    #[test]
    fn test_declared_type_of_a_name_that_only_starts_with_the_keyword_is_not_read_as_an_alias() {
        // `type` を語として見ないと、`typeof` で始まる綴りを開いてしまう
        assert_eq!(
            declared_type_of("typeof rate = number"),
            DeclaredAlias::NotAnAlias
        );
    }

    #[test]
    fn test_declared_type_of_an_alias_without_a_right_hand_side_cannot_be_opened() {
        // 空の綴りを解決結果にすると、差し込んだ先の型が消える。**エイリアスではある**ので、
        // 綴りのまま比べてよい `interface` とは分ける
        assert_eq!(
            declared_type_of("type Amount = "),
            DeclaredAlias::Unopenable
        );
    }

    #[test]
    fn test_resolved_types_answer_only_for_the_names_they_were_given() {
        let resolved = ResolvedTypes::new([("Amount".to_owned(), "number".to_owned())]);

        assert_eq!(resolved.resolved_of("Amount"), Some("number"));
        assert_eq!(resolved.resolved_of("Total"), None);
    }
}
