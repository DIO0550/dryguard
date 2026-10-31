//! `dryguard` のエントリポイント。
//!
//! 引数を解釈して、判定ラベル・位置・シグナルごとの根拠・提案を表示する
//! （`docs/dryguard-plan.md`「CLI仕様 (案)」の出力イメージ）。
//! `--format` が人の読む text とエージェントの読む JSON を切り替える。
//!
//! 終了コードは 3 つ（[`FAILED_ON_VERDICT`] / [`NOT_CLASSIFIED`] と成功）。
//! `--fail-on` を付けたときは、LSP に尋ね切れなかったことも [`NOT_CLASSIFIED`] にする
//! （[`exit_code_of`]）。
//!
//! 判定に当てる閾値は `--threshold` > `dryguard.toml` > 既定値の順に決まる
//! （[`configured_thresholds_of`]）。ドメインの宣言は `dryguard.toml` の `[domains]` だけが持つ。

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use dryguard::classification::verdict::Verdict;
use dryguard::classification::{ConfiguredThresholds, classification_of};
use dryguard::cli::{Cli, Command, CommonOptions, FailOn, LanguageOption, OutputFormat};
use dryguard::codebase::SourceLanguage;
use dryguard::codebase::diff::{DiffSides, DiffTarget};
use dryguard::config::configuration_of;
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::location::Location;
use dryguard::lsp::ServerCommand;
use dryguard::pipeline::{
    LanguageServers, Scan, check_with_language_servers, chunk_pair_of,
    measured_pair_with_language_servers, scan_with_language_servers,
};
use dryguard::report::{Explanation, json_of, scan_json_of, scan_text_of, text_of};
use dryguard::syntax::chunk::TestFunctions;
use dryguard::syntax::tree::Grammar;

fn main() -> ExitCode {
    let cli = Cli::parse();

    // **判定を始める前に落とす。** 設定を読めないまま既定値で走らせると、
    // 置いた設定が効いていないことを出力から確かめられない
    let configuration = match configuration_of(Path::new(CONFIG_DIRECTORY)) {
        Ok(configuration) => configuration,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(NOT_CLASSIFIED);
        }
    };
    let settings = Settings {
        thresholds: configured_thresholds_of(configuration.thresholds(), &cli.options),
        declarations: configuration.domain_declarations(),
    };

    match &cli.command {
        Command::Compare {
            location_a,
            location_b,
        } => report_compare(location_a, location_b, &cli.options, &settings),
        Command::Scan {
            path,
            include_tests,
        } => report_scan(
            path,
            test_functions_of(*include_tests),
            &cli.options,
            &settings,
        ),
        Command::Check {
            diff,
            staged,
            include_tests,
        } => report_check(
            &DiffSides::new(diff.clone(), diff_target_of(*staged)),
            test_functions_of(*include_tests),
            &cli.options,
            &settings,
        ),
    }
}

/// `--include-tests` の有無から、`test function` をチャンクにするか。
fn test_functions_of(include_tests: bool) -> TestFunctions {
    if include_tests {
        return TestFunctions::Included;
    }
    TestFunctions::Excluded
}

/// `--staged` の有無から、リビジョンと比べる側。
fn diff_target_of(staged: bool) -> DiffTarget {
    if staged {
        return DiffTarget::Index;
    }
    DiffTarget::WorkTree
}

/// `check --diff` が走査する根。
///
/// [`CONFIG_DIRECTORY`] と同じカレントディレクトリにする。差分を取る範囲と設定を探す場所が
/// 食い違うと、変更を読んだ設定と違う設定で判定することになる。
const CHECK_ROOT: &str = ".";

/// 判定を出し終えたうえで、`--fail-on` に指定した判定が 1 件以上出た。
///
/// 計画書と README が書いてきた「非推奨ペアがあれば exit 1」をそのまま使う。
const FAILED_ON_VERDICT: u8 = 1;

/// 判定を出せなかった（設定を読めない・チャンクを切り出せない・走査を始められないなど）か、
/// `--fail-on` を付けて候補ペアの Stage 2 を LSP に尋ね切れなかった。
///
/// 後者は判定を stdout に出し終えているが、**CI にとってはどちらも「答えとして読めない回」**で、
/// 直す先は stderr の理由が持つ。だから同じコードにし、名前も分けない。
///
/// **[`FAILED_ON_VERDICT`] と同じコードにしない。** 同じだと CI が「非推奨ペアがあった」と
/// 「検査できなかった」を区別できない。2 にするのは、clap が引数の誤りを 2 で落とすのと揃えるため。
const NOT_CLASSIFIED: u8 = 2;

