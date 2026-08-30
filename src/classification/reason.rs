//! 判定を組み上げた根拠。
//!
//! 判定と一緒に**どのシグナルがどちらへ傾けたか**を返すのは、説明可能性が
//! このツールの価値そのものだから（`docs/dryguard-plan.md`「差別化ポイント」）。
//! シグナルの値だけを並べても、読む側はそれが判定にどう効いたのかを再現できない。

use crate::classification::signal::{
    CallerDomainOverlap, ImportOverlap, StructuralSimilarity, TypeSignatureMatch,
};
use crate::syntax::module_distance::ModuleDistance;

/// シグナル 1 つが判定をどちらへ傾けたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lean {
    /// 共通化してよい側へ傾けた。
    TowardExtract,
    /// 共通化しない側へ傾けた。
    TowardDoNotExtract,
    /// どちらへも傾けなかった（測れなかった / 決め手にならなかった）。
    Neither,
}

/// 判定を傾けた根拠 1 件。シグナルの値と、その向きを組で持つ。
///
/// 値と向きを別々の入れ物にしないのは、**どの値がどちらへ効いたか**が対応を失うため。
/// シグナルの一覧と傾きの一覧を突き合わせるのは読む側の仕事ではない。
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// 構造の似かたが傾けた。
    StructuralSimilarity {
        signal: StructuralSimilarity,
        lean: Lean,
    },
    /// 依存先の重なりが傾けた。
    ImportOverlap { signal: ImportOverlap, lean: Lean },
    /// ディレクトリの隔たりが傾けた。
    ModuleDistance { signal: ModuleDistance, lean: Lean },
    /// 型シグネチャの単一化の可否が傾けた。
    TypeSignatureMatch {
        signal: TypeSignatureMatch,
        lean: Lean,
    },
    /// 呼び出し元ドメインの重なりが傾けた。
    CallerDomainOverlap {
        signal: CallerDomainOverlap,
        lean: Lean,
    },
}
