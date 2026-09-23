//! `dryguard.toml` が書いた閾値。
//!
//! 読むのは **`[thresholds]` テーブルと、そこに並ぶ 3 つのキーだけ**。
//! `dryguard.toml` が無ければ既定値で、**無いことは正常**（設定を置かずに使える）。
//! 在るのに読めない・書式が違う・知らない綴りが書いてあるときは `Err` にする。
//!
//! **知らない綴りを黙って落とさない。** 落とすと、キーを打ち間違えた利用者には
//! **設定したのに効かない**として現れ、しかもそれを確かめる手立てが無い
//! (`AGENTS.md`「強制力の序列」のフェイルオープンかつサイレント)。
//! 一覧に無い綴りが `Err` へ倒れる向きにしてあるので、**綴りを足し忘れると
//! 通らなくなる**側で失敗する（`rules/coding.md`「列挙で判定を組むときは、
//! 漏れの倒れる向きを選ぶ」）。
//!
//! **既定値は持たない。** 書かれていなかったキーに何を当てるかは
//! `classification` が決める（`rules/architecture.md`「判定は 1 箇所にだけ置く」）。
//! ここが持つのは「書かれていたかどうか」と、書かれていた値。
//!
//! # Why not（`toml` クレートを使う）
//!
//! `toml` は serde と proc macro を引き込む。`harness/deps/README.md` が cooldown を
//! 置いている理由が「build script と proc macro は `cargo build` の時点で
//! ローカル実行される」なので、**数値 3 つを読むために実行される面積を増やす**のは
//! 釣り合わない。JSON-RPC のフレーミングを自前で持っているのと同じ判断
//! (`Cargo.toml` の `lsp-types` のコメント)。
//!
//! 代わりに読む範囲を上の一覧へ狭め、**TOML として正しくてもここが知らない書き方は
//! すべて `Err`** にしてある。`[domains]` のようなテーブルを足すときは、
//! **そのテーブルを読む側と同じ PR で**ここへ足すことになる。

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::classification::ConfiguredThresholds;
use crate::line_number::LineNumber;
use crate::threshold::{Threshold, ThresholdParseError};

/// 設定ファイルの名前。
pub const CONFIG_FILE_NAME: &str = "dryguard.toml";

/// 閾値を並べるテーブルの名前。
const THRESHOLDS_TABLE_NAME: &str = "thresholds";

/// 構造類似度の閾値のキー。
const STRUCTURAL_SIMILARITY_KEY: &str = "structural_similarity";

/// 依存先の重なりの閾値のキー。
const SHARED_IMPORTS_KEY: &str = "shared_imports";

/// 呼び出し元ドメインの重なりの閾値のキー。
const SHARED_CALLER_DOMAINS_KEY: &str = "shared_caller_domains";

/// 行末のコメントを始める綴り。
///
/// 値は数値しか取らないので、**文字列の中に現れる可能性が無い**。
/// TOML の文字列を読まない（読めば `Err`）から、最初の 1 つで切ってよい。
const COMMENT_MARK: char = '#';

/// `directory` の `dryguard.toml` を読んで、判定に当てる閾値にする。
///
/// 書かれていたキーだけが既定値を上書きする。ファイルが無ければ
/// [`ConfiguredThresholds::default`] をそのまま返す。
///
/// # Errors
///
/// ファイルが在るのに読めない（[`ConfigError::Unreadable`]）、
/// または書式が違う（[`ConfigError::Malformed`]）とき。
pub fn thresholds_of(directory: &Path) -> Result<ConfiguredThresholds, ConfigError> {
    let path = directory.join(CONFIG_FILE_NAME);

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        // 無いことは正常。**読めなかったことと分けて扱う** —
        // 権限が無いファイルを「無い」と同じにすると、置いたのに効かない形になる
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ConfiguredThresholds::default());
        }
        Err(error) => return Err(ConfigError::Unreadable { path, error }),
    };

    thresholds_of_text(&text).map_err(|malformed| ConfigError::Malformed { path, malformed })
}