/// 判定に当てる閾値と、ドメインの宣言。
///
/// [`report_compare`] / [`report_scan`] / [`report_check`] の引数をまとめるためだけの型。
struct Settings<'a> {
    thresholds: ConfiguredThresholds,
    declarations: &'a DomainDeclarations,
}

/// 設定ファイルを探すディレクトリ。
///
/// **カレントディレクトリの 1 つだけを見る。** 上へ遡って探すと、立っている場所しだいで
/// **リポジトリの外の設定が黙って効く**。`scan <path>` でも `<path>` 側を見ないのは、
/// `compare` には根が無く、サブコマンドによって置き場所が変わることになるため。
const CONFIG_DIRECTORY: &str = ".";

/// `compare` の 2 箇所を判定して、理由付きで表示する。
///
/// チャンクを取れなかったとき・比べるファイルが名前の違う 2 つの宣言に当たったときは
/// 終了コードを [`NOT_CLASSIFIED`] にする。切り出せなかったことを
/// 成功として返すと、後段が「似ていない」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// **`--fail-on` が無ければ、LSP サーバを使えなくても失敗にしない。** Stage 1 のシグナルだけで
/// 判定でき、取れなかったことは判定の根拠に出る。理由だけを stderr へ回すのは、
/// **判定そのものではなく環境の話**だから（stdout は判定の出力に保つ）。
/// 片方の問い合わせだけが落ちることもあるので、**stderr でシグナルを数え上げない**。
///
/// 出力の組み立ては `dryguard::report` にある。ここでは stdout / stderr の
/// どちらへ出すかと、終了コードだけを決める。
fn report_compare(
    location_a: &Location,
    location_b: &Location,
    options: &CommonOptions,
    settings: &Settings<'_>,
) -> ExitCode {
    let pair = match chunk_pair_of(location_a, location_b) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(NOT_CLASSIFIED);
        }
    };
    if !accepts_grammar(options.lang, pair.chunk_a().grammar()) {
        eprintln!("指定した --lang と比較するファイルの言語が一致しません");
        return ExitCode::from(NOT_CLASSIFIED);
    }

    // **宣言の食い違いは判定を出さずに止める。** 直す先は dryguard.toml で、
    // どちらかの宣言へ寄せた判定を出すと、並べ替えただけで答えが変わる
    let measured = match measured_pair_with_language_servers(
        &pair,
        settings.thresholds,
        settings.declarations,
        LanguageServers::new(&language_servers()),
    ) {
        Ok(measured) => measured,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(NOT_CLASSIFIED);
        }
    };
    if let Some(error) = measured.semantics_error() {
        // **どのシグナルが取れなかったかはここで言わない。** 片方だけ落ちることが
        // あるので数え上げると判定の根拠と食い違う。取れなかったシグナルは
        // 根拠の行が 1 つずつ出す。
        eprintln!("LSP への問い合わせが最後まで通りませんでした: {error}");
    }

    let classification = classification_of(measured.signals(), settings.thresholds);
    let explanation = explanation_of(options);

    let report = match options.format {
        OutputFormat::Text => text_of(location_a, location_b, &classification, explanation),
        OutputFormat::Json => json_of(location_a, location_b, &classification, explanation),
    };
    println!("{report}");

    exit_code_of(
        options.fail_on,
        [classification.verdict()],
        semantics_coverage_of(measured.semantics_error().is_some()),
    )
}

/// `scan` の対象ディレクトリを走査して、候補ペアを理由付きで表示する。
///
/// 走査そのものが始められなかったときだけ終了コードを [`NOT_CLASSIFIED`] にする。読めなかった
/// 1 ファイルで全体を失敗にすると、出せていた候補ペアまで捨てることになる
/// （飛ばしたものは出力に残る）。
///
/// LSP サーバを使えなかったときの扱いと、理由を stderr へ回す分担は
/// [`report_compare`] と同じ（stdout は判定の出力に保つ）。
fn report_scan(
    root: &Path,
    test_functions: TestFunctions,
    options: &CommonOptions,
    settings: &Settings<'_>,
) -> ExitCode {
    let scan = match scan_with_language_servers(
        root,
        source_language_of(options.lang),
        test_functions,
        settings.thresholds,
        settings.declarations,
        LanguageServers::new(&language_servers()),
    ) {
        Ok(scan) => scan,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(NOT_CLASSIFIED);
        }
    };
    report_walked(&scan, options)
}

