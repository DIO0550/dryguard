//! `--fail-on` と終了コードの割り当てを、実際の CLI で見る。
//!
//! 終了コードは `main.rs` が決めるので、バイナリを起動して確かめる。
//! どのペアも Stage 1 のシグナルだけでラベルが決まるものを選んである
//! （`tests/compare.rs` / `tests/scan.rs` の同じペア）。実サーバの有無で判定は変わらず、
//! **変わるのは終了コードだけ**になる。
//!
//! 既定で走るテストは **`PATH` を空にして LSP サーバを見つけられなくする**。
//! `--fail-on` の下で検査を終えられなかったことが 2 になるのを、この経路で見る。
//! 判定で落ちた（1）・落ちなかった（0）は実サーバが要るので `#[ignore]` を付ける
//! （rules/testing.md「LSP を要するテストは、飛ばしたことが分かる形にする」）。

use std::path::Path;
use std::process::{Command, Output};

/// `--fail-on` に指定した判定が出たときの終了コード。
const FAILED_ON_VERDICT: i32 = 1;

/// 判定を出せなかったときの終了コード。
const NOT_CLASSIFIED: i32 = 2;

/// `--fail-on` の下で LSP に尋ね切れなかったときに stderr へ出る理由。
const UNCHECKED_REASON: &str = "--fail-on の検査を終えられませんでした";

fn fixture(relative_path: &str) -> String {
    format!(
        "{}/tests/fixtures/{relative_path}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dryguard"));
    command
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")));
    command
}

/// LSP サーバを見つけられない環境で走らせる。
fn run_without_an_lsp(args: &[&str]) -> Output {
    let Ok(output) = command(args).env("PATH", "").output() else {
        panic!("CLI を起動できる");
    };
    output
}

/// 実行環境の `PATH` のまま走らせる。CI は LSP サーバを `PATH` に入れている。
fn run_with_an_lsp(args: &[&str]) -> Output {
    let Ok(output) = command(args).output() else {
        panic!("CLI を起動できる");
    };
    output
}

/// 構造が同じで、依存先もディレクトリも分かれている組（DO-NOT-EXTRACT）。
fn do_not_extract_pair() -> [String; 2] {
    [
        fixture("billing/discount.ts:6"),
        fixture("inventory/reorder.ts:6"),
    ]
}

/// 構造が同じで、同じ道具立ての上に書かれた組（EXTRACT-CANDIDATE）。
fn extract_candidate_pair() -> [String; 2] {
    [
        fixture("utils/formatDate.ts:4"),
        fixture("report/dateHelper.ts:4"),
    ]
}

/// 構造が似ていない組（0.23）。候補ペアでないので LSP に尋ねない。
fn not_a_candidate_pair() -> [String; 2] {
    [
        fixture("billing/discount.ts:6"),
        fixture("report/summary.ts:6"),
    ]
}

#[test]
fn test_compare_with_fail_on_exits_with_two_when_the_lsp_cannot_be_asked() {
    // 判定が DO-NOT-EXTRACT でも 1 にしない。尋ね切れていない回に 1 を返すと、
    // CI は「非推奨ペアを直せばよい」と読み、検査が欠けたことを見落とす
    let [left, right] = do_not_extract_pair();
    let output = run_without_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
}

#[test]
fn test_compare_with_fail_on_exits_with_two_even_when_the_verdict_would_pass() {
    // 尋ね切れていれば DO-NOT-EXTRACT に変わったかもしれないので、0 を返さない
    let [left, right] = extract_candidate_pair();
    let output = run_without_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
}

#[test]
fn test_compare_with_fail_on_says_why_it_could_not_finish_on_stderr() {
    let [left, right] = do_not_extract_pair();
    let output = run_without_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains(UNCHECKED_REASON),
        "終了コード 2 の理由を stderr に出す: {stderr}"
    );
    assert!(
        stderr.contains("--fail-on に指定した判定が 1 件出ました"),
        "尋ね切れた分で見つかった件数も残す: {stderr}"
    );
}

#[test]
fn test_compare_with_fail_on_still_prints_the_verdict_when_the_lsp_cannot_be_asked() {
    // 失敗にしても判定の出力は捨てない。CI のログから、どのペアを見たのかを読めるように
    let [left, right] = do_not_extract_pair();
    let output = run_without_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.starts_with("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
fn test_compare_with_fail_on_succeeds_without_an_lsp_when_the_pair_is_not_a_candidate() {
    // 候補ペアでなければ LSP に尋ねないので、検査は欠けていない。
    // 対照は上のテストで、同じ環境・同じ指定で候補ペアは 2 になる
    let [left, right] = not_a_candidate_pair();
    let output = run_without_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.starts_with("[REVIEW] "), "{stdout}");
}

#[test]
fn test_compare_without_fail_on_succeeds_even_when_the_lsp_cannot_be_asked() {
    // `--fail-on` が無いときは Stage 1 だけの判定で 0 のまま
    let [left, right] = do_not_extract_pair();
    let output = run_without_an_lsp(&["compare", &left, &right]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains(UNCHECKED_REASON), "{stderr}");
}

#[test]
fn test_compare_that_cannot_classify_exits_with_two_even_with_fail_on() {
    // 判定で落ちた（1）と、判定を出せなかった（2）を CI が区別できるようにする
    let [left, right] = do_not_extract_pair();
    let output = run_without_an_lsp(&[
        "compare",
        &left,
        &right,
        "--lang",
        "rust",
        "--fail-on",
        "do-not-extract",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
}

#[test]
fn test_scan_with_fail_on_exits_with_two_when_the_lsp_cannot_be_asked() {
    let root = fixture("scan");
    let output = run_without_an_lsp(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
    assert!(stderr.contains(UNCHECKED_REASON), "{stderr}");
    assert!(stdout.contains("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
fn test_scan_that_cannot_start_exits_with_two() {
    let root = fixture("does-not-exist");
    let output = run_without_an_lsp(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_fail_on_exits_with_one_when_the_pair_is_do_not_extract() {
    let [left, right] = do_not_extract_pair();
    let output = run_with_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(FAILED_ON_VERDICT), "{stderr}");
    assert!(stdout.starts_with("[DO-NOT-EXTRACT] "), "{stdout}");
    assert!(
        stderr.contains("--fail-on に指定した判定が 1 件出ました"),
        "終了コード 1 の理由を stderr に出す: {stderr}"
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_fail_on_succeeds_when_the_pair_is_extract_candidate() {
    // 対照として、同じ指定で DO-NOT-EXTRACT の組が 1 になることを上のテストが見ている
    let [left, right] = extract_candidate_pair();
    let output = run_with_an_lsp(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.starts_with("[EXTRACT-CANDIDATE] "), "{stdout}");
    assert!(!stderr.contains(UNCHECKED_REASON), "{stderr}");
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_fail_on_exits_with_one_when_a_candidate_pair_is_do_not_extract() {
    let root = fixture("scan");
    let output = run_with_an_lsp(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(FAILED_ON_VERDICT), "{stdout}");
    assert!(stdout.contains("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
#[ignore = "typescript-language-server と rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_scan_with_fail_on_succeeds_when_no_candidate_pair_is_do_not_extract() {
    // 候補ペアは出るが、どれも REVIEW。「候補が 1 つも無い」では通らないよう、
    // 候補が出ていることも確かめる
    let root = fixture("language-scan");
    let output = run_with_an_lsp(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("[REVIEW] "), "{stdout}");
    assert!(!stdout.contains("[DO-NOT-EXTRACT] "), "{stdout}");
}
