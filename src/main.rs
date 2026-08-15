//! `dryguard` のエントリポイント。
//!
//! Phase 0 の骨格。引数を解釈して、この実行で使う設定を表示するところまでを担う。
//! 判定そのものは Stage 1 / Stage 3 が入ってから
//! (`docs/dryguard-plan.md`「Phase 0: 貫通させる (LSPなし)」)。

use std::process::ExitCode;

use clap::Parser;

use dryguard::cli::{Cli, Command, CommonOptions};
use dryguard::location::Location;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Command::Compare {
            location_a,
            location_b,
        } => report_compare(location_a, location_b, &cli.options),
    }

    ExitCode::SUCCESS
}

/// `compare` が受け取った内容を表示する。
///
/// 判定が入るまでの間、**指定したオプションが黙って捨てられていないこと**を
/// 目で確かめられるようにしておく。受け取るだけで痕跡の残らないオプションは、
/// 効いていないのか未実装なのかを使う側が区別できない。
fn report_compare(location_a: &Location, location_b: &Location, options: &CommonOptions) {
    println!("compare {location_a} <-> {location_b}");
    println!("  lang: {:?}", options.lang);
    println!("  format: {:?}", options.format);

    match options.threshold {
        Some(threshold) => println!("  threshold: {threshold}"),
        None => println!("  threshold: 既定値"),
    }

    println!("  explain: {}", options.explain);

    match options.fail_on {
        Some(fail_on) => println!("  fail-on: {fail_on:?}"),
        None => println!("  fail-on: なし"),
    }

    println!();
    println!("判定は未実装です (Stage 1 / Stage 3 が入ってから)。");
}
