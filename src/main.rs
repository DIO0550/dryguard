//! `dryguard` のエントリポイント。
//!
//! 引数を解釈して、判定ラベル・位置・シグナルごとの根拠・提案を text で表示する
//! （`docs/dryguard-plan.md`「CLI仕様 (案)」の出力イメージ）。

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::cli::{Cli, Command, CommonOptions};
use dryguard::location::Location;
use dryguard::lsp::ServerCommand;
use dryguard::pipeline::{chunk_pair_of, measured_pair_of, scan_of};
use dryguard::report::{scan_text_of, text_of};
use dryguard::threshold::Threshold;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Command::Compare {
            location_a,
            location_b,
        } => report_compare(location_a, location_b, &cli.options),
        Command::Scan { path } => report_scan(path, &cli.options),
    }
}

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
) -> ExitCode {
    let pair = match chunk_pair_of(location_a, location_b) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let threshold = threshold_of(options);
    let measured = measured_pair_of(&pair, threshold, &ServerCommand::typescript());
    if let Some(error) = measured.semantics_error() {
        // **どのシグナルが取れなかったかはここで言わない。** 片方だけ落ちることが
        // あるので数え上げると判定の根拠と食い違う。取れなかったシグナルは
        // 根拠の行が 1 つずつ出す。
        eprintln!("LSP への問い合わせが最後まで通りませんでした: {error}");
    }

    let classification = classification_of(measured.signals(), threshold);

    println!(
        "{}",
        text_of(location_a, location_b, &classification, threshold)
    );

    ExitCode::SUCCESS
}

/// `scan` の対象ディレクトリを走査して、候補ペアを理由付きで表示する。
///
/// 走査そのものが始められなかったときだけ終了コードを 1 にする。読めなかった
/// 1 ファイルで全体を失敗にすると、出せていた候補ペアまで捨てることになる
/// （飛ばしたものは出力に残る）。
fn report_scan(root: &Path, options: &CommonOptions) -> ExitCode {
    let threshold = threshold_of(options);

    let scan = match scan_of(root, threshold) {
        Ok(scan) => scan,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("{}", scan_text_of(&scan, threshold));

    ExitCode::SUCCESS
}

/// 判定に使う閾値。`--threshold` が無ければ既定値。
fn threshold_of(options: &CommonOptions) -> Threshold {
    options
        .threshold
        .unwrap_or(DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD)
}
