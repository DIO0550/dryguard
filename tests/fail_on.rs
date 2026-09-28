//! `--fail-on` と終了コードの割り当てを、実際の CLI で見る。
//!
//! 終了コードは `main.rs` が決めるので、バイナリを起動して確かめる。
//! **`PATH` を空にして LSP サーバを見つけられなくする。** サーバの有無で判定が変わると、
//! 同じテストが環境しだいで通ったり落ちたりする。どのペアも Stage 1 のシグナルだけで
//! ラベルが決まるものを選んである（`tests/compare.rs` / `tests/scan.rs` の同じペア）。

use std::path::Path;
use std::process::{Command, Output};

/// `--fail-on` に指定した判定が出たときの終了コード。
const FAILED_ON_VERDICT: i32 = 1;

/// 判定を出せなかったときの終了コード。
const NOT_CLASSIFIED: i32 = 2;

fn fixture(relative_path: &str) -> String {
    format!(
        "{}/tests/fixtures/{relative_path}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn run(args: &[&str]) -> Output {
    let Ok(output) = Command::new(env!("CARGO_BIN_EXE_dryguard"))
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .env("PATH", "")
        .output()
    else {
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

#[test]
fn test_compare_with_fail_on_exits_with_one_when_the_pair_is_do_not_extract() {
    let [left, right] = do_not_extract_pair();
    let output = run(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(FAILED_ON_VERDICT), "{stderr}");
}

#[test]
fn test_compare_with_fail_on_still_prints_the_verdict_to_stdout() {
    // 失敗にしても判定の出力は捨てない。CI のログから、どのペアで落ちたのかを読めるように
    let [left, right] = do_not_extract_pair();
    let output = run(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.starts_with("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
fn test_compare_with_fail_on_says_why_it_failed_on_stderr() {
    let [left, right] = do_not_extract_pair();
    let output = run(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("--fail-on に指定した判定が 1 件出ました"),
        "終了コード 1 の理由を stderr に出す: {stderr}"
    );
}

#[test]
fn test_compare_without_fail_on_succeeds_even_when_the_pair_is_do_not_extract() {
    let [left, right] = do_not_extract_pair();
    let output = run(&["compare", &left, &right]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "{stderr}");
}

#[test]
fn test_compare_with_fail_on_succeeds_when_the_pair_is_extract_candidate() {
    // 対照として、同じ指定で DO-NOT-EXTRACT の組が 1 になることを上のテストが見ている
    let left = fixture("utils/formatDate.ts:4");
    let right = fixture("report/dateHelper.ts:4");
    let output = run(&["compare", &left, &right, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.starts_with("[EXTRACT-CANDIDATE] "), "{stdout}");
}

#[test]
fn test_compare_that_cannot_classify_exits_with_two_even_with_fail_on() {
    // 判定で落ちた（1）と、判定を出せなかった（2）を CI が区別できるようにする
    let [left, right] = do_not_extract_pair();
    let output = run(&[
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
fn test_scan_with_fail_on_exits_with_one_when_a_candidate_pair_is_do_not_extract() {
    let root = fixture("scan");
    let output = run(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(FAILED_ON_VERDICT), "{stdout}");
    assert!(stdout.contains("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
fn test_scan_with_fail_on_succeeds_when_no_candidate_pair_is_do_not_extract() {
    // 候補ペアは出るが、どれも REVIEW。「候補が 1 つも無い」では通らないよう、
    // 候補が出ていることも確かめる
    let root = fixture("language-scan");
    let output = run(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("[REVIEW] "), "{stdout}");
    assert!(!stdout.contains("[DO-NOT-EXTRACT] "), "{stdout}");
}

#[test]
fn test_scan_that_cannot_start_exits_with_two() {
    let root = fixture("does-not-exist");
    let output = run(&["scan", &root, "--fail-on", "do-not-extract"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(NOT_CLASSIFIED), "{stderr}");
}
