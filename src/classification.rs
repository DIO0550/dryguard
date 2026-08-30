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
use crate::classification::signal::{
    CallerDomainOverlap, ImportOverlap, Signals, StructuralSimilarity, TypeSignatureMatch,
};
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

/// 呼び出し元を共有していると見なす重なりの下限。
///
/// 半分以上のドメインが共通なら、同じ機能から使われていると見る。
///
/// **Why not（[`SHARED_IMPORTS_THRESHOLD`] を使い回す）**: 測っている集合が違う
/// （依存先モジュールと呼び出し元ドメイン）。片方を調整したときに、もう片方まで
/// 黙って動くのを避ける。
const SHARED_CALLER_DOMAINS_THRESHOLD: Threshold = Threshold::from_literal(0.5);

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
    let placement = placement_domain_match_of(signals);
    let domain_match = domain_match_of(placement, signals);
    let type_signature_lean = type_signature_lean_of(signals.type_signature_match());

    Classification {
        verdict: verdict_of(
            structurally_similar,
            type_signature_lean,
            placement,
            domain_match,
        ),
        reasons: reasons_of(signals, structural_similarity_threshold),
    }
}

/// 構造が似ているか・型シグネチャが重なるか・依存ドメインが一致しているかから、ラベルを決める。
///
/// 構造が似ていないペアは、ドメインが食い違っていても `DO-NOT-EXTRACT` にしない。
/// **偶発的な重複と呼べるのは、そもそも共通化したくなるほど似ている場合だけ。**
fn verdict_of(
    structurally_similar: bool,
    type_signature_lean: Lean,
    placement: DomainMatch,
    domain_match: DomainMatch,
) -> Verdict {
    if !structurally_similar {
        return Verdict::Review;
    }

    match domain_match {
        DomainMatch::Same => shared_domain_verdict_of(type_signature_lean, placement),
        DomainMatch::Separate => Verdict::DoNotExtract,
        DomainMatch::Undecidable => Verdict::Review,
    }
}

/// ドメインが一致しているペアのラベル。型シグネチャが候補側の拒否権を持つ。
///
/// `placement` は Stage 1 だけで出したドメインの一致（[`placement_domain_match_of`]）。
///
/// **単一化できないなら、そのままでは 1 つにまとめられない**ので候補として出さない。
/// 計画が「ここで初めて型シグネチャ単一化可能判定が入り、`EXTRACT-CANDIDATE` 側の
/// 精度が上がる」と言っているのがこの形（`docs/dryguard-plan.md`「Phase 2」）。
///
/// **Why not（`DO-NOT-EXTRACT` にする）**: そちらは「偶発的な重複」を指すラベル
/// (`classification::verdict`)。ドメインが同じなら偶発ではなく、
/// 型を一般化すればまとめられることもある。決めるのは人。
///
/// **Why not（単一化できることを候補側の決め手にする）**: `(Date) => string` のような
/// 汎用の型は別ドメインでもよく重なる。**重なることは必要条件であって、
/// 同じドメインである証拠ではない。**
fn shared_domain_verdict_of(type_signature_lean: Lean, placement: DomainMatch) -> Verdict {
    match type_signature_lean {
        Lean::TowardDoNotExtract => Verdict::Review,
        Lean::TowardExtract => Verdict::ExtractCandidate,
        Lean::Neither => stage1_shared_domain_verdict_of(placement),
    }
}

/// 型シグネチャが取れていないときのラベル。**Stage 1 だけで同じドメインと言えている
/// 場合に限って**候補に出す。
///
/// 呼び出し元の観測だけで `Undecidable` から `Same` へ上げたペアをここで候補にすると、
/// **単一化できるかを確かめないまま「共通化してよい」と言う**ことになる。
/// これは hover だけが落ちた環境で起きるので、**環境の差で判定が緩む側へ動く**
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
///
/// **Why not（`Separate` 側にも同じ条件を付ける）**: 型シグネチャは
/// 「1 つにまとめられるか」の必要条件で、**偶発的な重複と言うのに要る証拠ではない**
/// （計画の表でも呼び出し元の分布だけで非推奨側に傾く）。誤る向きも違い、
/// 偽の `EXTRACT-CANDIDATE` は間違った抽象化を生むが、偽の `DO-NOT-EXTRACT` は
/// 分離が続くだけ。
fn stage1_shared_domain_verdict_of(placement: DomainMatch) -> Verdict {
    match placement {
        DomainMatch::Same => Verdict::ExtractCandidate,
        DomainMatch::Separate | DomainMatch::Undecidable => Verdict::Review,
    }
}

