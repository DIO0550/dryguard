//! `scan` が受け取ったディレクトリを、ファイルの収集から判定まで通すところ。
//!
//! ステージをつないだ結果はここで見る（rules/testing.md「ステージをまたぐテストと
//! 単体のテストを分ける」）。ファイルの集め方は `codebase`、チャンクの列挙は
//! `syntax::chunk`、出力の形は `report` のモジュール内テストにある。
//!
//! 入力は `tests/corpus/`。**業務アプリとして先に書いて判定を後から見た**もので
//! （`tests/corpus/README.md`）、`compare` が 2 箇所を名指しで見ているのと同じペアを
//! **総当たりの中から拾えるか**をここで見る。
//!
//! 実サーバを要するテストには `#[ignore]` を付ける。サーバの入っていない開発機で
//! 黙って通さないため（rules/testing.md「LSP を要するテストは、飛ばしたことが
//! 分かる形にする」）。それ以外は**起動できないサーバ**を渡して、LSP を使えない環境の
//! 経路を通す。

use std::path::{Path, PathBuf};

use dryguard::classification::DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
use dryguard::classification::verdict::Verdict;
use dryguard::lsp::ServerCommand;
use dryguard::pipeline::{CandidatePair, Scan, scan_of};
use dryguard::report::scan_text_of;

mod common;

use common::missing_server;

/// `tests/corpus/src/` を既定の閾値で走査した結果。LSP は使えない。
///
/// カレントディレクトリではなくマニフェストの位置から組み立てる
/// （テストの実行位置に依存させない）。
///
/// `expect` ではなく `panic!` で落とすのは、`clippy.toml` の `allow-expect-in-tests` が
/// `#[test]` の関数と `#[cfg(test)]` のモジュールしか見ないため。統合テストの
/// ヘルパー関数はそのどちらでもなく、`expect` は本番コードと同じく落とされる。
fn scan_of_corpus() -> Scan {
    scan_of_root(&corpus_root(), &missing_server())
}

/// `tests/corpus/src/` のパス。
fn corpus_root() -> PathBuf {
    PathBuf::from(format!("{}/tests/corpus/src", env!("CARGO_MANIFEST_DIR")))
}

/// そのディレクトリを既定の閾値で走査した結果。
fn scan_of_root(root: &Path, server: &ServerCommand) -> Scan {
    let Ok(scan) = scan_of(root, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, server) else {
        panic!("テストが渡す根は走査できる: {}", root.display());
    };
    scan
}

/// そのペアが、コーパスの中の 2 箇所（`<相対パス>:<行>`）を指しているか。
fn is_pair_of(pair: &CandidatePair, one: &str, other: &str) -> bool {
    let ends_with_both = |left: &str, right: &str| {
        pair.location_a().to_string().ends_with(left)
            && pair.location_b().to_string().ends_with(right)
    };

    ends_with_both(one, other) || ends_with_both(other, one)
}

/// そのペアが列挙された位置。`(先に列挙したチャンク, 後に列挙したチャンク)` の順で比べられる形。
fn enumeration_key_of(pair: &CandidatePair) -> (PathBuf, usize, PathBuf, usize) {
    (
        pair.location_a().path().to_path_buf(),
        pair.location_a().line().get(),
        pair.location_b().path().to_path_buf(),
        pair.location_b().line().get(),
    )
}

/// そのペアの判定。候補に出ていなければ `None`。
fn verdict_of(scan: &Scan, one: &str, other: &str) -> Option<Verdict> {
    scan.candidate_pairs()
        .iter()
        .find(|pair| is_pair_of(pair, one, other))
        .map(|pair| pair.classification().verdict())
}

#[test]
fn test_scan_of_the_corpus_finds_the_accidental_duplication_compare_reports() {
    // `compare` が名指しで DO-NOT-EXTRACT と判定するペア（tests/compare.rs の
    // 「2 つのフィルタループ」）。総当たりの中からこれを拾えないなら、
    // scan は compare で見つかるものを見落としている
    let scan = scan_of_corpus();

    assert_eq!(
        verdict_of(&scan, "billing/dunning.ts:10", "inventory/warehouse.ts:18"),
        Some(Verdict::DoNotExtract)
    );
}

#[test]
fn test_scan_of_the_corpus_leaves_out_a_pair_that_is_not_structurally_similar() {
    // 対照は上のテストのペア。同じ走査の中に候補として出る組があるので、
    // 「そもそも候補が 1 つも出ていない」では通らない
    let scan = scan_of_corpus();

    assert_eq!(
        verdict_of(
            &scan,
            "inventory/warehouse.ts:13",
            "notification/email.ts:6"
        ),
        None,
        "構造が似ていないペアは候補にしない"
    );
    assert!(
        verdict_of(&scan, "billing/dunning.ts:10", "inventory/warehouse.ts:18").is_some(),
        "同じ走査で候補に出る組はある"
    );
}

