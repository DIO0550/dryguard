//! シグナルを統合して判定する層（計画の Stage 3: 分類）。
//!
//! **このツールの価値がある層。** 判定と同時に、どのシグナルがどちらへ傾けたかを返す
//! （`docs/dryguard-plan.md`「差別化ポイント」）。
//!
//! I/O も LSP の呼び出しも持たない。意味情報はシグナルとして受け取るので、
//! LSP が使えない環境でも判定できる（`rules/architecture.md`「依存方向のルール」）。

pub mod reason;
pub mod signal;
pub mod verdict;

use crate::classification::reason::{Lean, Reason};
use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
use crate::classification::verdict::Verdict;
use crate::syntax::module_distance::ModuleDistance;
use crate::threshold::Threshold;

/// 構造が似ていると見なす類似度の下限。`--threshold` が無いときに使う。
///
/// **この値は構造類似度の測り方とセットでしか意味を持たない。** 測り方を変えたら、
/// ここも測り直して決める（`tests/corpus/` の全ペアで旧実装と突き合わせる）。
///
/// Why（0.50）: 正規化トークン列の 3-gram で測ると、`tests/corpus/` の 1081 ペアの
/// うち 65 ペアがこの値を超える。**素朴な集合 Jaccard で 0.85 を超えていたのと同じ本数**で、
/// 測り方を差し替えても候補に上げる網の細かさが変わらない。
///
/// Why not（0.85 のまま）: 集合 Jaccard は順序も出現回数も落としていたので上位が
/// 1.00 付近に潰れており、0.85 はその分布に対して選んだ値だった。並びを見る測り方では
/// 同じ 0.85 が 4 倍近く厳しくなり、Phase 0 で検出できていた真陽性が消える。
///
/// Phase 3 まで設定ファイルへ出さない。**先回りで外に出すと、まだ意味の分かっていない
/// つまみが増える**（`docs/dryguard-plan.md`「Phase 3: Stage 3 を厚くする」）。
pub const DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD: Threshold = Threshold::from_literal(0.50);

/// 依存先を共有していると見なす重なりの下限。
///
/// 半分以上が共通なら、同じ道具立ての上に書かれていると見る。
const SHARED_IMPORTS_THRESHOLD: Threshold = Threshold::from_literal(0.5);

/// 別のディレクトリへ下りていると見なす段数。
///
/// 1 段は片方がもう片方のディレクトリの下にある形なので、同じドメインの下位と見る。
/// 2 段になって初めて、双方が共通の親から別のディレクトリへ下りている。
const SEPARATE_DIRECTORY_STEPS: usize = 2;

/// 判定と、その根拠。
///
/// ラベルだけを返さないのは、**根拠が付かない判定はこのツールの価値を満たさない**ため
/// (`docs/dryguard-plan.md`「差別化ポイント」)。
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    verdict: Verdict,
    reasons: Vec<Reason>,
}

impl Classification {
    /// 判定の結果。
    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    /// 判定を傾けた根拠。シグナル 1 つにつき 1 件、測った順に並ぶ。
    pub fn reasons(&self) -> &[Reason] {
        &self.reasons
    }
}

/// シグナルを統合して判定する。
///
/// `structural_similarity_threshold` は構造が似ていると見なす下限で、
/// `--threshold` が指定されなければ [`DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD`]。
///
/// **シグナルからラベルへの決定木はここにしか無い**
/// (`rules/architecture.md`「判定は 1 箇所にだけ置く」)。
pub fn classification_of(
    signals: &Signals,
    structural_similarity_threshold: Threshold,
) -> Classification {
    let structurally_similar = is_structurally_similar(
        signals.structural_similarity(),
        structural_similarity_threshold,
    );
    let domain = domain_of(signals);

    Classification {
        verdict: verdict_of(structurally_similar, domain),
        reasons: reasons_of(signals, structural_similarity_threshold),
    }
}

/// 構造が似ているかと、依存ドメインが一致しているかから、ラベルを決める。
///
/// 構造が似ていないペアは、ドメインが食い違っていても `DO-NOT-EXTRACT` にしない。
/// **偶発的な重複と呼べるのは、そもそも共通化したくなるほど似ている場合だけ。**
fn verdict_of(structurally_similar: bool, domain: Domain) -> Verdict {
    if !structurally_similar {
        return Verdict::Review;
    }

    match domain {
        Domain::Same => Verdict::ExtractCandidate,
        Domain::Separate => Verdict::DoNotExtract,
        Domain::Undecidable => Verdict::Review,
    }
}

