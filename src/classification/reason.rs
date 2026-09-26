//! 判定を組み上げた根拠。
//!
//! 判定と一緒に**どのシグナルがどちらへ傾けたか**を返すのは、説明可能性が
//! このツールの価値そのものだから（`docs/dryguard-plan.md`「差別化ポイント」）。
//! シグナルの値だけを並べても、読む側はそれが判定にどう効いたのかを再現できない。

use crate::classification::signal::{
    CalleeDomainOverlap, CallerDomainOverlap, ImportOverlap, StructuralSimilarity,
    TypeSignatureMatch,
};
use crate::syntax::module_distance::ModuleDistance;
use crate::threshold::Threshold;

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

/// 判定を傾けた根拠 1 件。シグナルの値と、**それに当てた閾値**と、その向きを組で持つ。
///
/// 値と向きを別々の入れ物にしないのは、**どの値がどちらへ効いたか**が対応を失うため。
/// シグナルの一覧と傾きの一覧を突き合わせるのは読む側の仕事ではない。
///
/// **閾値も同じ組に入れる。** 値と向きだけでは、**閾値を知らない読者は傾きを再現できない**
/// （`--explain` が出す根拠を全部足せば判定を再現できる、という性質。
/// `rules/architecture.md`「判定は 1 箇所にだけ置く」）。
///
/// **Why not（出力側に定数を読ませる）**: どのシグナルにどの閾値を当てたかの対応が
/// 判定側と出力側の 2 箇所に載り、判定側だけ差し替えたときに `--explain` が
/// **当てていない閾値**を出す。組にしておくと、その取り違えが書けない。
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// 構造の似かたが傾けた。
    StructuralSimilarity {
        signal: StructuralSimilarity,
        /// 構造が似ていると見なした下限。**`--threshold` で動く**唯一の閾値。
        threshold: Threshold,
        lean: Lean,
    },
    /// 依存先の重なりが傾けた。
    ImportOverlap {
        signal: ImportOverlap,
        /// 依存先を共有していると見なした下限。
        threshold: Threshold,
        lean: Lean,
    },
    /// ディレクトリの隔たりが傾けた。
    ModuleDistance {
        signal: ModuleDistance,
        /// 別のディレクトリへ下りていると見なした段数。
        ///
        /// **[`ModuleDistance`] で持たない。** あちらは 2 つのファイルの間を測った結果で、
        /// こちらは測っていない境目。同じ型にすると、**測っていない値を測った結果として
        /// 渡せてしまう**（`rules/coding.md`「不正な状態を型で表現できなくする」）。
        separate_directory_steps: usize,
        lean: Lean,
    },
    /// 型シグネチャの単一化の可否が傾けた。
    ///
    /// **閾値を持たない。** 単一化できるかは重なりの量ではなく可否なので、当てる境目が無い。
    TypeSignatureMatch {
        signal: TypeSignatureMatch,
        lean: Lean,
    },
    /// 呼び出し元ドメインの重なりが傾けた。
    CallerDomainOverlap {
        signal: CallerDomainOverlap,
        /// 呼び出し元を共有していると見なした下限。
        threshold: Threshold,
        lean: Lean,
    },
    /// 呼び出し先ドメインの重なりが傾けた。
    CalleeDomainOverlap {
        signal: CalleeDomainOverlap,
        /// 呼び出し先を共有していると見なした下限。**外から動かせない**
        /// （`classification::SHARED_CALLEE_DOMAINS_THRESHOLD`）。
        threshold: Threshold,
        lean: Lean,
    },
}