#[test]
fn test_scan_of_the_corpus_compares_every_pair_of_chunks_it_found() {
    // 47 関数 / 1081 ペアは `docs/dryguard-plan.md`「Stage 1」と
    // `classification` の既定閾値が拠っているコーパスの大きさ。ここがずれたら、
    // 閾値を決めたときの母数がもう成り立っていない
    let scan = scan_of_corpus();

    assert_eq!(scan.chunk_count(), 47, "コーパスの関数の数");
    assert_eq!(scan.compared_pair_count(), 1081, "総当たりのペアの数");
}

#[test]
fn test_scan_of_the_corpus_keeps_every_candidate_pair_it_can_reach() {
    // 突き合わせを省く仕組み（長さの上限による枝刈り）は、候補ペアを 1 組も
    // 落としてはならない。落ちればこの数が減る
    let scan = scan_of_corpus();

    assert_eq!(scan.candidate_pairs().len(), 65, "閾値に届いたペアの数");
}

#[test]
fn test_scan_of_the_corpus_lists_its_candidate_pairs_in_enumeration_order() {
    // ファイルはパス順、ファイルの中のチャンクはソース順に列挙されるので、
    // 総当たりで出た候補ペアは (先のチャンク, 後のチャンク) の昇順に並ぶ。
    // `Scan::candidate_pairs` が doc で約束している「列挙した順」がこれ
    let scan = scan_of_corpus();

    let keys: Vec<(PathBuf, usize, PathBuf, usize)> = scan
        .candidate_pairs()
        .iter()
        .map(enumeration_key_of)
        .collect();
    assert!(
        keys.len() > 1,
        "並びを見るには 2 件以上の候補が要る: {keys:?}"
    );
    assert!(keys.is_sorted(), "候補ペアは列挙した順に並ぶ: {keys:?}");
}

#[test]
fn test_scan_of_the_corpus_reads_every_typescript_file_under_the_root() {
    let scan = scan_of_corpus();

    assert_eq!(scan.file_count(), 19);
    assert!(
        scan.skipped_files().is_empty(),
        "コーパスに読めないファイルは無い: {:?}",
        scan.skipped_files()
    );
}