/// 2 つのチャンクが同じドメインに属しているか。
///
/// `classification` の中だけの中間の値なので公開しない。外へ出すのは
/// ラベルと根拠で、その間の畳み方は判定の内側にある。
///
/// **ドメインそのものではなく、一致しているかどうか**を持つ
/// （`rules/naming.md`「名前と実体を一致させる」）。ドメインを表す値は
/// `semantics::caller_domain::Domain` にある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DomainMatch {
    /// 同じドメイン。
    Same,
    /// 別のドメイン。
    Separate,
    /// どちらとも言えない。
    Undecidable,
}

/// 置き場所の判定に、実際に誰が使っているかの観測を重ねて、ドメインが同じかを決める。
///
/// **観測で置き換えない。** ディレクトリと依存先は「どこに置かれているか」、
/// 呼び出し元は「誰が使っているか」で、片方がもう片方の言い換えではない
/// (`rules/naming.md`「`module distance` と `caller domain` を混ぜない」)。
///
/// `placement` は Stage 1 だけで出した判定（[`placement_domain_match_of`]）。
///
/// - 観測が取れなければ、置き場所の判定のまま（LSP が無い環境が通る道）
/// - 観測と置き場所が食い違えば `Undecidable`。**潰さずに人へ回す**
/// - 食い違わなければ観測の側に寄せる
fn domain_match_of(placement: DomainMatch, signals: &Signals) -> DomainMatch {
    match caller_domain_lean_of(signals.caller_domain_overlap()) {
        Lean::Neither => placement,
        Lean::TowardExtract => match placement {
            DomainMatch::Same | DomainMatch::Undecidable => DomainMatch::Same,
            DomainMatch::Separate => DomainMatch::Undecidable,
        },
        Lean::TowardDoNotExtract => match placement {
            DomainMatch::Separate | DomainMatch::Undecidable => DomainMatch::Separate,
            DomainMatch::Same => DomainMatch::Undecidable,
        },
    }
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
fn placement_domain_match_of(signals: &Signals) -> DomainMatch {
    match import_overlap_lean_of(signals.import_overlap()) {
        Lean::TowardExtract => DomainMatch::Same,
        Lean::Neither => DomainMatch::Undecidable,
        Lean::TowardDoNotExtract => match module_distance_lean_of(signals.module_distance()) {
            Lean::TowardDoNotExtract => DomainMatch::Separate,
            Lean::TowardExtract => DomainMatch::Undecidable,
            Lean::Neither => DomainMatch::Undecidable,
        },
    }
}

/// 構造類似度が閾値に届いているか。測れていなければ届いていない扱いにする。
///
/// `scan` が候補ペアを絞るのにも使う。**同じ条件を呼ぶ側に書き直させない**ため公開する。
/// 書き直すと、`classification` の決定木が「似ている」と見なす範囲と、候補として
/// 拾われる範囲が黙ってずれる（`rules/architecture.md`「判定は 1 箇所にだけ置く」）。
pub fn is_structurally_similar(signal: StructuralSimilarity, threshold: Threshold) -> bool {
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
        Reason::TypeSignatureMatch {
            signal: signals.type_signature_match(),
            lean: type_signature_lean_of(signals.type_signature_match()),
        },
        Reason::CallerDomainOverlap {
            signal: signals.caller_domain_overlap().clone(),
            lean: caller_domain_lean_of(signals.caller_domain_overlap()),
        },
    ]
}

/// 型シグネチャの単一化の可否が傾けた向き。尋ねていない / 測れなければ傾けない。
fn type_signature_lean_of(signal: TypeSignatureMatch) -> Lean {
    match signal {
        TypeSignatureMatch::Unifiable => Lean::TowardExtract,
        TypeSignatureMatch::NotUnifiable => Lean::TowardDoNotExtract,
        TypeSignatureMatch::Unavailable { .. }
        | TypeSignatureMatch::NoName
        | TypeSignatureMatch::NoTypeThere
        | TypeSignatureMatch::UnreadableHover
        | TypeSignatureMatch::UnreadableSignature
        | TypeSignatureMatch::HoverNotProvided
        | TypeSignatureMatch::TypeDefinitionNotProvided => Lean::Neither,
    }
}

