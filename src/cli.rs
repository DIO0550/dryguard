//! コマンドラインの受け口。

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::location::Location;
use crate::threshold::Threshold;

/// `dryguard` のコマンドライン。
#[derive(Debug, Parser)]
#[command(
    name = "dryguard",
    version,
    about = "構造の似たコードが偶発的な重複かどうかを、理由付きで判定する"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub options: CommonOptions,
}

/// サブコマンド。
///
/// `scan` / `check --diff` は Phase 1 / Phase 5 で足す。**受け取るだけで何もしない
/// サブコマンドを先に生やさない** — 実行できるのに結果が出ないコマンドは、
/// 使う側が「壊れている」と「未実装」を区別できない。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 特定の 2 関数を比較する
    Compare {
        /// 比較元の位置（file:line）
        location_a: Location,
        /// 比較先の位置（file:line）
        location_b: Location,
    },
}

/// サブコマンドをまたいで使うオプション。
///
/// すべて `global = true`。そうしないと**サブコマンドより前にしか書けない**
/// (`dryguard compare a.ts:1 b.ts:2 --lang ts` が「予期しない引数」で落ちる)。
/// 使う側はサブコマンドの後ろに書くほうが自然なので、両方の位置を受ける。
#[derive(Debug, Args)]
pub struct CommonOptions {
    /// 対象言語
    #[arg(long, value_enum, global = true, default_value_t = LanguageOption::Auto)]
    pub lang: LanguageOption,

    /// 出力形式
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// 構造類似度の閾値（0.0-1.0）。既定値を上書きする
    #[arg(long, global = true)]
    pub threshold: Option<Threshold>,

    /// 判定根拠のシグナル値を全表示する
    #[arg(long, global = true)]
    pub explain: bool,

    /// 指定したラベルが出たら終了コードを 1 にする
    #[arg(long, value_enum, global = true)]
    pub fail_on: Option<FailOn>,
}

/// `--lang` が取る値。
///
/// `rust` は Phase 4 で足す。**まだ動かない選択肢をヘルプに並べない** —
/// 選べるのに結果が変わらない値があると、使う側は指定が効いていないのか
/// 未対応なのかを区別できない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LanguageOption {
    /// TypeScript
    Ts,
    /// 対象から判定する
    Auto,
}

/// `--format` が取る値。
///
/// `json` は Phase 3 で足す。判定ルールが整理される前に出力形式を固めると、
/// シグナルの構造が変わるたびに読む側が壊れる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// 人が読む形式
    Text,
}

/// `--fail-on` が取る値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOn {
    /// 共通化すべきでないペアがあれば失敗にする
    DoNotExtract,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("テストが渡す引数は解釈できる")
    }

    #[test]
    fn test_compare_with_two_locations_parses_both_positions() {
        let cli = parse(&["dryguard", "compare", "a.ts:10", "b.ts:20"]);

        let Command::Compare {
            location_a,
            location_b,
        } = &cli.command;
        assert_eq!(location_a.path(), Path::new("a.ts"));
        assert_eq!(location_b.path(), Path::new("b.ts"));
    }

    #[test]
    fn test_compare_without_options_falls_back_to_auto_and_text() {
        let cli = parse(&["dryguard", "compare", "a.ts:10", "b.ts:20"]);

        assert_eq!(cli.options.lang, LanguageOption::Auto);
        assert_eq!(cli.options.format, OutputFormat::Text);
        assert_eq!(cli.options.threshold, None);
        assert!(!cli.options.explain);
        assert_eq!(cli.options.fail_on, None);
    }

    #[test]
    fn test_compare_with_lang_option_overrides_the_default() {
        // 既定は Auto なので、既定と違う値を選ばないと指定が効いたか分からない
        let cli = parse(&["dryguard", "compare", "a.ts:10", "b.ts:20", "--lang", "ts"]);

        assert_eq!(cli.options.lang, LanguageOption::Ts);
    }

    #[test]
    fn test_compare_with_threshold_option_keeps_the_value() {
        let cli = parse(&[
            "dryguard",
            "compare",
            "a.ts:10",
            "b.ts:20",
            "--threshold",
            "0.75",
        ]);

        assert_eq!(cli.options.threshold.map(Threshold::value), Some(0.75));
    }

    #[test]
    fn test_compare_with_out_of_range_threshold_is_rejected() {
        let result = Cli::try_parse_from([
            "dryguard",
            "compare",
            "a.ts:10",
            "b.ts:20",
            "--threshold",
            "1.5",
        ]);

        assert!(result.is_err(), "1.5 は 0.0-1.0 の範囲外なので受け付けない");
    }

    #[test]
    fn test_compare_with_explain_and_fail_on_keeps_both() {
        let cli = parse(&[
            "dryguard",
            "compare",
            "a.ts:10",
            "b.ts:20",
            "--explain",
            "--fail-on",
            "do-not-extract",
        ]);

        assert!(cli.options.explain);
        assert_eq!(cli.options.fail_on, Some(FailOn::DoNotExtract));
    }

    #[test]
    fn test_compare_with_malformed_location_is_rejected() {
        let result = Cli::try_parse_from(["dryguard", "compare", "a.ts", "b.ts:20"]);

        assert!(
            result.is_err(),
            "file:line の形になっていない位置は受け付けない"
        );
    }

    #[test]
    fn test_compare_with_zero_line_is_rejected() {
        let result = Cli::try_parse_from(["dryguard", "compare", "a.ts:0", "b.ts:20"]);

        assert!(result.is_err(), "行番号は 1 始まりなので 0 は受け付けない");
    }
}
