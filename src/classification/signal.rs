//! 判定の材料。
//!
//! 測れなかったことを 0.0 や「中立」で埋めず、**取れなかった理由をバリアントに出す**
//! (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
//! 埋めてしまうと、後段も読者も「そういう値だった」と「見ていない」を区別できない。

use crate::similarity::Similarity;
use crate::syntax::module_distance::ModuleDistance;

/// 1 つのペアについて測ったシグナル一式。
///
/// Phase 0 のシグナルはこの 3 つ。型シグネチャと呼び出し先 / 呼び出し元は
/// Stage 2（LSP）が入ってから足す（`docs/dryguard-plan.md`「Stage 3: 分類」）。
#[derive(Debug, Clone, PartialEq)]
pub struct Signals {
    structural_similarity: StructuralSimilarity,
    import_overlap: ImportOverlap,
    module_distance: ModuleDistance,
}

impl Signals {
    /// 測った 3 つのシグナルをまとめる。
    pub fn new(
        structural_similarity: StructuralSimilarity,
        import_overlap: ImportOverlap,
        module_distance: ModuleDistance,
    ) -> Self {
        Self {
            structural_similarity,
            import_overlap,
            module_distance,
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
