//! 判定の材料。
//!
//! 測れなかったことを 0.0 や「中立」で埋めず、**取れなかった理由をバリアントに出す**
//! (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
//! 埋めてしまうと、後段も読者も「そういう値だった」と「見ていない」を区別できない。

use crate::semantics::caller_domain::CallerDomains;
use crate::similarity::Similarity;
use crate::syntax::module_distance::ModuleDistance;

/// 1 つのペアについて測ったシグナル一式。
///
/// Stage 1（`syntax`）が採る 3 つと、Stage 2（`semantics`）が採る 2 つ。
/// 呼び出し先（callHierarchy）は Phase 3 で足す
/// （`docs/dryguard-plan.md`「Stage 3: 分類」）。
#[derive(Debug, Clone, PartialEq)]
pub struct Signals {
    structural_similarity: StructuralSimilarity,
    import_overlap: ImportOverlap,
    module_distance: ModuleDistance,
    type_signature_match: TypeSignatureMatch,
    caller_domain_overlap: CallerDomainOverlap,
}

impl Signals {
    /// Stage 1 で測った 3 つのシグナルをまとめる。
    ///
    /// Stage 2 の 2 つは「LSP に尋ねていない」になる。**これは既定値ではなく、
    /// 尋ねていないという実際の状態**で、尋ねようとして届かなかった
    /// （[`SemanticsUnavailable::LspUnusable`] など）とは別物
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    pub fn new(
        structural_similarity: StructuralSimilarity,
        import_overlap: ImportOverlap,
        module_distance: ModuleDistance,
    ) -> Self {
        Self {
            structural_similarity,
            import_overlap,
            module_distance,
            type_signature_match: TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            caller_domain_overlap: CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        }
    }

    /// Stage 2 で測ったシグナルを重ねる。
    ///
    /// 重ねる形にしているのは、**Stage 1 だけで判定できる状態を残す**ため
    /// (`rules/architecture.md`「依存方向のルール」)。
    pub fn with_semantics(
        self,
        type_signature_match: TypeSignatureMatch,
        caller_domain_overlap: CallerDomainOverlap,
    ) -> Self {
        Self {
            type_signature_match,
            caller_domain_overlap,
            ..self
        }
    }

    /// 正規化トークン集合で測った構造の似かた。
    pub fn structural_similarity(&self) -> StructuralSimilarity {
        self.structural_similarity
    }

    /// 2 つのチャンクが属するファイルの、依存先の重なり。
    pub fn import_overlap(&self) -> ImportOverlap {
        self.import_overlap
    }

    /// 2 つのファイルを隔てているディレクトリの段数。
    pub fn module_distance(&self) -> ModuleDistance {
        self.module_distance
    }

    /// 2 つのチャンクの型シグネチャが単一化できるか。
    pub fn type_signature_match(&self) -> TypeSignatureMatch {
        self.type_signature_match
    }

    /// 2 つのチャンクの呼び出し元ドメインの重なり。
    pub fn caller_domain_overlap(&self) -> &CallerDomainOverlap {
        &self.caller_domain_overlap
    }
}

/// 構造類似度のシグナル。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StructuralSimilarity {
    /// 測れた類似度。
    Measured(Similarity),
    /// どちらかのチャンクからトークンが 1 つも取れず、測れなかった。
    NoTokens,
}

/// 依存モジュールの重なりのシグナル。
///
/// **片側に import が無いときを 0.00 にしない。** 重なりが 0.00 なのは依存先が
/// 食い違っている証拠だが、片側に import が無いのは材料が無いだけで、
/// 同じ値にすると判定も読者も両者を区別できない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportOverlap {
    /// 測れた重なり。
    Measured(Similarity),
    /// どちらかのファイルに import が無く、測れなかった。
    NoImports,
}

/// Stage 2 へ届かなかった理由。
///
/// **どちらの Stage 2 シグナルも同じ理由で欠ける。** サーバに尋ねる前に止まるので、
/// 片方だけが取れることはない。1 つの型を両方が持つことで、理由を足したときに
/// 同じバリアントを 2 箇所へ書き足さずに済む。
///
/// **1 つにまとめない。** サーバを使えないのと、根を決められないのと、
/// 候補ペアでないから尋ねていないのとで**利用者が次にすることが違う**
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
/// `pipeline::SemanticsError` のバリアントと 1 対 1 で対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticsUnavailable {
    /// LSP に尋ねていない（Stage 1 のシグナルだけで組み立てた）。
    NotAsked,
    /// 構造が似ておらず候補ペアではないので尋ねなかった。
    ///
    /// 似ていないペアの判定は Stage 2 で変わらないので、**サーバを起こす前に降りる**
    /// (`docs/dryguard-plan.md`「候補ペアに対してだけ問い合わせる」)。
    NotACandidate,
    /// サーバに開かせる形にできなかった。
    DocumentUnopenable,
    /// ワークスペースの根を決められなかった。
    WorkspaceRootUndecidable,
    /// LSP サーバを使えなかった（起動できない / 握手できない / 会話が途切れた）。
    LspUnusable,
}