/// 設定の綴りから、判定に当てる閾値を出す。
///
/// ファイルを読まないので、**設定の書式そのものはここだけで確かめられる**
/// (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」と同じ形)。
///
/// # Errors
///
/// 知らないテーブル・知らないキー・同じキーの重複・値が閾値として読めないとき。
fn thresholds_of_text(text: &str) -> Result<ConfiguredThresholds, MalformedConfig> {
    let mut written = WrittenThresholds::default();
    let mut in_thresholds_table = false;

    for (index, raw_line) in text.lines().enumerate() {
        let line = LineNumber::from_index(index);
        let content = without_comment(raw_line).trim();
        if content.is_empty() {
            continue;
        }

        if let Some(name) = table_name_of(content) {
            if name != THRESHOLDS_TABLE_NAME {
                return Err(MalformedConfig::new(
                    line,
                    MalformedReason::UnknownTable(name.to_owned()),
                ));
            }
            in_thresholds_table = true;
            continue;
        }

        let Some((key, value)) = content.split_once('=') else {
            return Err(MalformedConfig::new(
                line,
                MalformedReason::NotAKeyValue(content.to_owned()),
            ));
        };
        let key = key.trim();

        if !in_thresholds_table {
            return Err(MalformedConfig::new(
                line,
                MalformedReason::KeyOutsideTable(key.to_owned()),
            ));
        }

        written.written(key, value.trim(), line)?;
    }

    Ok(written.applied_to(ConfiguredThresholds::default()))
}

/// 行末のコメントを落とした部分。
fn without_comment(line: &str) -> &str {
    match line.split_once(COMMENT_MARK) {
        Some((content, _)) => content,
        None => line,
    }
}

/// その行がテーブルの見出しなら、その名前。
///
/// `[` で始まり `]` で終わる行だけを見出しとして読む。`[` で始まって閉じていない行は
/// 見出しではないので [`MalformedReason::NotAKeyValue`] のほうへ落ちる。
fn table_name_of(content: &str) -> Option<&str> {
    let inner = content.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim())
}

/// 設定に書かれていた閾値。書かれていなければ `None`。
///
/// **`Option` で持つのは、既定値で埋めないため。** 埋めてしまうと
/// 「既定値と同じ値が書いてあった」と「書かれていなかった」を区別できず、
/// 同じキーを 2 回書いた行を見つけられない。
#[derive(Debug, Default)]
struct WrittenThresholds {
    structural_similarity: Option<Threshold>,
    shared_imports: Option<Threshold>,
    shared_caller_domains: Option<Threshold>,
}

impl WrittenThresholds {
    /// キーと値の組を 1 つ受け取る。
    ///
    /// # Errors
    ///
    /// 知らないキー・同じキーの 2 度目・値が閾値として読めないとき。
    fn written(&mut self, key: &str, value: &str, line: LineNumber) -> Result<(), MalformedConfig> {
        let slot = match key {
            STRUCTURAL_SIMILARITY_KEY => &mut self.structural_similarity,
            SHARED_IMPORTS_KEY => &mut self.shared_imports,
            SHARED_CALLER_DOMAINS_KEY => &mut self.shared_caller_domains,
            unknown => {
                return Err(MalformedConfig::new(
                    line,
                    MalformedReason::UnknownKey(unknown.to_owned()),
                ));
            }
        };

        if slot.is_some() {
            return Err(MalformedConfig::new(
                line,
                MalformedReason::DuplicateKey(key.to_owned()),
            ));
        }

        let threshold = value
            .parse()
            .map_err(|error| MalformedConfig::new(line, MalformedReason::Threshold(error)))?;
        *slot = Some(threshold);
        Ok(())
    }

