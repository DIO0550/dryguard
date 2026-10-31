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
fn test_scan_auto_reports_typescript_server_failure_before_rust_analyzer() {
    let root = fixture("");
    let output = run(&["scan", &root, "--lang", "auto"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let typescript = stderr.find("typescript-language-server");
    let rust = stderr.find("rust-analyzer");

    assert!(output.status.success(), "{stderr}");
    assert!(typescript.is_some() && rust.is_some(), "{stderr}");
    assert!(typescript < rust, "サーバの並びの順に出す: {stderr}");
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

fn swift_fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/swift-language/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn test_compare_typescript_language_rejects_swift_files() {
    let left = format!("{}:1", swift_fixture("a.swift"));
    let right = format!("{}:1", swift_fixture("b.swift"));
    let output = run(&["compare", &left, &right, "--lang", "ts"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("一致しません"), "{stderr}");
    assert!(
        !stdout.contains(" <-> "),
        "不一致なら判定を出さない: {stdout}"
    );
}

#[test]
fn test_compare_swift_language_rejects_typescript_files() {
    let left = format!("{}:1", fixture("a.ts"));
    let right = format!("{}:1", fixture("b.ts"));
    let output = run(&["compare", &left, &right, "--lang", "swift"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("一致しません"), "{stderr}");
    assert!(
        !stdout.contains(" <-> "),
        "不一致なら判定を出さない: {stdout}"
    );
}

#[test]
fn test_compare_auto_accepts_swift_files() {
    // `auto` の走査は Swift を拾わないが、`compare` は名指しされれば比べる
    let left = format!("{}:1", swift_fixture("a.swift"));
    let right = format!("{}:1", swift_fixture("b.swift"));
    let output = run(&["compare", &left, &right]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("a.swift:1 <->"), "{stdout}");
}

#[test]
fn test_compare_swift_language_accepts_swift_files() {
    // 上の拒否と対にする。Swift を受け付ける腕が無ければ、こちらも不一致で落ちる
    let left = format!("{}:1", swift_fixture("a.swift"));
    let right = format!("{}:1", swift_fixture("b.swift"));
    let output = run(&["compare", &left, &right, "--lang", "swift"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("a.swift:1 <->"), "{stdout}");
}

#[test]
fn test_compare_typescript_language_rejects_java_files() {
    let location = format!("{}:4", fixture("Discount.java"));
    let output = run(&["compare", &location, &location, "--lang", "ts"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("一致しません"), "{stderr}");
}

#[test]
fn test_compare_auto_uses_sourcekit_lsp_for_swift_files() {
    let left = format!("{}:1", swift_fixture("a.swift"));
    let right = format!("{}:1", swift_fixture("b.swift"));
    let output = run(&["compare", &left, &right]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("sourcekit-lsp"),
        "Swift の候補は sourcekit-lsp に尋ねる: {stderr}"
    );
    assert!(
        !stderr.contains("typescript-language-server") && !stderr.contains("rust-analyzer"),
        "{stderr}"
    );
}

#[test]
fn test_scan_swift_only_asks_sourcekit_lsp_and_excludes_typescript() {
    // 同じディレクトリに `.ts` を 1 つ置く。TypeScript を走査すると対象が 1 ファイルになる
    let root = swift_fixture("");
    let output = run(&["scan", &root, "--lang", "swift"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert_eq!(stdout.matches(" <-> ").count(), 1, "{stdout}");
    assert!(stdout.contains("a.swift:1 <->"), "{stdout}");
    assert!(stdout.contains("対象 2 ファイル"), "{stdout}");
    assert!(stderr.contains("sourcekit-lsp"), "{stderr}");
    assert!(
        !stderr.contains("typescript-language-server") && !stderr.contains("rust-analyzer"),
        "{stderr}"
    );
}

fn java_fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/java-language/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn test_compare_java_language_rejects_typescript_files() {
    let location = format!("{}:1", java_fixture("a.ts"));
    let output = run(&["compare", &location, &location, "--lang", "java"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("一致しません"), "{stderr}");
    assert!(stdout.is_empty(), "判定を出さない: {stdout}");
}

#[test]
fn test_compare_java_language_accepts_java_files() {
    // 対照は上の `.ts`。Java の腕を拒否へ写し違えると、こちらが落ちる
    let left = format!("{}:2", java_fixture("A.java"));
    let right = format!("{}:2", java_fixture("B.java"));
    let output = run(&["compare", &left, &right, "--lang", "java"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("A.java:2 <->"), "{stdout}");
}

#[test]
fn test_scan_java_language_walks_only_java_files() {
    // 同じディレクトリに `.ts` を 1 つ置く。TypeScript も走査すると対象が 3 ファイルになり、
    // Java の腕を TypeScript へ写し違えると対象が 1 ファイルになる
    let root = java_fixture("");
    let output = run(&["scan", &root, "--lang", "java"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert_eq!(stdout.matches(" <-> ").count(), 1, "{stdout}");
    assert!(stdout.contains("A.java:2 <->"), "{stdout}");
    assert!(stdout.contains("対象 2 ファイル"), "{stdout}");
}
