//! `compare` が受け取った 2 箇所を、実際のファイルから判定まで通すところ。
//!
//! ステージをつないだ結果はここで見る（rules/testing.md「ステージをまたぐテストと
//! 単体のテストを分ける」）。切り出しそのものの振る舞いは `syntax::chunk`、
//! 決定木は `classification` のモジュール内テストにある。
//!
//! 入力は 2 種類ある。`tests/fixtures/` は**期待するラベルを先に決めて書いた**もので、
//! `tests/corpus/` は**業務アプリとして先に書いて判定を後から見た**もの
//! （`tests/corpus/README.md`）。前者は出力の形を、後者は**実データに当てたときの
//! 判定そのもの**を固定する。

use std::path::Path;

use dryguard::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
use dryguard::classification::verdict::Verdict;
use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::location::Location;
use dryguard::pipeline::{ChunkPairError, chunk_pair_of, signals_of};
use dryguard::report::text_of;
use dryguard::similarity::Similarity;

/// `tests/fixtures/` 配下の位置。
///
/// カレントディレクトリではなくマニフェストの位置から組み立てる
/// （テストの実行位置に依存させない）。
///
/// `expect` ではなく `panic!` で落とすのは、`clippy.toml` の `allow-expect-in-tests` が
/// `#[test]` の関数と `#[cfg(test)]` のモジュールしか見ないため。統合テストの
/// ヘルパー関数はそのどちらでもなく、`expect` は本番コードと同じく落とされる。
fn fixture(relative_path: &str, line: usize) -> Location {
    let text = format!(
        "{}/tests/fixtures/{relative_path}:{line}",
        env!("CARGO_MANIFEST_DIR")
    );

    let Ok(location) = text.parse() else {
        panic!("テストが組み立てる位置は解釈できる: {text}");
    };
    location
}

/// `tests/corpus/src/` 配下の位置。
///
/// 組み立て方は [`fixture`] と同じで、根だけが違う。
fn corpus(relative_path: &str, line: usize) -> Location {
    let text = format!(
        "{}/tests/corpus/src/{relative_path}:{line}",
        env!("CARGO_MANIFEST_DIR")
    );

    let Ok(location) = text.parse() else {
        panic!("テストが組み立てる位置は解釈できる: {text}");
    };
    location
}

/// 2 箇所を実ファイルから切り出して、シグナルを測るところまで。
fn signals(location_a: &Location, location_b: &Location) -> Signals {
    let Ok((chunk_a, chunk_b)) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    signals_of(&chunk_a, &chunk_b)
}

/// 測れた構造類似度。
fn structural_similarity(location_a: &Location, location_b: &Location) -> Similarity {
    let StructuralSimilarity::Measured(similarity) =
        signals(location_a, location_b).structural_similarity()
    else {
        panic!("テストが渡すチャンクにはトークンがある");
    };

    similarity
}

/// 依存先の重なり。測れなかったこと自体を見るテストがあるので、そのまま返す。
fn import_overlap(location_a: &Location, location_b: &Location) -> ImportOverlap {
    signals(location_a, location_b).import_overlap()
}

/// 2 箇所を実ファイルから切り出して、既定の閾値で判定するところまで。
fn verdict(location_a: &Location, location_b: &Location) -> Verdict {
    let signals = signals(location_a, location_b);

    classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD).verdict()
}

