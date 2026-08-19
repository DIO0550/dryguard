//! `dryguard` のエントリポイント。
//!
//! Phase 0 の骨格。引数を解釈して、判定ラベル・位置・構造類似度・理由・提案を
//! text で表示する（`docs/dryguard-plan.md`「CLI仕様 (案)」の出力イメージ）。

use std::process::ExitCode;

use clap::Parser;

use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::cli::{Cli, Command, CommonOptions};
use dryguard::location::Location;
use dryguard::pipeline::{chunk_pair_of, signals_of};
use dryguard::report::text_of;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Command::Compare {
            location_a,
            location_b,
        } => report_compare(location_a, location_b, &cli.options),
    }
}

/// `compare` の 2 箇所を判定して、理由付きで表示する。
///
/// チャンクを取れなかったときは終了コードを 1 にする。切り出せなかったことを
/// 成功として返すと、後段が「似ていない」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// 出力の組み立ては `dryguard::report` にある。ここでは stdout / stderr の
/// どちらへ出すかと、終了コードだけを決める。
fn report_compare(
    location_a: &Location,
    location_b: &Location,
    options: &CommonOptions,
) -> ExitCode {
    let (chunk_a, chunk_b) = match chunk_pair_of(location_a, location_b) {
        Ok(chunks) => chunks,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let signals = signals_of(&chunk_a, &chunk_b);
    let threshold = options
        .threshold
        .unwrap_or(DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    let classification = classification_of(&signals, threshold);

    println!(
        "{}",
        text_of(location_a, location_b, &classification, threshold)
    );

    ExitCode::SUCCESS
}