/// `check --diff` の対象を、`sides` の差分が変えたチャンクに絞って、
/// 候補ペアを理由付きで表示する。
///
/// 終了コードの決め方は [`report_scan`] と同じ。差分を取れないとき（git が無い・
/// git の作業ツリーでない・リビジョンを解決できない・index に未解決の衝突がある）も
/// [`NOT_CLASSIFIED`] にする。
/// **差分を取れないまま 0 件として成功させると、フックが「重複は無い」と読む。**
fn report_check(
    sides: &DiffSides,
    test_functions: TestFunctions,
    options: &CommonOptions,
    settings: &Settings<'_>,
) -> ExitCode {
    let scan = match check_with_language_servers(
        Path::new(CHECK_ROOT),
        sides,
        source_language_of(options.lang),
        test_functions,
        settings.thresholds,
        settings.declarations,
        LanguageServers::new(&language_servers()),
    ) {
        Ok(scan) => scan,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(NOT_CLASSIFIED);
        }
    };
    report_walked(&scan, options)
}

/// 言語ごとに起こす LSP サーバ。**言語を足すときはここに 1 行足す。**
///
/// 並びの順が、サーバを使えなかった理由を stderr に出す順になる。
fn language_servers() -> Vec<ServerCommand> {
    vec![
        ServerCommand::typescript(),
        ServerCommand::rust(),
        ServerCommand::swift(),
        ServerCommand::java(),
    ]
}

/// `--lang` から、走査するソースの言語。
fn source_language_of(lang: LanguageOption) -> SourceLanguage {
    match lang {
        LanguageOption::Ts => SourceLanguage::TypeScript,
        LanguageOption::Rust => SourceLanguage::Rust,
        LanguageOption::Swift => SourceLanguage::Swift,
        LanguageOption::Java => SourceLanguage::Java,
        LanguageOption::Auto => SourceLanguage::Auto,
    }
}

/// `compare` で、`--lang` が比べるファイルの grammar を受け付けるか。
///
/// grammar 側も網羅 `match` にする。「Rust 以外」の否定で書くと、grammar を足したときに
/// `--lang ts` が黙って受け付ける。
///
/// Why not（`SourceLanguage::accepts` を引く）: あちらは走査に入れる grammar で、
/// `auto` が Swift / Java を拾わない。`compare` は名指しされれば `auto` でも比べる。
fn accepts_grammar(lang: LanguageOption, grammar: Grammar) -> bool {
    match (lang, grammar) {
        (
            LanguageOption::Auto,
            Grammar::TypeScript | Grammar::Tsx | Grammar::Rust | Grammar::Java | Grammar::Swift,
        ) => true,
        (LanguageOption::Ts, Grammar::TypeScript | Grammar::Tsx) => true,
        (LanguageOption::Ts, Grammar::Rust | Grammar::Java | Grammar::Swift) => false,
        (LanguageOption::Rust, Grammar::Rust) => true,
        (
            LanguageOption::Rust,
            Grammar::TypeScript | Grammar::Tsx | Grammar::Java | Grammar::Swift,
        ) => false,
        (LanguageOption::Swift, Grammar::Swift) => true,
        (
            LanguageOption::Swift,
            Grammar::TypeScript | Grammar::Tsx | Grammar::Rust | Grammar::Java,
        ) => false,
        (LanguageOption::Java, Grammar::Java) => true,
        (
            LanguageOption::Java,
            Grammar::TypeScript | Grammar::Tsx | Grammar::Rust | Grammar::Swift,
        ) => false,
    }
}

/// 走査した結果を出し、終了コードを決める。`scan` と `check` で共通。
fn report_walked(scan: &Scan, options: &CommonOptions) -> ExitCode {
    for error in scan.semantics_errors() {
        // **どのシグナルが取れなかったかはここで言わない。** 候補ペアごとに
        // 効いたシグナルが違うので、数え上げると判定の根拠と食い違う。
        eprintln!("LSP への問い合わせが最後まで通りませんでした: {error}");
    }

    let explanation = explanation_of(options);

    let report = match options.format {
        OutputFormat::Text => scan_text_of(scan, explanation),
        OutputFormat::Json => scan_json_of(scan, explanation),
    };
    println!("{report}");

    let verdicts = scan
        .candidate_pairs()
        .iter()
        .map(|pair| pair.classification().verdict());
    exit_code_of(
        options.fail_on,
        verdicts,
        semantics_coverage_of(!scan.semantics_errors().is_empty()),
    )
}

