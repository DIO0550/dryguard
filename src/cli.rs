//! コマンドラインの受け口。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::classification::verdict::Verdict;
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
/// `check --diff` は Phase 5 で足す。**受け取るだけで何もしない
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
    /// コードベース全体をスキャンする
    Scan {
        /// 走査を始めるディレクトリ
        #[arg(default_value = DEFAULT_SCAN_ROOT)]
        path: PathBuf,
        // 既定で外すのは、テスト関数どうしが `fn()` の型シグネチャと呼び出し元の無さで
        // 空の一致を作り、本番コードの候補ペアを埋もれさせるため。`compare` は位置を
        // 名指しするので、このオプションを持たない（global にしない）
        /// `#[test]` の付いた Rust の関数も比較の対象に入れる
        #[arg(long)]
        include_tests: bool,
    },
}

/// `scan` の対象を省いたときに走査する場所。
///
/// 計画の `dryguard scan [path]` に合わせて省略できる形にする
/// （`docs/dryguard-plan.md`「CLI仕様 (案)」）。
const DEFAULT_SCAN_ROOT: &str = ".";

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LanguageOption {
    /// TypeScript
    Ts,
    /// Rust
    Rust,
    /// 対象から判定する
    Auto,
}

/// `--format` が取る値。
///
/// **JSON スキーマはまだ安定していない。** 形を固めるのは Phase 5 で、
/// それまではシグナルの構造が変わると読む側も追従することになる
/// （`docs/dryguard-plan.md`「Phase 5: エージェント連携」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// 人が読む形式
    Text,
    /// エージェントが読む形式
    Json,
}

/// `--fail-on` が取る値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOn {
    /// 共通化すべきでないペアがあれば失敗にする
    DoNotExtract,
}

impl FailOn {
    /// その判定が、この指定で失敗にする判定か。
    ///
    /// **判定そのものは `classification` が出したものを受け取るだけ。** ここが持つのは
    /// `--fail-on` の値がどの判定を指すかの解釈で、決定木には触らない
    /// （rules/architecture.md「判定は 1 箇所にだけ置く」）。
    pub fn is_met_by(self, verdict: Verdict) -> bool {
        match self {
            Self::DoNotExtract => verdict == Verdict::DoNotExtract,
        }
    }
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
        } = &cli.command
        else {
            panic!("compare を渡したので Compare になる");
        };
        assert_eq!(location_a.path(), Path::new("a.ts"));
        assert_eq!(location_b.path(), Path::new("b.ts"));
    }

    #[test]
    fn test_scan_with_a_path_walks_that_directory() {
        // 既定は "." なので、既定と違う値を渡さないと指定が効いたか分からない
        let cli = parse(&["dryguard", "scan", "src/billing"]);

        let Command::Scan { path, .. } = &cli.command else {
            panic!("scan を渡したので Scan になる");
        };
        assert_eq!(path, Path::new("src/billing"));
    }

    #[test]
    fn test_scan_without_a_path_walks_the_current_directory() {
        let cli = parse(&["dryguard", "scan"]);

        let Command::Scan { path, .. } = &cli.command else {
            panic!("scan を渡したので Scan になる");
        };
        assert_eq!(path, Path::new("."));
    }

    #[test]
    fn test_scan_without_include_tests_leaves_test_functions_out() {
        let cli = parse(&["dryguard", "scan", "src"]);

        let Command::Scan { include_tests, .. } = &cli.command else {
            panic!("scan を渡したので Scan になる");
        };
        assert!(!include_tests);
    }

    #[test]
    fn test_scan_with_include_tests_takes_test_functions_in() {
        // 既定は外す側なので、指定が効いたかは入れる側で確かめる
        let cli = parse(&["dryguard", "scan", "src", "--include-tests"]);

        let Command::Scan { include_tests, .. } = &cli.command else {
            panic!("scan を渡したので Scan になる");
        };
        assert!(include_tests);
    }

    #[test]
    fn test_compare_with_include_tests_is_rejected() {
        // `compare` は位置を名指しするので外すものが無い。受けて何もしない指定を作らない
        let parsed =
            Cli::try_parse_from(["dryguard", "compare", "a.rs:1", "b.rs:2", "--include-tests"]);

        assert!(parsed.is_err());
    }

    #[test]
    fn test_scan_with_a_threshold_option_keeps_the_value() {
        // オプションは global なのでサブコマンドの後ろに書ける
        let cli = parse(&["dryguard", "scan", "src", "--threshold", "0.75"]);

        assert_eq!(cli.options.threshold.map(Threshold::value), Some(0.75));
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
    fn test_compare_with_format_json_overrides_the_default() {
        // 既定は Text なので、既定と違う値を選ばないと指定が効いたか分からない
        let cli = parse(&[
            "dryguard", "compare", "a.ts:10", "b.ts:20", "--format", "json",
        ]);

        assert_eq!(cli.options.format, OutputFormat::Json);
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
    fn test_fail_on_do_not_extract_is_met_by_a_do_not_extract_verdict() {
        assert!(FailOn::DoNotExtract.is_met_by(Verdict::DoNotExtract));
    }

    #[test]
    fn test_fail_on_do_not_extract_is_not_met_by_the_other_verdicts() {
        assert!(!FailOn::DoNotExtract.is_met_by(Verdict::ExtractCandidate));
        assert!(!FailOn::DoNotExtract.is_met_by(Verdict::Review));
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
