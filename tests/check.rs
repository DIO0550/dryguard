//! `check --diff` が、git の差分から変わったチャンクを取り、コードベース全体と比べるところ。
//!
//! 差分の書式の読み方は `codebase::diff`、候補ペアの判定は `scan` と同じ経路なので、
//! ここでは**ステージをつないだ結果**（どのペアが起点に含まれるか）だけを見る
//! （rules/testing.md「ステージをまたぐテストと単体のテストを分ける」）。
//!
//! 入力は一時ディレクトリに作る git の作業ツリー。git は実物を起動する
//! （`rules/testing.md`「モックは使わない」）。LSP は起動できないサーバを渡して、
//! 使えない環境の経路を通す。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use dryguard::classification::ConfiguredThresholds;
use dryguard::codebase::SourceLanguage;
use dryguard::codebase::diff::{DiffBase, DiffError};
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::pipeline::{
    CandidatePair, CheckError, LanguageServers, Scan, check_with_language_servers, scan_of,
};
use dryguard::report::{Explanation, scan_json_of, scan_text_of};
use dryguard::syntax::chunk::TestFunctions;

mod common;

use common::missing_server;

/// 構造の同じ関数を、別々のファイルに 4 つ置く。どの 2 つも候補ペアになる。
const DISCOUNT: &str = "export function applyDiscount(price: number, rate: number): number {\n\
                        \x20 const discounted = price * (1 - rate);\n\
                        \x20 return Math.max(discounted, 0);\n\
                        }\n";
const REORDER: &str = "export function reorderPoint(level: number, buffer: number): number {\n\
                       \x20 const point = level * (1 + buffer);\n\
                       \x20 return Math.max(point, 0);\n\
                       }\n";
const FEE: &str = "export function shippingFee(weight: number, rate: number): number {\n\
                   \x20 const fee = weight * (1 + rate);\n\
                   \x20 return Math.max(fee, 0);\n\
                   }\n";
const BONUS: &str = "export function bonusOf(salary: number, rate: number): number {\n\
                     \x20 const bonus = salary * (1 - rate);\n\
                     \x20 return Math.max(bonus, 0);\n\
                     }\n";

/// 構造を変えずに、`DISCOUNT` の本体の 1 行だけを書き換えたもの（リテラルの値だけが違う）。
const DISCOUNT_EDITED: &str = "export function applyDiscount(price: number, rate: number): number {\n\
                               \x20 const discounted = price * (1 - rate);\n\
                               \x20 return Math.max(discounted, 1);\n\
                               }\n";

const FILES: [(&str, &str); 4] = [
    ("src/billing/discount.ts", DISCOUNT),
    ("src/inventory/reorder.ts", REORDER),
    ("src/shipping/fee.ts", FEE),
    ("src/payroll/bonus.ts", BONUS),
];

/// テスト用の git 作業ツリー。`files` を書いて 1 度コミットした状態で返す。
///
/// 名前にプロセス ID を混ぜるのは、同時に走る別の実行とぶつからないようにするため。
/// 前の実行の残りは消してから作る（残ったコミットや変更で結果が変わらないように）。
fn committed_work_tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dryguard-check-{name}-{}", process::id()));
    if root.exists() && fs::remove_dir_all(&root).is_err() {
        panic!("前の実行の作業ツリーは消せる: {}", root.display());
    }
    if fs::create_dir_all(&root).is_err() {
        panic!("テストが作る作業ディレクトリは書ける: {}", root.display());
    }

    git(&root, &["init", "--quiet"]);
    for (path, source) in files {
        write_file(&root, path, source);
    }
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "--message", "initial"]);
    root
}

/// `root` の下に書く。途中のディレクトリも作る。
fn write_file(root: &Path, path: &str, source: &str) {
    let path = root.join(path);
    let written = path
        .parent()
        .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        && fs::write(&path, source).is_ok();
    if !written {
        panic!("テストが作るファイルは書ける: {}", path.display());
    }
}

/// テストの作業ツリーで git を走らせる。
///
/// 署名と作者を引数で固定する。**実行する人のグローバル設定**（コミットの署名など）で
/// コミットが落ちると、テストの結果が環境で変わる。
fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=dryguard-test",
            "-c",
            "user.email=dryguard-test@example.com",
        ])
        .args(arguments)
        .status();
    let Ok(status) = status else {
        panic!("テストは git を起動できる環境で走らせる");
    };
    assert!(status.success(), "git {arguments:?} が失敗した");
}

fn check_of(root: &Path, base: &str) -> Result<Scan, CheckError> {
    let Ok(base) = base.parse::<DiffBase>() else {
        panic!("テストが渡すリビジョンは受け付ける綴り: {base}");
    };
    let server = missing_server();

    check_with_language_servers(
        root,
        &base,
        SourceLanguage::TypeScript,
        TestFunctions::Excluded,
        ConfiguredThresholds::default(),
        &DomainDeclarations::default(),
        LanguageServers {
            typescript: &server,
            rust: &server,
        },
    )
}

fn includes_file(pair: &CandidatePair, file: &str) -> bool {
    pair.location_a().path().ends_with(file) || pair.location_b().path().ends_with(file)
}