/// 2 つのチャンクが同じドメインに属しているか。
///
/// `classification` の中だけの中間の値なので公開しない。外へ出すのは
/// ラベルと根拠で、その間の畳み方は判定の内側にある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    /// 同じドメイン。
    Same,
    /// 別のドメイン。
    Separate,
    /// どちらとも言えない。
    Undecidable,
}

/// 依存先の重なりとディレクトリの隔たりから、ドメインが同じかを決める。
///
/// **根拠が持つ傾きをそのまま材料にする。** 同じ条件を判定用にもう一度書くと、
/// `--explain` が出す根拠と実際の判定が食い違いうる
/// (`rules/architecture.md`「判定は 1 箇所にだけ置く」)。
///
/// 重なりが十分なら隔たりを見ない。同じ道具立てに依存しているなら、ディレクトリが
/// 分かれていても偶発的な重複ではない（`utils/formatDate.ts` と `report/dateHelper.ts` が
/// 計画の出力イメージで `EXTRACT-CANDIDATE` なのがこれ）。
///
/// 逆に**別のドメインと言うには隔たりも要る**。ディレクトリはドメイン境界の代理指標
/// でしかないので、同じディレクトリにあるものを依存先の違いだけで別ドメインと呼ばない。
fn domain_of(signals: &Signals) -> Domain {
    match import_overlap_lean_of(signals.import_overlap()) {
        Lean::TowardExtract => Domain::Same,
        Lean::Neither => Domain::Undecidable,
        Lean::TowardDoNotExtract => match module_distance_lean_of(signals.module_distance()) {
            Lean::TowardDoNotExtract => Domain::Separate,
            Lean::TowardExtract => Domain::Undecidable,
            Lean::Neither => Domain::Undecidable,
        },
    }
}

/// 構造類似度が閾値に届いているか。測れていなければ届いていない扱いにする。
fn is_structurally_similar(signal: StructuralSimilarity, threshold: Threshold) -> bool {
    match signal {
        StructuralSimilarity::Measured(similarity) => similarity.is_at_least(threshold),
        StructuralSimilarity::NoTokens => false,
    }
}

/// シグナル 1 つずつを、値と傾きの組にする。
fn reasons_of(signals: &Signals, structural_similarity_threshold: Threshold) -> Vec<Reason> {
    vec![
        Reason::StructuralSimilarity {
            signal: signals.structural_similarity(),
            lean: structural_similarity_lean_of(
                signals.structural_similarity(),
                structural_similarity_threshold,
            ),
        },
        Reason::ImportOverlap {
            signal: signals.import_overlap(),
            lean: import_overlap_lean_of(signals.import_overlap()),
        },
        Reason::ModuleDistance {
            signal: signals.module_distance(),
            lean: module_distance_lean_of(signals.module_distance()),
        },
    ]
}

/// 構造類似度が傾けた向き。
///
/// 閾値未満を `TowardDoNotExtract` にしない。**似ていないことは「共通化するな」ではなく、
/// そもそも候補でないという別の話**で、混ぜると `--explain` が「偶発的な重複だから
/// 共通化しない」と読める根拠を出してしまう。
fn structural_similarity_lean_of(signal: StructuralSimilarity, threshold: Threshold) -> Lean {
    if is_structurally_similar(signal, threshold) {
        return Lean::TowardExtract;
    }
    Lean::Neither
}

/// 依存先の重なりが傾けた向き。測れなければどちらへも傾けない。
fn import_overlap_lean_of(signal: ImportOverlap) -> Lean {
    let ImportOverlap::Measured(overlap) = signal else {
        return Lean::Neither;
    };

    if overlap.is_at_least(SHARED_IMPORTS_THRESHOLD) {
        return Lean::TowardExtract;
    }
    Lean::TowardDoNotExtract
}