    /// 書かれていたキーだけを `base` へ重ねる。
    fn applied_to(&self, base: ConfiguredThresholds) -> ConfiguredThresholds {
        let mut thresholds = base;
        if let Some(threshold) = self.structural_similarity {
            thresholds = thresholds.with_structural_similarity(threshold);
        }
        if let Some(threshold) = self.shared_imports {
            thresholds = thresholds.with_shared_imports(threshold);
        }
        if let Some(threshold) = self.shared_caller_domains {
            thresholds = thresholds.with_shared_caller_domains(threshold);
        }
        thresholds
    }
}

/// 設定を閾値にできなかった理由。
#[derive(Debug)]
pub enum ConfigError {
    /// ファイルは在るが読めなかった。保持しているのは読もうとした場所と、その理由。
    Unreadable {
        /// 読もうとしたファイル。
        path: PathBuf,
        /// 読めなかった理由。
        error: io::Error,
    },
    /// 読めたが書式が違う。保持しているのは読んだ場所と、どの行がなぜ駄目か。
    Malformed {
        /// 読んだファイル。
        path: PathBuf,
        /// 駄目だった行と理由。
        malformed: MalformedConfig,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, error } => {
                write!(formatter, "{} を読めませんでした: {error}", path.display())
            }
            Self::Malformed { path, malformed } => {
                write!(formatter, "{} {malformed}", path.display())
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unreadable { error, .. } => Some(error),
            Self::Malformed { malformed, .. } => Some(malformed),
        }
    }
}

/// 書式が違った行と、その理由。
#[derive(Debug, PartialEq)]
pub struct MalformedConfig {
    line: LineNumber,
    reason: MalformedReason,
}

impl MalformedConfig {
    /// 行と理由を組にする。
    fn new(line: LineNumber, reason: MalformedReason) -> Self {
        Self { line, reason }
    }

    /// 駄目だった行。
    pub fn line(&self) -> LineNumber {
        self.line
    }

    /// なぜ駄目だったか。
    pub fn reason(&self) -> &MalformedReason {
        &self.reason
    }
}

impl fmt::Display for MalformedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} 行目: {}", self.line, self.reason)
    }
}

impl Error for MalformedConfig {}

/// 行が駄目だった理由。
///
/// **利用者が直す先ごとに分ける**（`rules/coding.md`「エラー型は原因ごとに
/// バリアントを分ける」）。テーブルの名前・キーの綴り・同じキーの 2 度目・
/// 値の範囲は、どれも直す場所が違う。
#[derive(Debug, PartialEq)]
pub enum MalformedReason {
    /// `[thresholds]` 以外のテーブル。保持しているのは書かれていた名前。
    UnknownTable(String),
    /// テーブルの見出しでもキーと値でもない行。保持しているのはその行。
    NotAKeyValue(String),
    /// テーブルの見出しより前に書かれたキー。保持しているのはそのキー。
    KeyOutsideTable(String),
    /// `[thresholds]` が持たないキー。保持しているのは書かれていた綴り。
    UnknownKey(String),
    /// 同じキーが 2 回。保持しているのはそのキー。
    DuplicateKey(String),
    /// 値を閾値として読めなかった。
    Threshold(ThresholdParseError),
}

