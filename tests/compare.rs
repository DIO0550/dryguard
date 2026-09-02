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

use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use dryguard::classification::signal::{
    CallerDomainOverlap, ImportOverlap, SemanticsUnavailable, Signals, StructuralSimilarity,
    TypeSignatureMatch,
};
use dryguard::classification::verdict::Verdict;
use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::location::Location;
use dryguard::pipeline::{
    ChunkPairError, MeasuredPair, chunk_pair_of, measured_pair_of, signals_of,
};
use dryguard::report::text_of;
use dryguard::similarity::Similarity;

mod common;

use common::missing_server;

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
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    signals_of(pair.chunk_a(), pair.chunk_b())
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
    let pair = chunk_pair_of(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(
        pair.chunk_a().path().file_name(),
        Some("discount.ts".as_ref())
    );
    assert_eq!(
        pair.chunk_b().path().file_name(),
        Some("reorder.ts".as_ref())
    );
}

#[test]
fn test_compare_of_two_locations_yields_the_function_that_encloses_each_line() {
    let pair = chunk_pair_of(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(pair.chunk_a().lines().to_string(), "5-8");
    assert!(
        pair.chunk_a()
            .source()
            .starts_with("export function applyDiscount("),
        "指定行を含む関数の先頭から始まる: {}",
        pair.chunk_a().source()
    );
    assert_eq!(pair.chunk_b().lines().to_string(), "5-8");
}

#[test]
fn test_compare_of_two_locations_in_one_file_yields_each_function() {
    // 同じファイルの 2 箇所。**1 回しか読まない**ので、2 つのチャンクは必ず同じ版から出る
    // （2 回読むと、間で編集されたときに 1 つのファイルに 2 つの版ができ、
    // サーバが見る版と後のチャンクの名前の位置がずれる）
    let pair = chunk_pair_of(
        &corpus("inventory/warehouse.ts", 13),
        &corpus("inventory/warehouse.ts", 18),
    )
    .expect("どちらも関数の中を指している");

    assert_eq!(pair.chunk_a().lines().to_string(), "13-16");
    assert_eq!(
        pair.chunk_b().lines().to_string(),
        "18-27",
        "同じファイルでも、指した行ごとに別の関数が切り出される"
    );
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

/// 2 箇所を実ファイルから切り出して、起動できないサーバで Stage 2 を尋ねるところまで。
fn measured_without_an_lsp(location_a: &Location, location_b: &Location) -> MeasuredPair {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    measured_pair_of(
        &pair,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        &missing_server(),
    )
}

#[test]
fn test_compare_below_the_threshold_does_not_start_the_lsp_server() {
    // 似ていないペアの判定は Stage 2 で変わらないので、起こすだけ待たされる
    // （`docs/dryguard-plan.md`「候補ペアに対してだけ問い合わせる」）。
    // **起動できないサーバを渡しているので、起こしていれば理由が付く。**
    // 対照は下のテストで、同じサーバを閾値に届くペアへ渡すと理由が付く
    let measured = measured_without_an_lsp(
        &fixture("billing/discount.ts", 6),
        &fixture("report/summary.ts", 6),
    );

    assert!(
        measured.semantics_error().is_none(),
        "サーバを起こしていない: {:?}",
        measured.semantics_error().map(ToString::to_string)
    );
    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unavailable {
            reason: SemanticsUnavailable::NotACandidate
        }
    );
}

#[test]
fn test_compare_asks_with_the_source_stage1_read_even_if_the_file_disappears() {
    // 切り出した後にファイルが消えても、**Stage 1 が読んだ版で尋ねに行く**。
    // 読み直していると、ここで読み込みの失敗が理由に出る（構造のシグナルと
    // 意味のシグナルが別の版から出るのも同じ読み直しが原因）
    let directory = scratch_directory("vanishing-source");
    let vanishing = typescript_file(&directory, "vanishing.ts", SIMILAR_SOURCE);
    let staying = typescript_file(&directory, "staying.ts", SIMILAR_SOURCE);
    let Ok(pair) = chunk_pair_of(&location_at(&vanishing, 2), &location_at(&staying, 2)) else {
        panic!("テストが書いたソースはどちらも関数を持つ");
    };
    fs::remove_file(&vanishing).expect("テストが書いたファイルは消せる");

    let measured = measured_pair_of(
        &pair,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        &missing_server(),
    );

    fs::remove_dir_all(&directory).expect("テストが作ったディレクトリは消せる");
    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unavailable {
            reason: SemanticsUnavailable::LspUnusable
        },
        "消えたファイルではなくサーバが理由になる: {:?}",
        measured.semantics_error().map(ToString::to_string)
    );
}

