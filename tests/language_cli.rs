//! 言語指定と auto が、実際の CLI でどのサーバを選ぶかを見る。

use std::path::Path;
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/language-scan/{name}",
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

#[test]
fn test_compare_auto_uses_rust_analyzer_for_rust_files() {
    let left = format!("{}:1", fixture("a.rs"));
    let right = format!("{}:1", fixture("b.rs"));
    let output = run(&["compare", &left, &right]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("rust-analyzer"),
        "Rust の候補は rust-analyzer に尋ねる: {stderr}"
    );
    assert!(!stderr.contains("typescript-language-server"));
}

#[test]
fn test_compare_explicit_language_rejects_a_mismatched_extension() {
    let left = format!("{}:1", fixture("a.rs"));
    let right = format!("{}:1", fixture("b.rs"));
    let output = run(&["compare", &left, &right, "--lang", "ts"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("--lang"), "不一致の直し先を示す: {stderr}");
    assert!(
        !stderr.contains("LSP"),
        "不一致ならサーバは起動しない: {stderr}"
    );
}

#[test]
fn test_compare_auto_reports_supported_extensions_when_language_is_unknown() {
    let left = format!("{}:1", fixture("unknown.md"));
    let right = format!("{}:1", fixture("b.rs"));
    let output = run(&["compare", &left, &right]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("unknown.md:1"),
        "対象の位置を示す: {stderr}"
    );
    assert!(stderr.contains(".ts") && stderr.contains(".rs"), "{stderr}");
}

#[test]
fn test_scan_auto_asks_both_servers_without_comparing_across_languages() {
    let root = fixture("");
    let output = run(&["scan", &root, "--lang", "auto", "--explain"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("typescript-language-server"), "{stderr}");
    assert!(stderr.contains("rust-analyzer"), "{stderr}");
    assert_eq!(stdout.matches(" <-> ").count(), 2, "{stdout}");
    assert!(
        stdout.contains("a.ts:1 <->") && stdout.contains("a.rs:1 <->"),
        "{stdout}"
    );
    assert!(stdout.contains("対象 4 ファイル"), "{stdout}");
}

#[test]
fn test_scan_rust_only_asks_rust_analyzer_and_excludes_typescript() {
    let root = fixture("");
    let output = run(&["scan", &root, "--lang", "rust"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("rust-analyzer"), "{stderr}");
    assert!(!stderr.contains("typescript-language-server"), "{stderr}");
    assert_eq!(stdout.matches(" <-> ").count(), 1, "{stdout}");
    assert!(stdout.contains("対象 2 ファイル"), "{stdout}");
}
