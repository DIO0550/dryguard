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
use dryguard::codebase::diff::{DiffBase, DiffError, DiffSides, DiffTarget};
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
    assert!(
        git_succeeds(root, arguments),
        "git {arguments:?} が失敗した"
    );
}

/// テストの作業ツリーで git を走らせ、成功したか。失敗を前提にする手順（衝突するマージ）で使う。
fn git_succeeds(root: &Path, arguments: &[&str]) -> bool {
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
    status.success()
}

fn check_of(root: &Path, base: &str) -> Result<Scan, CheckError> {
    check_against(root, base, DiffTarget::WorkTree)
}

/// `check --diff <base> --staged`。index を `base` と比べる。
fn staged_check_of(root: &Path, base: &str) -> Result<Scan, CheckError> {
    check_against(root, base, DiffTarget::Index)
}

fn check_against(root: &Path, base: &str, target: DiffTarget) -> Result<Scan, CheckError> {
    let Ok(base) = base.parse::<DiffBase>() else {
        panic!("テストが渡すリビジョンは受け付ける綴り: {base}");
    };
    let server = missing_server();

    check_with_language_servers(
        root,
        &DiffSides::new(base, target),
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

/// 本体の前に 3 行のコメントを足した `DISCOUNT_EDITED`。構造は同じで、関数の始まりが 4 行目にずれる。
const DISCOUNT_EDITED_SHIFTED: &str = "// 1\n// 2\n// 3\n\
                                       export function applyDiscount(price: number, rate: number): number {\n\
                                       \x20 const discounted = price * (1 - rate);\n\
                                       \x20 return Math.max(discounted, 1);\n\
                                       }\n";

/// 候補ペアのうち、`file` 側の位置の行番号。
fn lines_of(check: &Scan, file: &str) -> Vec<usize> {
    check
        .candidate_pairs()
        .iter()
        .flat_map(|pair| [pair.location_a(), pair.location_b()])
        .filter(|location| location.path().ends_with(file))
        .map(|location| location.line().get())
        .collect()
}

#[test]
fn test_check_staged_reads_chunks_from_the_index_when_the_work_tree_has_unstaged_edits_on_top() {
    // `git add -p` の後の形。index の行番号で差分を取り、チャンクも index の版から切り出す。
    // 作業ツリーから切り出すと、関数が 4 行目にずれて index の差分の行と食い違う
    let root = committed_work_tree("staged-partial", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["add", "src/billing/discount.ts"]);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED_SHIFTED);

    let check = staged_check_of(&root, "HEAD").expect("index の差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(
        lines_of(&check, "src/billing/discount.ts"),
        vec![1, 1, 1],
        "位置は index の版の行番号を指す: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_staged_does_not_take_unstaged_edits_as_changed() {
    let root = committed_work_tree("staged-unstaged", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);

    let staged = staged_check_of(&root, "HEAD").expect("index の差分を取れる");
    // 対照。同じ作業ツリーを `--staged` なしで比べると、変わったチャンクになる
    let work_tree = check_of(&root, "HEAD").expect("作業ツリーの差分を取れる");

    assert_eq!(staged.changed_chunk_count(), Some(0));
    assert!(staged.candidate_pairs().is_empty());
    assert_eq!(work_tree.changed_chunk_count(), Some(1));
}

#[test]
fn test_check_staged_takes_a_staged_new_file_as_changed() {
    let root = committed_work_tree("staged-new", &FILES[..2]);
    write_file(&root, "src/shipping/fee.ts", FEE);
    git(&root, &["add", "src/shipping/fee.ts"]);

    let check = staged_check_of(&root, "HEAD").expect("index の差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(check.candidate_pairs().len(), 2);
    assert!(
        check
            .candidate_pairs()
            .iter()
            .all(|pair| includes_file(pair, "src/shipping/fee.ts")),
        "ステージした新しいファイルの関数を起点に比べる: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_staged_leaves_untracked_files_out_of_the_comparison() {
    // 追跡していないファイルはコミットに入らないので、起点にも比べる相手にもしない
    let root = committed_work_tree("staged-untracked", &FILES[..2]);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["add", "src/billing/discount.ts"]);
    write_file(&root, "src/shipping/fee.ts", FEE);

    let check = staged_check_of(&root, "HEAD").expect("index の差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(
        check.candidate_pairs().len(),
        1,
        "変えた関数と、追跡しているもう 1 つとの組だけが出る: {:?}",
        check.candidate_pairs()
    );
    assert!(
        !check
            .candidate_pairs()
            .iter()
            .any(|pair| includes_file(pair, "src/shipping/fee.ts")),
        "追跡していないファイルの関数は比べない: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_staged_compares_with_a_file_removed_only_from_the_work_tree() {
    // 比べる相手も index の版から読む。作業ツリーから消しただけ（ステージしていない）の
    // ファイルはコミットに残るので、相手に入る
    let root = committed_work_tree("staged-removed", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["add", "src/billing/discount.ts"]);
    fs::remove_file(root.join("src/inventory/reorder.ts")).expect("テストが作ったファイルは消せる");

    let check = staged_check_of(&root, "HEAD").expect("index の差分を取れる");

    assert_eq!(check.candidate_pairs().len(), 3);
    assert!(
        check
            .candidate_pairs()
            .iter()
            .any(|pair| includes_file(pair, "src/inventory/reorder.ts")),
        "作業ツリーに無くても index にあるファイルと比べる: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_staged_with_an_unresolved_merge_conflict_reports_it() {
    // 衝突したファイルは index に版が 3 つあり、どれを読むかが決まらない。コミットもできない
    let root = committed_work_tree("staged-conflict", &FILES[..1]);
    git(&root, &["checkout", "--quiet", "-b", "other"]);
    write_file(
        &root,
        "src/billing/discount.ts",
        &DISCOUNT_EDITED.replace("discounted, 1", "discounted, 2"),
    );
    git(&root, &["commit", "--quiet", "--all", "--message", "other"]);
    git(&root, &["checkout", "--quiet", "-"]);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["commit", "--quiet", "--all", "--message", "main"]);
    assert!(
        !git_succeeds(&root, &["merge", "--quiet", "other"]),
        "同じ行を両方で変えたので衝突する"
    );

    let result = staged_check_of(&root, "HEAD");

    assert!(
        matches!(
            result,
            Err(CheckError::Diff(DiffError::UnmergedPath { .. }))
        ),
        "衝突したまま、どれかの版で比べない: {result:?}"
    );
}

#[test]
fn test_check_staged_does_not_compare_with_a_file_removed_from_the_index() {
    // 対照は上のテスト。`git rm` で消したファイルはコミットに残らないので、相手に入れない
    let root = committed_work_tree("staged-git-rm", &FILES);
    write_file(&root, "src/billing/discount.ts", DISCOUNT_EDITED);
    git(&root, &["add", "src/billing/discount.ts"]);
    git(&root, &["rm", "--quiet", "src/inventory/reorder.ts"]);

    let check = staged_check_of(&root, "HEAD").expect("index の差分を取れる");

    assert_eq!(check.candidate_pairs().len(), 2);
    assert!(
        !check
            .candidate_pairs()
            .iter()
            .any(|pair| includes_file(pair, "src/inventory/reorder.ts")),
        "index から消したファイルとは比べない: {:?}",
        check.candidate_pairs()
    );
}

#[test]
fn test_check_staged_from_a_subdirectory_finds_the_changes_under_it() {
    // index のパスも根からの相対で読めているかを見る。差分のパスと揃っていなければ、
    // 変わったチャンクが 0 件のまま成功する
    let nested: Vec<(String, &str)> = FILES
        .iter()
        .map(|(path, source)| (format!("package/{path}"), *source))
        .collect();
    let nested: Vec<(&str, &str)> = nested
        .iter()
        .map(|(path, source)| (path.as_str(), *source))
        .collect();
    let root = committed_work_tree("staged-subdirectory", &nested);
    write_file(&root, "package/src/billing/discount.ts", DISCOUNT_EDITED);
    write_file(&root, "outside.ts", FEE);
    git(&root, &["add", "--all"]);

    let check = staged_check_of(&root.join("package"), "HEAD").expect("index の差分を取れる");

    assert_eq!(check.changed_chunk_count(), Some(1));
    assert_eq!(
        check.candidate_pairs().len(),
        3,
        "根の外にステージしたファイルは起点にも相手にもならない: {:?}",
        check.candidate_pairs()
    );
}