#[test]
fn test_check_lists_only_the_pairs_that_include_a_changed_chunk() {
    let root = committed_work_tree("changed", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);

    let check = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(
        check.candidate_pairs().len(),
        3,
        "変えた関数と、残りの 3 つとの組だけが出る: {:?}",
        check.candidate_pairs()
    );
    assert!(
        check
            .candidate_pairs()
            .iter()
            .all(|pair| includes_file(pair, "src/billing/discount.ts")),
        "変わっていない関数どうしの組は出ない: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_scan_of_the_same_tree_lists_the_pairs_check_leaves_out() {
    // 上のテストの対照。同じ作業ツリーを全体で走査すると、変わっていない関数どうしの組も
    // 候補になる。ここが 3 組なら、上のテストは何も絞っていない
    let root = committed_work_tree("changed-scan", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);

    let scan = scan_of(
        &root,
        TestFunctions::Excluded,
        ConfiguredThresholds::default(),
        &DomainDeclarations::default(),
        &missing_server(),
    )
    .expect("作業ツリーを走査できる");

    assert_eq!(
        scan.candidate_pairs().len(),
        6,
        "4 つのどの 2 つも候補になる"
    );
    assert_eq!(scan.changed_chunk_count(), None, "scan は差分で絞らない");
}

#[test]
fn test_check_without_changes_lists_no_pairs_and_starts_no_server() {
    let root = committed_work_tree("unchanged", &FILES);

    let check = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(0));
    assert!(check.candidate_pairs().is_empty());
    assert!(
        check.semantics_errors().is_empty(),
        "候補が無ければサーバを起こさない（起動できないサーバを渡しているので、起こせば失敗が残る）"
    );
}

#[test]
fn test_check_takes_an_untracked_file_as_changed() {
    // `git diff HEAD` は新しく作ったファイルを出さない。足したばかりの関数が既存のコードと
    // 重複しているかを見るのが、このコマンドの主な使い道
    let root = committed_work_tree("untracked", &FILES[..2]);
    write_file(&root, "src/shipping/fee.ts", FEE);

    let check = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(check.candidate_pairs().len(), 2);
    assert!(
        check
            .candidate_pairs()
            .iter()
            .all(|pair| includes_file(pair, "src/shipping/fee.ts")),
        "足したファイルの関数を起点に比べる: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_against_an_earlier_revision_takes_committed_changes_as_well() {
    // 既定の HEAD と違う答えになる入力。変更をコミットしてしまうと HEAD とは差が無いが、
    // 1 つ前と比べれば変わっている
    let root = committed_work_tree("revision", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["commit", "--quiet", "--all", "--message", "edit"]);

    let against_head = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");
    let against_parent = check_of(&root, "HEAD~1").expect("作業ツリーの差分を取れる");

    assert_eq!(against_head.changed_chunk_count(), Some(0));
    assert_eq!(against_parent.changed_chunk_count(), Some(1));
}

#[test]
fn test_check_outside_a_git_work_tree_reports_it() {
    let root = std::env::temp_dir().join(format!("dryguard-check-no-git-{}", process::id()));
    fs::create_dir_all(&root).expect("テストが作る作業ディレクトリは書ける");

    let result = check_of(&root, "HEAD");

    assert!(
        matches!(
            result,
            Err(CheckError::Diff(DiffError::NotAWorkTree { .. }))
        ),
        "差分を取れないまま 0 件として返さない: {result:?}"
    );
}

#[test]
fn test_check_against_an_unknown_revision_reports_it() {
    let root = committed_work_tree("unknown-revision", &FILES[..1]);

    let result = check_of(&root, "no-such-branch");

    assert!(
        matches!(result, Err(CheckError::Diff(DiffError::UnknownBase { .. }))),
        "解決できないリビジョンを空の差分として扱わない: {result:?}"
    );
}

#[test]
fn test_check_reports_how_many_chunks_the_changes_touched() {
    // 候補ペアの数だけでは「変更が無かった」と「似たコードが無かった」を読み分けられない
    let root = committed_work_tree("report", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);

    let check = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");

    let text = scan_text_of(&check, Explanation::AskedSignals);
    assert!(
        text.contains("チャンク 4 件 / 変更を含むチャンク 1 件 / 比較 3 ペア"),
        "変わったチャンクの数と、それを含む組だけを比べた数が読める: {text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&scan_json_of(&check, Explanation::AskedSignals))
            .expect("組み立てた綴りは JSON として読み直せる");
    assert_eq!(json["walked"]["changed_chunks"], 1);
    assert_eq!(json["walked"]["compared_pairs"], 3);
}

#[test]
fn test_check_from_a_subdirectory_finds_the_changes_under_it() {
    // 差分のパスを根からの相対で読めているかを見る。リポジトリの最上位からのパスで読むと
    // 走査したパスと引き当たらず、変わったチャンクが 0 件のまま成功する
    let nested: Vec<(String, &str)> = FILES
        .iter()
        .map(|(path, source)| (format!("package/{path}"), *source))
        .collect();
    let nested: Vec<(&str, &str)> = nested
        .iter()
        .map(|(path, source)| (path.as_str(), *source))
        .collect();
    let root = committed_work_tree("subdirectory", &nested);
    write_file(&root, "package/src/billing/discount.ts", DISCOUNT_EDITED);

    let check = check_of(&root.join("package"), "HEAD").expect("作業ツリーの差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(check.candidate_pairs().len(), 3);
}
