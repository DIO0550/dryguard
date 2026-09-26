//! `dryguard` のエントリポイント。
//!
//! 引数を解釈して、判定ラベル・位置・シグナルごとの根拠・提案を表示する
//! （`docs/dryguard-plan.md`「CLI仕様 (案)」の出力イメージ）。
//! `--format` が人の読む text とエージェントの読む JSON を切り替える。
//!
//! 判定に当てる閾値は `--threshold` > `dryguard.toml` > 既定値の順に決まる
//! （[`configured_thresholds_of`]）。ドメインの宣言は `dryguard.toml` の `[domains]` だけが持つ。

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use dryguard::classification::{ConfiguredThresholds, classification_of};
use dryguard::cli::{Cli, Command, CommonOptions, OutputFormat};
use dryguard::config::configuration_of;
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::location::Location;
use dryguard::lsp::ServerCommand;
use dryguard::pipeline::{chunk_pair_of, measured_pair_of, scan_of};
use dryguard::report::{Explanation, json_of, scan_json_of, scan_text_of, text_of};

fn main() -> ExitCode {
    let cli = Cli::parse();

    // **判定を始める前に落とす。** 設定を読めないまま既定値で走らせると、
    // 置いた設定が効いていないことを出力から確かめられない
    let configuration = match configuration_of(Path::new(CONFIG_DIRECTORY)) {
        Ok(configuration) => configuration,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
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
        Command::Scan { path } => report_scan(path, &cli.options, &settings),
    }
}

/// 判定に当てる閾値と、ドメインの宣言。
///
/// [`report_compare`] と [`report_scan`] の引数をまとめるためだけの型。
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
/// チャンクを取れなかったときは終了コードを 1 にする。切り出せなかったことを
/// 成功として返すと、後段が「似ていない」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// **LSP サーバを使えなくても失敗にしない。** Stage 1 のシグナルだけで判定でき、
/// 取れなかったことは判定の根拠に出る。理由だけを stderr へ回すのは、
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
            return ExitCode::FAILURE;
        }
    };

    let measured = measured_pair_of(
        &pair,
        settings.thresholds,
        settings.declarations,
        &ServerCommand::typescript(),
    );
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

    ExitCode::SUCCESS
}

/// `scan` の対象ディレクトリを走査して、候補ペアを理由付きで表示する。
///
/// 走査そのものが始められなかったときだけ終了コードを 1 にする。読めなかった
/// 1 ファイルで全体を失敗にすると、出せていた候補ペアまで捨てることになる
/// （飛ばしたものは出力に残る）。
///
/// **LSP サーバを使えなくても失敗にしない。** 理由を stderr へ回す分担は
/// [`report_compare`] と同じ（stdout は判定の出力に保つ）。
fn report_scan(root: &Path, options: &CommonOptions, settings: &Settings<'_>) -> ExitCode {
    let scan = match scan_of(
        root,
        settings.thresholds,
        settings.declarations,
        &ServerCommand::typescript(),
    ) {
        Ok(scan) => scan,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(error) = scan.semantics_error() {
        // **どのシグナルが取れなかったかはここで言わない。** 候補ペアごとに
        // 効いたシグナルが違うので、数え上げると判定の根拠と食い違う。
        eprintln!("LSP への問い合わせが最後まで通りませんでした: {error}");
    }

    let explanation = explanation_of(options);

    let report = match options.format {
        OutputFormat::Text => scan_text_of(&scan, explanation),
        OutputFormat::Json => scan_json_of(&scan, explanation),
    };
    println!("{report}");

    ExitCode::SUCCESS
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
