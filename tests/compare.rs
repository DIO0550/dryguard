//! `compare` が受け取った 2 箇所を、実際のファイルからチャンクにするところまで。
//!
//! ステージをつないだ結果はここで見る（rules/testing.md「ステージをまたぐテストと
//! 単体のテストを分ける」）。切り出しそのものの振る舞いは `syntax::chunk` の
//! モジュール内テストにある。

use std::path::Path;

use dryguard::location::Location;
use dryguard::pipeline::{ChunkCollectionError, collect_chunks};
use dryguard::similarity::Similarity;
use dryguard::syntax::token::TokenSet;

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

/// 2 箇所を実ファイルから切り出して、構造類似度を出すところまで。
fn structural_similarity(location_a: &Location, location_b: &Location) -> Similarity {
    let Ok((chunk_a, chunk_b)) = collect_chunks(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let (Some(tokens_a), Some(tokens_b)) = (
        TokenSet::from_source(chunk_a.source()),
        TokenSet::from_source(chunk_b.source()),
    ) else {
        panic!("テストが渡すチャンクにはトークンがある");
    };

    tokens_a.jaccard(&tokens_b)
}

#[test]
fn test_compare_of_two_locations_collects_a_chunk_for_each() {
    let (chunk_a, chunk_b) = collect_chunks(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(chunk_a.path().file_name(), Some("discount.ts".as_ref()));
    assert_eq!(chunk_b.path().file_name(), Some("reorder.ts".as_ref()));
}

#[test]
fn test_compare_of_two_locations_collects_the_function_that_encloses_each_line() {
    let (chunk_a, chunk_b) = collect_chunks(
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

    let result = collect_chunks(&fixture("billing/discount.ts", 6), &missing);

    let Err(ChunkCollectionError::SourceUnreadable { location, .. }) = result else {
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

    let result = collect_chunks(&outside, &fixture("inventory/reorder.ts", 6));

    let Err(ChunkCollectionError::ChunkingFailed { location, .. }) = result else {
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
        "名前と定数だけが違う 2 つの関数は構造がほぼ同じ: {similarity}"
    );
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