#[test]
fn test_compare_of_similar_functions_in_separate_domains_is_do_not_extract() {
    // Phase 0 の仮説そのもの。構造は同じ（1.00）だが、依存先が食い違っていて
    // ディレクトリも分かれている
    let verdict = verdict(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    assert_eq!(verdict, Verdict::DoNotExtract);
}

#[test]
fn test_compare_of_similar_functions_sharing_a_utility_is_extract_candidate() {
    // 同じ tests/fixtures/utils/pad に依存している。ディレクトリは分かれているが、
    // 依存先を共有しているので偶発的な重複ではない
    let verdict = verdict(
        &fixture("utils/formatDate.ts", 4),
        &fixture("report/dateHelper.ts", 4),
    );

    assert_eq!(verdict, Verdict::ExtractCandidate);
}

#[test]
fn test_compare_of_functions_with_different_shapes_is_review() {
    // 依存先もディレクトリも上の DO-NOT-EXTRACT の組と同じ条件で、構造だけが
    // 似ていない（0.23）。似ていないことが DO-NOT-EXTRACT に倒れないことを見る
    let verdict = verdict(
        &fixture("billing/discount.ts", 6),
        &fixture("report/summary.ts", 6),
    );

    assert_eq!(verdict, Verdict::Review);
}

#[test]
fn test_compare_of_two_functions_in_different_domains_reports_no_shared_dependency() {
    // 対照として、依存先が一致する組を下のテストに置いてある
    let overlap = import_overlap(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    assert_eq!(
        overlap,
        ImportOverlap::Measured(Similarity::new(0.0).expect("0.0 は範囲に含む")),
        "billing は ./invoice、inventory は ./stock にしか依存していない"
    );
}

#[test]
fn test_compare_of_two_functions_sharing_a_utility_reports_a_total_overlap() {
    // 綴りの違う相対指定（./pad と ../utils/pad）が同じファイルを指す組。
    // 指定子を文字列のまま比べると 0.0 になり、共有しているのに
    // 「依存先ドメインが不一致」と出る
    let overlap = import_overlap(
        &fixture("utils/formatDate.ts", 4),
        &fixture("report/dateHelper.ts", 4),
    );

    assert_eq!(
        overlap,
        ImportOverlap::Measured(Similarity::new(1.0).expect("1.0 は範囲に含む")),
        "どちらも tests/fixtures/utils/pad に依存している"
    );
}

#[test]
fn test_compare_of_two_locations_yields_a_chunk_for_each() {
    let (chunk_a, chunk_b) = chunk_pair_of(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(chunk_a.path().file_name(), Some("discount.ts".as_ref()));
    assert_eq!(chunk_b.path().file_name(), Some("reorder.ts".as_ref()));
}

#[test]
fn test_compare_of_two_locations_yields_the_function_that_encloses_each_line() {
    let (chunk_a, chunk_b) = chunk_pair_of(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(chunk_a.lines().to_string(), "5-8");
    assert!(
        chunk_a
            .source()
            .starts_with("export function applyDiscount("),
        "指定行を含む関数の先頭から始まる: {}",
        chunk_a.source()
    );
    assert_eq!(chunk_b.lines().to_string(), "5-8");
}

#[test]
fn test_compare_with_a_missing_file_reports_the_location_that_could_not_be_read() {
    let missing = fixture("billing/missing.ts", 6);

    let result = chunk_pair_of(&fixture("billing/discount.ts", 6), &missing);

    let Err(ChunkPairError::SourceUnreadable { location, .. }) = result else {
        panic!("読めないファイルは SourceUnreadable になる");
    };
    assert_eq!(
        location.path().file_name(),
        Some("missing.ts".as_ref()),
        "読めなかったほうの位置を持つ"
    );
}

#[test]
fn test_compare_with_a_line_outside_every_function_reports_the_location_that_failed() {
    // 10 行目は関数の外。同じファイルの 5-8 行目には関数があるので、
    // 「関数が 1 つも無いから失敗した」では通らない
    let outside = fixture("billing/discount.ts", 10);

    let result = chunk_pair_of(&outside, &fixture("inventory/reorder.ts", 6));

    let Err(ChunkPairError::ChunkingFailed { location, .. }) = result else {
        panic!("関数の外を指した位置は ChunkingFailed になる");
    };
    assert_eq!(location.path(), Path::new(outside.path()));
}

#[test]
fn test_compare_of_two_functions_that_differ_only_in_names_reports_a_high_similarity() {
    let similarity = structural_similarity(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    assert!(
        similarity.value() >= 0.9,
        "名前と定数だけが違う 2 つの関数は構造が同じ: {similarity}"
    );
}

#[test]
fn test_compare_of_similar_functions_in_separate_domains_reports_the_verdict_and_a_reason_that_leans_that_way()
 {
    // ステージをつなげて出力までいけることを見るテスト。個々のシグナルの傾きは
    // report のモジュール内テストで見る。ここでは実データから 3 種の情報
    // （ラベル・シグナル値・傾き）が揃って出るところまで
    let location_a = fixture("billing/discount.ts", 6);
    let location_b = fixture("inventory/reorder.ts", 6);
    let signals = signals(&location_a, &location_b);
    let classification = classification_of(&signals, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    let text = text_of(
        &location_a,
        &location_b,
        &classification,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
    );

    let first_line = text.lines().next().unwrap_or_default();
    assert!(
        first_line.starts_with("[DO-NOT-EXTRACT] "),
        "1 行目がラベルと 2 つの位置: {first_line}"
    );
    assert!(
        text.contains("依存先の重なり 0.00 → 共通化しない側"),
        "測った値と傾きの組が並ぶ: {text}"
    );
    assert!(
        text.contains("提案: 偶発的な重複の可能性が高い。共通化せず分離を維持する。"),
        "ラベルに対する提案が出る: {text}"
    );
}

#[test]
fn test_compare_of_two_filter_loops_over_unrelated_types_is_do_not_extract() {
    // Phase 0 の検証で出た真陽性そのもの（Issue #17）。`overdueInvoices` と
    // `itemsInWarehouse` は「配列を回して条件で絞り、別の配列へ push する」形が同じで、
    // 依存先は 1 つも重ならない。**このツールが検出したかったケース**なので、
    // 構造類似度の測り方や既定の閾値を触ったときに黙って落ちないよう固定する。
    let verdict = verdict(
        &corpus("billing/dunning.ts", 10),
        &corpus("inventory/warehouse.ts", 18),
    );

    assert_eq!(verdict, Verdict::DoNotExtract);
}

#[test]
fn test_compare_of_two_short_functions_without_a_shared_shape_is_review() {
    // 対照は上の真陽性。`allocate`（数値のクランプ）と `renderInvoiceEmail`
    // （文字列の組み立て）は、どちらも 3 行で `const` と `return` しか共有していない。
    // Phase 0 の集合 Jaccard はこれを 0.93 と測って DO-NOT-EXTRACT に倒していた
    // （Issue #17 が「弱点 1」と呼んだ偽陽性）。
    let verdict = verdict(
        &corpus("inventory/warehouse.ts", 13),
        &corpus("notification/email.ts", 6),
    );

    assert_eq!(verdict, Verdict::Review);
}

#[test]
fn test_compare_of_two_functions_with_different_shapes_reports_a_lower_similarity() {
    // 対照を 1 件置く。名前を潰した結果すべてが 0.9 を超えるなら、
    // このシグナルは閾値で分けられず何も言っていない
    let same_shape = structural_similarity(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );
    let different_shape = structural_similarity(
        &fixture("billing/discount.ts", 6),
        &fixture("report/summary.ts", 6),
    );

    assert!(
        different_shape.value() < same_shape.value(),
        "ループと配列を持つ関数のほうが構造が遠い: {different_shape} < {same_shape}"
    );
}