#[test]
fn test_scan_text_of_the_corpus_lists_the_pairs_and_ends_with_what_it_walked() {
    let scan = scan_of_corpus();

    let text = scan_text_of(&scan, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    let first_line = text.lines().next().unwrap_or_default();
    assert!(
        first_line.starts_with('['),
        "候補ペアの 1 行目から始まる: {first_line}"
    );
    assert!(
        text.lines().last().unwrap_or_default().starts_with("対象 "),
        "走査した量で終わる: {text}"
    );
}

#[test]
fn test_scan_of_a_directory_without_typescript_finds_nothing_to_compare() {
    // 対照はコーパスの走査。docs/ には TypeScript が無いので、候補も比較も出ない
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");

    let scan = scan_of_root(&root, &missing_server());

    assert_eq!(scan.file_count(), 0);
    assert_eq!(scan.compared_pair_count(), 0);
    assert!(scan.candidate_pairs().is_empty());
}

#[test]
fn test_scan_without_an_lsp_server_reports_why_the_stage2_signals_are_missing() {
    // 対照は Stage 1 の根拠（依存先の重なり）。**測れた値と測れなかったことが
    // 同じ出方をすると、読者が両者を区別できない**
    let scan = scan_of_corpus();

    let text = scan_text_of(&scan, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    assert!(
        scan.semantics_error().is_some(),
        "尋ねられなかったことが理由として残る"
    );
    assert!(
        text.contains("型シグネチャ: 測れない (LSP サーバを使えない)")
            && text.contains("呼び出し元ドメインの重なりを測れない (LSP サーバを使えない)"),
        "候補ペアごとに、Stage 2 が効かなかった理由が出る: {text}"
    );
    assert!(
        text.contains("依存先の重なり"),
        "Stage 1 の根拠はそのまま出る: {text}"
    );
}

#[test]
fn test_scan_without_an_lsp_server_keeps_the_verdict_the_stage1_signals_reach() {
    // サーバを使えないことで判定が動くと、**環境の差で結果が変わる**。
    // 対照は上のテストで、同じ走査に「測れない」の行は出ている
    let scan = scan_of_corpus();

    assert_eq!(
        verdict_of(&scan, "billing/dunning.ts:10", "inventory/warehouse.ts:18"),
        Some(Verdict::DoNotExtract)
    );
}

#[test]
fn test_scan_without_candidate_pairs_does_not_start_the_lsp_server() {
    // 候補ペアでないペアの判定は Stage 2 で変わらないので、起こすだけ待たされる
    // （`docs/dryguard-plan.md`「候補ペアに対してだけ問い合わせる」）。
    // **起動できないサーバを渡しているので、起こしていれば理由が付く。**
    // 対照は上の 2 つで、同じコーパス全体を走査すると理由が付く
    let scan = scan_of_root(&corpus_root().join("shared"), &missing_server());

    assert!(
        scan.candidate_pairs().is_empty(),
        "閾値に届くペアが無いディレクトリを選んでいる: {}",
        scan.candidate_pairs().len()
    );
    assert!(
        scan.compared_pair_count() > 0,
        "比べてはいる（そもそもペアが無いのではない）"
    );
    assert!(
        scan.semantics_error().is_none(),
        "サーバを起こしていない: {:?}",
        scan.semantics_error().map(ToString::to_string)
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_an_lsp_finds_a_candidate_pair_sharing_a_utility_unifiable() {
    // 対照は下のテストで、**同じ走査の中の別の 1 組**が単一化不能と出る
    // （片方だけでは、実装が全部同じ答えを返しても通る）
    let scan = scan_of_root(&references_root(), &ServerCommand::typescript());

    assert!(
        scan.semantics_error().is_none(),
        "実サーバには尋ねられる: {:?}",
        scan.semantics_error().map(ToString::to_string)
    );
    let text = pair_text_of(&scan, "report/dateHelper.ts:3", "utils/formatDate.ts:3");
    assert!(
        text.as_deref()
            .is_some_and(|text| text.contains("型シグネチャ: 単一化可能 → 共通化する側")),
        "同じ道具立ての上に書かれた 2 つは重なる: {text:?}"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_an_lsp_finds_the_accidental_duplication_not_unifiable() {
    // 対照は上のテスト。構造は同じ（1.00）だが、受け取る型が別ドメインのもの
    let scan = scan_of_root(&references_root(), &ServerCommand::typescript());

    let text = pair_text_of(&scan, "billing/discount.ts:5", "inventory/reorder.ts:5");
    assert!(
        text.as_deref()
            .is_some_and(|text| text.contains("型シグネチャ: 単一化不能 → 共通化しない側")),
        "別ドメインの型を受け取る 2 つは重ならない: {text:?}"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_an_lsp_counts_the_caller_domains_of_the_accidental_duplication() {
    // 対照は下のテスト。**同じ走査の中の別の 1 組**は呼び出し元を共有している
    let scan = scan_of_root(&references_root(), &ServerCommand::typescript());

    let text = pair_text_of(&scan, "billing/discount.ts:5", "inventory/reorder.ts:5");
    assert!(
        text.as_deref().is_some_and(|text| {
            text.contains("呼び出し元ドメインの重なり 0.00 (")
                && text.contains("/billing ")
                && text.contains("/inventory ")
        }),
        "重なりの値と、両側の分布が出る: {text:?}"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_an_lsp_finds_the_shared_utility_called_from_one_domain() {
    // 対照は上のテスト。呼び出し元のドメインだけが違う
    let scan = scan_of_root(&references_root(), &ServerCommand::typescript());

    let text = pair_text_of(&scan, "report/dateHelper.ts:3", "utils/formatDate.ts:3");
    assert!(
        text.as_deref()
            .is_some_and(|text| text.contains("呼び出し元ドメインの重なり 1.00 (")),
        "同じドメインから呼ばれていることが出る: {text:?}"
    );
}

/// `tests/fixtures/references/src/` のパス。
///
/// LSP 向けのフィクスチャ（候補ペアの共通の祖先に tsconfig.json がある）を走査の根にする。
fn references_root() -> PathBuf {
    PathBuf::from(format!(
        "{}/tests/fixtures/references/src",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// そのペアについて出力に出た区切り。候補に出ていなければ `None`。
///
/// **シグナルを直接覗かず、利用者が見るのと同じものを見る**
/// （rules/testing.md「振る舞いをテストする」）。
fn pair_text_of(scan: &Scan, one: &str, other: &str) -> Option<String> {
    let pair = scan
        .candidate_pairs()
        .iter()
        .find(|pair| is_pair_of(pair, one, other))?;
    let heading = format!(
        "[{}] {} <-> {}",
        pair.classification().verdict(),
        pair.location_a(),
        pair.location_b()
    );

    scan_text_of(scan, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD)
        .split("\n\n")
        .find(|block| block.starts_with(&heading))
        .map(ToOwned::to_owned)
}

#[test]
fn test_scan_of_the_corpus_rules_out_pairs_whose_lengths_are_too_far_apart() {
    // 上限だけで確定できるペアが 1 組も無いなら、枝刈りは何も飛ばしていない。
    // 対照は上のテスト（候補 65 組）。飛ばしすぎればあちらが落ちる
    let scan = scan_of_corpus();

    assert!(
        scan.pruned_pair_count() > 0,
        "長さが 2 倍以上離れたペアは突き合わせずに確定する"
    );
    assert!(
        scan.pruned_pair_count() < scan.compared_pair_count(),
        "突き合わせたペアも残る: {} / {}",
        scan.pruned_pair_count(),
        scan.compared_pair_count()
    );
}