/// 候補ペアの Stage 2 を LSP に尋ね切れたか。
///
/// 数えるのは LSP とのやりとりが通らなかったこと（サーバが見つからない・握手に失敗した・
/// 根を決められない・問い合わせ 1 本の往復が落ちたなど。`SemanticsError` が持つもの）だけ。
/// 候補ペアでないので尋ねなかったペアと、サーバは答えたがシグナルが「測れない」だった
/// ペアは数えない。前者は Stage 2 で判定が変わらず、後者は環境が正常でも起きるので、
/// 数えると `--fail-on` が通る回が無くなる。
///
/// **サーバを並べていない言語の候補ペアも数えない**（尋ねずに `NotAsked` になる）。
/// 言語を足すときにサーバを [`language_servers`] に並べ忘れると、その言語は尋ねないまま通る。
#[derive(Debug, Clone, Copy)]
enum SemanticsCoverage {
    /// 尋ねるべき候補ペアに尋ね切れた（尋ねる候補ペアが無かった場合を含む）。
    Complete,
    /// どこかの LSP サーバに尋ね切れなかった。
    Incomplete,
}

/// LSP への問い合わせが通らなかったか（`has_semantics_error`）から、[`SemanticsCoverage`]。
fn semantics_coverage_of(has_semantics_error: bool) -> SemanticsCoverage {
    if has_semantics_error {
        return SemanticsCoverage::Incomplete;
    }
    SemanticsCoverage::Complete
}

/// 判定を出し終えたときの終了コード。`--fail-on` に指定した判定が 1 件でも出たら
/// [`FAILED_ON_VERDICT`]、出なければ成功。
///
/// **`--fail-on` を付けて LSP に尋ね切れなかったら、判定に関わらず [`NOT_CLASSIFIED`]。**
/// 成功にすると、検査されなかったのか通ったのかを CI が区別できない（フェイルオープン）。
/// [`FAILED_ON_VERDICT`] より優先するのは、それが「判定を出し終えたうえで」の約束だから。
/// 1 を返すと CI は非推奨ペアを直せばよいと読み、検査が欠けたことを見落とす。
///
/// 失敗にするときも**判定の出力は stdout に出し終えてから**にし、理由を stderr に 1 行出す。
/// stdout に混ぜると `--format json` を読む側が壊れる。
fn exit_code_of(
    fail_on: Option<FailOn>,
    verdicts: impl IntoIterator<Item = Verdict>,
    coverage: SemanticsCoverage,
) -> ExitCode {
    let Some(fail_on) = fail_on else {
        return ExitCode::SUCCESS;
    };
    let met_count = verdicts
        .into_iter()
        .filter(|verdict| fail_on.is_met_by(*verdict))
        .count();
    // 2 で終わるときも件数は出す。理由の行だけにすると、尋ね切れた分で既に見つかった
    // 非推奨ペアの数が CI のログから消える
    if met_count > 0 {
        eprintln!("--fail-on に指定した判定が {met_count} 件出ました");
    }

    match coverage {
        SemanticsCoverage::Incomplete => {
            eprintln!(
                "--fail-on の検査を終えられませんでした（LSP に尋ね切れなかった候補ペアがあります）"
            );
            ExitCode::from(NOT_CLASSIFIED)
        }
        SemanticsCoverage::Complete if met_count > 0 => ExitCode::from(FAILED_ON_VERDICT),
        SemanticsCoverage::Complete => ExitCode::SUCCESS,
    }
}

/// 根拠をどこまで出すか。`--explain` があれば、尋ねなかったシグナルと当てた閾値まで。
fn explanation_of(options: &CommonOptions) -> Explanation {
    if options.explain {
        return Explanation::AllSignals;
    }
    Explanation::AskedSignals
}

/// 判定に当てる閾値。**`--threshold` > `dryguard.toml` > 既定値**。
///
/// `from_file` は `dryguard.toml` の書かれていたキーを既定値へ重ねたもの。
///
/// 優先順位を条件で書かずに**重ねる順番**で表す。既定値から始めて、設定ファイルに
/// 書かれていたキーを重ね、最後に `--threshold` を重ねるので、後から重ねたほうが残る。
fn configured_thresholds_of(
    from_file: ConfiguredThresholds,
    options: &CommonOptions,
) -> ConfiguredThresholds {
    let Some(threshold) = options.threshold else {
        return from_file;
    };
    from_file.with_structural_similarity(threshold)
}