/// ディレクトリの隔たりが傾けた向き。
///
/// 段数は必ず取れるので `Neither` にはならない。
fn module_distance_lean_of(distance: ModuleDistance) -> Lean {
    let in_separate_directories = distance.steps() >= SEPARATE_DIRECTORY_STEPS;

    if in_separate_directories {
        return Lean::TowardDoNotExtract;
    }
    Lean::TowardExtract
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::classification::reason::{Lean, Reason};
    use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
    use crate::classification::verdict::Verdict;
    use crate::similarity::Similarity;
    use crate::syntax::module_distance::ModuleDistance;
    use crate::threshold::Threshold;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// 別のディレクトリにある 2 ファイルの隔たり（2 段）。
    fn separate_directories() -> ModuleDistance {
        ModuleDistance::between(
            Path::new("src/billing/discount.ts"),
            Path::new("src/inventory/reorder.ts"),
        )
    }

    /// 同じディレクトリにある 2 ファイルの隔たり（0 段）。
    fn same_directory() -> ModuleDistance {
        ModuleDistance::between(
            Path::new("src/billing/discount.ts"),
            Path::new("src/billing/invoice.ts"),
        )
    }

    fn leans(classification: &Classification, expected: &Reason) -> bool {
        classification.reasons().contains(expected)
    }

    #[test]
    fn test_classification_of_similar_chunks_sharing_dependencies_is_extract_candidate() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(1.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
    }

    #[test]
    fn test_classification_of_similar_chunks_in_separate_directories_without_shared_dependencies_is_do_not_extract()
     {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::DoNotExtract);
    }

    #[test]
    fn test_classification_of_similar_chunks_in_the_same_directory_without_shared_dependencies_is_review()
     {
        // 依存先が食い違っていても、同じディレクトリなら別ドメインとは言えない。
        // 上のテストとの違いはディレクトリだけで、そちらは DO-NOT-EXTRACT になる
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            same_directory(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_of_chunks_below_the_threshold_is_review() {
        // ドメインは不一致にしてある。構造が似ていないだけで DO-NOT-EXTRACT に
        // 倒れないことを見る（似ていないペアは、そもそも共通化の候補ではない）
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.2)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_at_the_threshold_is_extract_candidate() {
        // 境界。閾値ちょうどを候補から外すと、指定した閾値そのものが判定に出てこない
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(
                DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD.value(),
            )),
            ImportOverlap::Measured(measured(1.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
    }

    #[test]
    fn test_classification_with_a_lowered_threshold_reaches_a_verdict_the_default_would_not() {
        // 既定では届かない類似度を選ぶ。既定と同じ答えになる入力では、
        // 渡した閾値が使われたのか既定が使われたのかが分からない
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.3)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, Threshold::from_literal(0.2));

        assert_eq!(classification.verdict(), Verdict::DoNotExtract);
    }

    #[test]
    fn test_classification_without_structural_similarity_is_review() {
        // ドメインは不一致。構造類似度が取れないことだけで REVIEW になる
        let signals = Signals::new(
            StructuralSimilarity::NoTokens,
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_without_imports_is_review() {
        // 構造は十分似ていて、距離も離れている。import が取れないことだけで
        // ドメインの一致 / 不一致を決めない
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::NoImports,
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_reports_one_reason_for_every_signal() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(1.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.reasons().len(), 3);
    }

    #[test]
    fn test_classification_of_disjoint_imports_leans_the_import_reason_toward_do_not_extract() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::ImportOverlap {
                    signal: ImportOverlap::Measured(measured(0.0)),
                    lean: Lean::TowardDoNotExtract,
                }
            ),
            "重なりが無いことが共通化しない側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_shared_imports_leans_the_import_reason_toward_extract() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(1.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::ImportOverlap {
                    signal: ImportOverlap::Measured(measured(1.0)),
                    lean: Lean::TowardExtract,
                }
            ),
            "同じ依存先を共有していることが候補側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_below_the_threshold_leans_the_similarity_reason_neither_way() {
        // 似ていないことは「共通化するな」ではない。TowardDoNotExtract に倒すと、
        // --explain が「偶発的重複だから共通化しない」と読める根拠を出してしまう
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.2)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::StructuralSimilarity {
                    signal: StructuralSimilarity::Measured(measured(0.2)),
                    lean: Lean::Neither,
                }
            ),
            "閾値未満の構造類似度はどちらへも傾けない: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_far_modules_leans_the_distance_reason_toward_do_not_extract() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::ModuleDistance {
                    signal: separate_directories(),
                    lean: Lean::TowardDoNotExtract,
                }
            ),
            "別のディレクトリにあることが共通化しない側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_modules_in_the_same_directory_leans_the_distance_reason_toward_extract()
     {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            same_directory(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::ModuleDistance {
                    signal: same_directory(),
                    lean: Lean::TowardExtract,
                }
            ),
            "同じディレクトリにあることが候補側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_without_imports_leans_the_import_reason_neither_way() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::NoImports,
            separate_directories(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::ImportOverlap {
                    signal: ImportOverlap::NoImports,
                    lean: Lean::Neither,
                }
            ),
            "測れなかったシグナルはどちらへも傾けない: {:?}",
            classification.reasons()
        );
    }
}