impl fmt::Display for MalformedReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTable(name) => write!(
                formatter,
                "知らないテーブルです（読めるのは [{THRESHOLDS_TABLE_NAME}] だけ）: [{name}]"
            ),
            Self::NotAKeyValue(content) => {
                write!(formatter, "キーと値の組として読めません: {content}")
            }
            Self::KeyOutsideTable(key) => write!(
                formatter,
                "[{THRESHOLDS_TABLE_NAME}] より前に書かれています: {key}"
            ),
            Self::UnknownKey(key) => write!(
                formatter,
                "知らないキーです（読めるのは {STRUCTURAL_SIMILARITY_KEY} / \
                 {SHARED_IMPORTS_KEY} / {SHARED_CALLER_DOMAINS_KEY}）: {key}"
            ),
            Self::DuplicateKey(key) => write!(formatter, "同じキーが 2 回あります: {key}"),
            Self::Threshold(error) => write!(formatter, "{error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 つの既定値はどれも 0.5 なので、**既定値と違い、互いにも違う値**を使う。
    /// 同じ値を使うと、別のキーへ書き込む実装でも通る
    /// (rules/testing.md「既定値と違う答えになる入力を選ぶ」)。
    const STRUCTURAL_SIMILARITY: f64 = 0.8;
    const SHARED_IMPORTS: f64 = 0.7;
    const SHARED_CALLER_DOMAINS: f64 = 0.6;

    fn threshold(value: f64) -> Threshold {
        Threshold::new(value).expect("テストが渡す値は範囲内")
    }

    fn thresholds_of_spelling(text: &str) -> ConfiguredThresholds {
        thresholds_of_text(text).expect("テストが渡す綴りは読める")
    }

    fn reason_of_spelling(text: &str) -> MalformedReason {
        thresholds_of_text(text)
            .expect_err("テストが渡す綴りは読めない")
            .reason
    }

    fn line_of_spelling(text: &str) -> usize {
        thresholds_of_text(text)
            .expect_err("テストが渡す綴りは読めない")
            .line
            .get()
    }

    #[test]
    fn test_empty_settings_leave_every_threshold_at_its_default() {
        assert_eq!(thresholds_of_spelling(""), ConfiguredThresholds::default());
    }

    #[test]
    fn test_structural_similarity_key_moves_only_that_threshold() {
        let thresholds = thresholds_of_spelling("[thresholds]\nstructural_similarity = 0.8\n");

        assert_eq!(
            thresholds,
            ConfiguredThresholds::default()
                .with_structural_similarity(threshold(STRUCTURAL_SIMILARITY))
        );
    }

    #[test]
    fn test_shared_imports_key_moves_only_that_threshold() {
        let thresholds = thresholds_of_spelling("[thresholds]\nshared_imports = 0.7\n");

        assert_eq!(
            thresholds,
            ConfiguredThresholds::default().with_shared_imports(threshold(SHARED_IMPORTS))
        );
    }

    #[test]
    fn test_shared_caller_domains_key_moves_only_that_threshold() {
        let thresholds = thresholds_of_spelling("[thresholds]\nshared_caller_domains = 0.6\n");

        assert_eq!(
            thresholds,
            ConfiguredThresholds::default()
                .with_shared_caller_domains(threshold(SHARED_CALLER_DOMAINS))
        );
    }

    #[test]
    fn test_three_keys_together_each_reach_their_own_threshold() {
        // 3 つを同時に書いても混ざらない。値を互いに違えてあるので、
        // 2 つのキーを同じ場所へ書く実装では落ちる
        let thresholds = thresholds_of_spelling(
            "[thresholds]\n\
             structural_similarity = 0.8\n\
             shared_imports = 0.7\n\
             shared_caller_domains = 0.6\n",
        );

        assert_eq!(
            thresholds,
            ConfiguredThresholds::default()
                .with_structural_similarity(threshold(STRUCTURAL_SIMILARITY))
                .with_shared_imports(threshold(SHARED_IMPORTS))
                .with_shared_caller_domains(threshold(SHARED_CALLER_DOMAINS))
        );
    }

    #[test]
    fn test_comments_and_blank_lines_are_not_read_as_settings() {
        // 対照として、コメントに挟まれたキーが 1 つ効いていることまで見る。
        // 何も書いていない綴りでは、全部を読み飛ばす実装でも通る
        let thresholds = thresholds_of_spelling(
            "# 閾値の設定\n\
             \n\
             [thresholds]  # ここから\n\
             shared_imports = 0.7  # 半分より厳しく\n",
        );

        assert_eq!(
            thresholds,
            ConfiguredThresholds::default().with_shared_imports(threshold(SHARED_IMPORTS))
        );
    }

    #[test]
    fn test_unknown_table_is_reported_instead_of_being_skipped() {
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = \"src/billing\"\n"),
            MalformedReason::UnknownTable("domains".to_owned())
        );
    }

    #[test]
    fn test_unknown_key_is_reported_instead_of_being_skipped() {
        // 打ち間違いを黙って落とすと、設定したのに効かない形で現れる
        assert_eq!(
            reason_of_spelling("[thresholds]\nsharedimports = 0.7\n"),
            MalformedReason::UnknownKey("sharedimports".to_owned())
        );
    }

    #[test]
    fn test_key_written_before_any_table_is_reported() {
        assert_eq!(
            reason_of_spelling("shared_imports = 0.7\n"),
            MalformedReason::KeyOutsideTable("shared_imports".to_owned())
        );
    }

    #[test]
    fn test_same_key_written_twice_is_reported() {
        // 後の行で黙って上書きすると、どちらが効いたのかを綴りから読めない
        assert_eq!(
            reason_of_spelling("[thresholds]\nshared_imports = 0.7\nshared_imports = 0.6\n"),
            MalformedReason::DuplicateKey("shared_imports".to_owned())
        );
    }

    #[test]
    fn test_line_that_is_neither_a_table_nor_a_pair_is_reported() {
        assert_eq!(
            reason_of_spelling("[thresholds]\nshared_imports\n"),
            MalformedReason::NotAKeyValue("shared_imports".to_owned())
        );
    }

    #[test]
    fn test_unclosed_table_header_is_not_read_as_a_table() {
        assert_eq!(
            reason_of_spelling("[thresholds\n"),
            MalformedReason::NotAKeyValue("[thresholds".to_owned())
        );
    }

    #[test]
    fn test_value_out_of_range_keeps_the_reason_from_the_threshold() {
        assert_eq!(
            reason_of_spelling("[thresholds]\nshared_imports = 1.5\n"),
            MalformedReason::Threshold(ThresholdParseError::OutOfRange(1.5))
        );
    }

    #[test]
    fn test_value_that_is_not_a_number_keeps_the_reason_from_the_threshold() {
        assert_eq!(
            reason_of_spelling("[thresholds]\nshared_imports = high\n"),
            MalformedReason::Threshold(ThresholdParseError::NotANumber("high".to_owned()))
        );
    }

    #[test]
    fn test_the_reported_line_is_the_one_that_is_wrong() {
        // 1 行目ではない行を選ぶ。行番号を常に 1 にする実装でも 1 行目なら通る
        assert_eq!(
            line_of_spelling("[thresholds]\nshared_imports = 0.7\nsharedcallers = 0.6\n"),
            3
        );
    }

    #[test]
    fn test_settings_file_that_is_absent_leaves_every_threshold_at_its_default() {
        // 設定ファイルを置いたフィクスチャの親。ここ自身には置かない
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config");

        assert_eq!(
            thresholds_of(&directory).expect("設定ファイルが無いことは失敗ではない"),
            ConfiguredThresholds::default()
        );
    }

    #[test]
    fn test_settings_file_on_disk_moves_the_thresholds_it_writes() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/tuned");

        assert_eq!(
            thresholds_of(&directory).expect("フィクスチャの設定は読める"),
            ConfiguredThresholds::default()
                .with_structural_similarity(threshold(STRUCTURAL_SIMILARITY))
                .with_shared_imports(threshold(SHARED_IMPORTS))
                .with_shared_caller_domains(threshold(SHARED_CALLER_DOMAINS))
        );
    }

    #[test]
    fn test_settings_file_on_disk_that_is_malformed_names_the_file_and_the_line() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/unknown-key");

        let error = thresholds_of(&directory).expect_err("知らないキーは読めない");

        let text = error.to_string();
        assert!(
            text.contains(CONFIG_FILE_NAME)
                && text.contains("2 行目")
                && text.contains("similarity"),
            "どのファイルの何行目の何が駄目かが出る: {text}"
        );
    }
}