/// 呼び出し元ドメインの重なりが傾けた向き。尋ねていない / 測れなければ傾けない。
fn caller_domain_lean_of(signal: &CallerDomainOverlap) -> Lean {
    let CallerDomainOverlap::Measured(measured) = signal else {
        return Lean::Neither;
    };

    if measured
        .overlap()
        .is_at_least(SHARED_CALLER_DOMAINS_THRESHOLD)
    {
        return Lean::TowardExtract;
    }
    Lean::TowardDoNotExtract
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

    use std::path::PathBuf;

    use crate::classification::reason::{Lean, Reason};
    use crate::classification::signal::{
        CallerDomainOverlap, ImportOverlap, MeasuredCallerDomains, SemanticsUnavailable, Signals,
        StructuralSimilarity, TypeSignatureMatch,
    };
    use crate::classification::verdict::Verdict;
    use crate::semantics::caller_domain::CallerDomains;
    use crate::similarity::Similarity;
    use crate::syntax::module_distance::ModuleDistance;
    use crate::threshold::Threshold;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    fn caller_domains(reference_paths: &[&str]) -> CallerDomains {
        let paths: Vec<PathBuf> = reference_paths.iter().map(PathBuf::from).collect();

        CallerDomains::from_reference_paths(&paths).expect("テストが渡す参照元は 1 件以上")
    }

    /// 呼び出し元が別のドメインに分かれている（重なり 0.00）。
    fn callers_in_separate_domains() -> CallerDomainOverlap {
        CallerDomainOverlap::Measured(MeasuredCallerDomains::new(
            caller_domains(&["src/billing/invoice.ts"]),
            caller_domains(&["src/inventory/stock.ts"]),
        ))
    }

    /// 呼び出し元が同じドメインから来ている（重なり 1.00）。
    fn callers_in_the_same_domain() -> CallerDomainOverlap {
        CallerDomainOverlap::Measured(MeasuredCallerDomains::new(
            caller_domains(&["src/report/monthly.ts"]),
            caller_domains(&["src/report/daily.ts"]),
        ))
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

        assert_eq!(classification.reasons().len(), 5);
    }

    /// 構造が似ていて依存先も共有している組（Stage 1 だけなら `EXTRACT-CANDIDATE`）。
    fn signals_of_a_shared_domain() -> Signals {
        Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(1.0)),
            separate_directories(),
        )
    }

    /// 構造が似ていて依存先が食い違い、ディレクトリも分かれている組
    /// （Stage 1 だけなら `DO-NOT-EXTRACT`）。
    fn signals_of_separate_domains() -> Signals {
        Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        )
    }

    #[test]
    fn test_classification_of_a_shared_domain_whose_type_signatures_do_not_unify_is_review() {
        // ドメインは一致している。**そのままでは 1 つにまとめられない**ので、
        // 候補として出さずに人へ回す
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::NotUnifiable,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_of_a_shared_domain_whose_type_signatures_unify_is_extract_candidate() {
        // 対照は上のテスト。型シグネチャ以外はすべて同じ
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
    }

    #[test]
    fn test_classification_with_an_unusable_lsp_keeps_the_verdict_the_stage1_signals_reach() {
        // サーバを使えないことを「単一化できない」と同じ扱いにすると、環境が悪いだけで
        // 候補が REVIEW に落ちる。上の 2 つと同じ入力で、Stage 2 だけを取れなくしてある
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
    }

    #[test]
    fn test_classification_of_callers_in_separate_domains_decides_an_undecidable_placement() {
        // import が無いので Stage 1 だけでは REVIEW。実際に誰が使っているかが
        // 取れて初めて別ドメインと言える
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::NoImports,
            separate_directories(),
        )
        .with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_separate_domains(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::DoNotExtract);
    }

    #[test]
    fn test_classification_of_callers_sharing_a_domain_against_a_separate_placement_is_review() {
        // 置き場所と依存先は別ドメインと言っているのに、使っているのは同じドメイン。
        // **観測で置き換えず、食い違いとして人へ回す**
        // (`rules/naming.md`「置き場所の代理指標を、使われ方の観測で置き換えない」)
        let signals = signals_of_separate_domains().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_the_same_domain(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_of_callers_in_separate_domains_against_a_shared_placement_is_review() {
        // 上と逆向きの食い違い。依存先は共有しているが、使っているドメインは分かれている
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_separate_domains(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    /// 依存先を測れず、呼び出し元だけが同じドメインを指す組。
    ///
    /// Stage 1 だけでは `REVIEW`（置き場所を決められない）。
    fn signals_of_a_caller_only_domain_match() -> Signals {
        Signals::new(
            StructuralSimilarity::Measured(measured(0.9)),
            ImportOverlap::NoImports,
            separate_directories(),
        )
    }

    #[test]
    fn test_classification_of_a_caller_only_domain_match_without_a_type_signature_is_review() {
        // 呼び出し元だけで候補へ上げると、**単一化できるかを確かめないまま
        // 「共通化してよい」と言う**ことになる。hover だけが落ちた環境で起きる
        let signals = signals_of_a_caller_only_domain_match().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
            callers_in_the_same_domain(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::Review);
    }

    #[test]
    fn test_classification_of_a_caller_only_domain_match_with_a_type_signature_is_extract_candidate()
     {
        // 対照は上のテスト。型シグネチャが取れているかどうかだけが違う
        let signals = signals_of_a_caller_only_domain_match()
            .with_semantics(TypeSignatureMatch::Unifiable, callers_in_the_same_domain());

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
    }

    #[test]
    fn test_classification_of_callers_in_separate_domains_does_not_need_a_type_signature() {
        // 候補側と違い、非推奨側には型シグネチャを求めない。型は「1 つにまとめられるか」の
        // 必要条件であって、偶発的な重複と言うのに要る証拠ではない
        let signals = signals_of_a_caller_only_domain_match().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::LspUnusable,
            },
            callers_in_separate_domains(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(classification.verdict(), Verdict::DoNotExtract);
    }

    #[test]
    fn test_classification_of_unifiable_type_signatures_leans_that_reason_toward_extract() {
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::TypeSignatureMatch {
                    signal: TypeSignatureMatch::Unifiable,
                    lean: Lean::TowardExtract,
                }
            ),
            "単一化できることが候補側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_type_signatures_that_do_not_unify_leans_that_reason_the_other_way() {
        let signals = signals_of_a_shared_domain().with_semantics(
            TypeSignatureMatch::NotUnifiable,
            CallerDomainOverlap::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::TypeSignatureMatch {
                    signal: TypeSignatureMatch::NotUnifiable,
                    lean: Lean::TowardDoNotExtract,
                }
            ),
            "単一化できないことが共通化しない側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_without_asking_the_lsp_leans_the_stage2_reasons_neither_way() {
        let signals = signals_of_a_shared_domain();

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::TypeSignatureMatch {
                    signal: TypeSignatureMatch::Unavailable {
                        reason: SemanticsUnavailable::NotAsked
                    },
                    lean: Lean::Neither,
                }
            ) && leans(
                &classification,
                &Reason::CallerDomainOverlap {
                    signal: CallerDomainOverlap::Unavailable {
                        reason: SemanticsUnavailable::NotAsked
                    },
                    lean: Lean::Neither,
                }
            ),
            "尋ねていないシグナルはどちらへも傾けない: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_callers_in_separate_domains_leans_that_reason_toward_do_not_extract()
    {
        let signals = signals_of_separate_domains().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_separate_domains(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::CallerDomainOverlap {
                    signal: callers_in_separate_domains(),
                    lean: Lean::TowardDoNotExtract,
                }
            ),
            "呼び出し元が分かれていることが共通化しない側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_callers_sharing_a_domain_leans_that_reason_toward_extract() {
        // 対照は上のテスト。呼び出し元のドメインだけが違う
        let signals = signals_of_separate_domains().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers_in_the_same_domain(),
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert!(
            leans(
                &classification,
                &Reason::CallerDomainOverlap {
                    signal: callers_in_the_same_domain(),
                    lean: Lean::TowardExtract,
                }
            ),
            "同じドメインから呼ばれていることが候補側へ傾けた根拠になる: {:?}",
            classification.reasons()
        );
    }

    #[test]
    fn test_classification_of_callers_overlapping_at_the_threshold_leans_toward_extract() {
        // 境界。合わせて 2 ドメインのうち 1 つが共通で、重なりは閾値ちょうどの 0.50
        let callers = CallerDomainOverlap::Measured(MeasuredCallerDomains::new(
            caller_domains(&["src/report/monthly.ts", "src/billing/invoice.ts"]),
            caller_domains(&["src/report/daily.ts"]),
        ));
        let signals = signals_of_separate_domains().with_semantics(
            TypeSignatureMatch::Unavailable {
                reason: SemanticsUnavailable::NotAsked,
            },
            callers,
        );

        let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        assert_eq!(
            classification.verdict(),
            Verdict::Review,
            "閾値ちょうどを共有している側に数えるので、置き場所の判定と食い違う: {:?}",
            classification.reasons()
        );
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
