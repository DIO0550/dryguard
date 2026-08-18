//! `dryguard` のエントリポイント。
//!
//! Phase 0 の骨格。引数を解釈して、切り出したチャンク・測ったシグナル・
//! 判定ラベルを表示する。理由と提案まで含めた出力は次
//! (`docs/dryguard-plan.md`「CLI仕様 (案)」の出力イメージ)。

use std::process::ExitCode;

use clap::Parser;

use dryguard::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::cli::{Cli, Command, CommonOptions};
use dryguard::location::Location;
use dryguard::pipeline::{chunk_pair_of, signals_of};
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

    let chunks = chunk_pair_of(location_a, location_b);
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

    let signals = signals_of(&chunk_a, &chunk_b);
    println!();
    report_signals(&signals);

    let threshold = options
        .threshold
        .unwrap_or(DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    let classification = classification_of(&signals, threshold);

    println!();
    println!("判定: {}", classification.verdict());

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

    // 既定値のときも数値を出す。「既定値」とだけ書くと、指定が無かったことは
    // 分かっても、この実行がどの値で判定したのかが読めない
    match options.threshold {
        Some(threshold) => println!("  threshold: {threshold}"),
        None => println!("  threshold: {DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD} (既定値)"),
    }

    println!("  explain: {}", options.explain);

    match options.fail_on {
        Some(fail_on) => println!("  fail-on: {fail_on:?}"),
        None => println!("  fail-on: なし"),
    }
}

/// 判定の材料として測ったシグナルを表示する。
///
/// 測れなかったシグナルは、値の代わりに測れなかったことを出す。0.00 で埋めると、
/// 読む側が「そういう値だった」と「見ていない」を区別できない
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// モジュール距離は「近い / 遠い」ではなく段数をそのまま出す。何段を遠いと
/// みなすかは判定なので、ここでは言わない
/// (rules/architecture.md「判定は 1 箇所にだけ置く」)。
fn report_signals(signals: &Signals) {
    match signals.structural_similarity() {
        StructuralSimilarity::Measured(similarity) => {
            println!("  構造類似度: {similarity}")
        }
        StructuralSimilarity::NoTokens => {
            println!("  構造類似度: 取れません (トークンが 1 つも無い)")
        }
    }

    match signals.import_overlap() {
        ImportOverlap::Measured(overlap) => println!("  依存モジュールの重なり: {overlap}"),
        ImportOverlap::NoImports => {
            println!("  依存モジュールの重なり: 取れません (import が無いファイルがある)")
        }
    }

    println!("  モジュール距離: {} 段", signals.module_distance().steps());
}

/// 切り出したチャンクを 1 行で表示する。
///
/// 全文ではなく先頭行だけを出す。ここは切り出せたことを確かめるための表示で、
/// 理由付きの出力は判定と一緒に作る。
fn report_chunk(chunk: &Chunk) {
    let head = chunk.source().lines().next().unwrap_or_default();
    println!("  {} {} | {}", chunk.path().display(), chunk.lines(), head);
}