/// 閾値に届くだけの構造を持つソース。2 つのファイルに同じものを書いて候補ペアにする。
const SIMILAR_SOURCE: &str = "export function totalOf(values: number[]): number {\n\
                              \x20 return values.reduce((sum, value) => sum + value, 0);\n\
                              }\n";

/// テストが書き込む作業ディレクトリ。作ってから返す。
///
/// 名前にプロセス ID を混ぜるのは、**同時に走る別の実行とぶつからない**ようにするため。
fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("dryguard-{name}-{}", process::id()));

    if fs::create_dir_all(&directory).is_err() {
        panic!(
            "テストが作る作業ディレクトリは書ける: {}",
            directory.display()
        );
    }
    directory
}

/// そのディレクトリに TypeScript のファイルを書いて、パスを返す。
fn typescript_file(directory: &Path, name: &str, source: &str) -> PathBuf {
    let path = directory.join(name);

    if fs::write(&path, source).is_err() {
        panic!("テストが作るファイルは書ける: {}", path.display());
    }
    path
}

/// 書いたファイルの指定行を指す位置。
fn location_at(path: &Path, line: usize) -> Location {
    let text = format!("{}:{line}", path.display());

    let Ok(location) = text.parse() else {
        panic!("テストが組み立てる位置は解釈できる: {text}");
    };
    location
}

#[test]
fn test_compare_without_an_lsp_server_still_reaches_the_stage1_verdict() {
    // Phase 0 の仮説と同じペア。**サーバを使えなくても判定まで届く**
    // （届かないと、LSP の入っていない環境でツールが使えなくなる）
    let measured = measured_without_an_lsp(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    let verdict =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD).verdict();
    assert_eq!(verdict, Verdict::DoNotExtract);
}

#[test]
fn test_compare_without_an_lsp_server_marks_the_stage2_signals_as_unmeasured() {
    // 対照は下のテスト（Stage 1 のシグナルは測れている）。取れなかったことを
    // 0.00 や「尋ねていない」で埋めると、環境が悪いのか材料が無いのかを読者が区別できない
    let measured = measured_without_an_lsp(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unavailable {
            reason: SemanticsUnavailable::LspUnusable
        }
    );
    assert_eq!(
        measured.signals().caller_domain_overlap(),
        &CallerDomainOverlap::Unavailable {
            reason: SemanticsUnavailable::LspUnusable
        }
    );
}

#[test]
fn test_compare_without_an_lsp_server_keeps_the_stage1_signals() {
    let measured = measured_without_an_lsp(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    assert_eq!(
        measured.signals().import_overlap(),
        ImportOverlap::Measured(Similarity::new(0.0).expect("0.0 は範囲に含む")),
        "LSP が要らないシグナルはそのまま測れている"
    );
}

#[test]
fn test_compare_without_an_lsp_server_reports_which_server_it_could_not_start() {
    // 「LSP サーバを使えない」だけでは、利用者は何を直せばよいか分からない
    let measured = measured_without_an_lsp(
        &fixture("billing/discount.ts", 6),
        &fixture("inventory/reorder.ts", 6),
    );

    let Some(error) = measured.semantics_error() else {
        panic!("起動できないサーバを渡したので理由が付く");
    };
    assert!(
        error
            .to_string()
            .contains("dryguard-no-such-language-server"),
        "どの実行ファイルを起こせなかったかが読める: {error}"
    );
}

#[test]
fn test_compare_without_an_lsp_server_reports_the_stage2_signals_as_unmeasured_in_the_text() {
    let location_a = fixture("billing/discount.ts", 6);
    let location_b = fixture("inventory/reorder.ts", 6);
    let measured = measured_without_an_lsp(&location_a, &location_b);
    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    let text = text_of(
        &location_a,
        &location_b,
        &classification,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
    );

    assert!(
        text.contains("型シグネチャ: 測れない (LSP サーバを使えない)")
            && text.contains("呼び出し元ドメインの重なりを測れない (LSP サーバを使えない)"),
        "どのシグナルが取れなかったかが出力から読める: {text}"
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