/// 型シグネチャが単一化できるかのシグナル（Stage 2）。
///
/// **取れなかった理由を 1 つにまとめない。** どれなのかで**利用者が次に試すことが違う**
/// （サーバを入れる / サーバを替える / そのチャンクを諦める / dryguard 側の穴）。
/// `lsp::HoverOutcome` が理由を分けて持っているのを、そのままここまで運ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeSignatureMatch {
    /// 2 つの型シグネチャが同じ型構造に重なる。
    Unifiable,
    /// 重ならない。
    NotUnifiable,
    /// サーバの答えまで届かなかった。
    Unavailable {
        /// 届かなかった理由。
        reason: SemanticsUnavailable,
    },
    /// どちらかのチャンクが名前を持たず、尋ねる位置を決められなかった。
    NoName,
    /// サーバがその位置に型を持たなかった。
    NoTypeThere,
    /// hover の応答を `lsp` が読めなかった。
    UnreadableHover,
    /// 綴りは返ったが、`semantics` が型シグネチャへ直せなかった。
    UnreadableSignature,
    /// サーバが hover を提供していない。
    HoverNotProvided,
    /// サーバが typeDefinition を提供していないので、書かれた型名を 1 つも開けなかった。
    TypeDefinitionNotProvided,
}

/// 呼び出し元ドメインの重なりのシグナル（Stage 2）。
///
/// **どこに置かれているか（`module distance`）とは別のシグナル。** こちらは
/// 実際に誰が使っているかで、置き場所の代理指標を置き換えるのではなく重ねる
/// (`rules/naming.md`「`module distance` と `caller domain` を混ぜない」)。
#[derive(Debug, Clone, PartialEq)]
pub enum CallerDomainOverlap {
    /// 両側の呼び出し元が取れた。
    Measured(MeasuredCallerDomains),
    /// サーバの答えまで届かなかった。
    Unavailable {
        /// 届かなかった理由。
        reason: SemanticsUnavailable,
    },
    /// どちらかのチャンクが名前を持たず、尋ねる位置を決められなかった。
    NoName,
    /// どちらかのチャンクに参照元が 1 件も返らなかった。
    NoReferences,
    /// 参照元は返ったが、パスとして読めない URI が混じっていた。
    UnreadableReferences,
    /// サーバが作業中で、落ち着いた答えを受け取れなかった。
    ServerStillWorking,
    /// サーバが references を提供していない。
    ReferencesNotProvided,
}

/// 両側の呼び出し元と、そこから出る重なり。
///
/// **重なりを別の値として持たない。** 持つと、呼び出し元と食い違う重なりを
/// 組み立てられてしまう（`rules/coding.md`「不正な状態を型で表現できなくする」）。
///
/// 件数まで持つのは、根拠の文が分布（`billing 3件 / inventory 5件`）を出すため
/// (`docs/dryguard-plan.md`「出力イメージ」)。重なりの値だけでは、
/// どちらに寄っているかを言えない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredCallerDomains {
    callers_a: CallerDomains,
    callers_b: CallerDomains,
}

impl MeasuredCallerDomains {
    /// 両側の呼び出し元からまとめる。
    pub fn new(callers_a: CallerDomains, callers_b: CallerDomains) -> Self {
        Self {
            callers_a,
            callers_b,
        }
    }

    /// 呼び出し元ドメイン集合の Jaccard 係数。
    pub fn overlap(&self) -> Similarity {
        self.callers_a.jaccard(&self.callers_b)
    }

    /// 先に列挙したほうのチャンクの呼び出し元。
    pub fn callers_a(&self) -> &CallerDomains {
        &self.callers_a
    }

    /// 後に列挙したほうのチャンクの呼び出し元。
    pub fn callers_b(&self) -> &CallerDomains {
        &self.callers_b
    }
}
