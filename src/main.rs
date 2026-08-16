//! `dryguard` のエントリポイント。
//!
//! Phase 0 の骨格。引数を解釈して、この実行で使う設定と切り出せたチャンクを
//! 表示するところまでを担う。判定そのものは Stage 3 が入ってから
//! (`docs/dryguard-plan.md`「Phase 0: 貫通させる (LSPなし)」)。

use std::process::ExitCode;

use clap::Parser;

use dryguard::cli::{Cli, Command, CommonOptions};
use dryguard::location::Location;
use dryguard::pipeline::collect_chunks;
use dryguard::syntax::chunk::Chunk;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Command::Compare {
            location_a,
            location_b,
        } => report_compare(location_a, location_b, &cli.options),
    }
}

/// `compare` が受け取った内容と、そこから切り出せたチャンクを表示する。
///
/// チャンクを取れなかったときは終了コードを 1 にする。切り出せなかったことを
/// 成功として返すと、後段が「似ていない」と「見ていない」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
fn report_compare(
    location_a: &Location,
    location_b: &Location,
    options: &CommonOptions,
) -> ExitCode {
    report_options(location_a, location_b, options);

    let chunks = collect_chunks(location_a, location_b);
    let (chunk_a, chunk_b) = match chunks {
        Ok(chunks) => chunks,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    println!();
    report_chunk(&chunk_a);
    report_chunk(&chunk_b);

    println!();
    println!("判定は未実装です (Stage 3 が入ってから)。");

    ExitCode::SUCCESS
}

/// この実行で使う設定を表示する。
///
/// 判定が入るまでの間、**指定したオプションが黙って捨てられていないこと**を
/// 目で確かめられるようにしておく。受け取るだけで痕跡の残らないオプションは、
/// 効いていないのか未実装なのかを使う側が区別できない。
fn report_options(location_a: &Location, location_b: &Location, options: &CommonOptions) {
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
}

/// 切り出したチャンクを 1 行で表示する。
///
/// 全文ではなく先頭行だけを出す。ここは切り出せたことを確かめるための表示で、
/// 理由付きの出力は判定と一緒に作る。
fn report_chunk(chunk: &Chunk) {
    let head = chunk.source().lines().next().unwrap_or_default();
    println!("  {} {} | {}", chunk.path().display(), chunk.lines(), head);
}
