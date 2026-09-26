//! `dryguard.toml` が書いた閾値とドメインの宣言。
//!
//! 読むのは **`[thresholds]` テーブルとそこに並ぶ 3 つのキー、`[domains]` テーブルと
//! そこに並ぶドメインの宣言だけ**。`dryguard.toml` が無ければ既定値で宣言も無く、
//! **無いことは正常**（設定を置かずに使える）。
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
//! すべて `Err`** にしてある。値として読むのは数値と、**1 行に閉じた、二重引用符の
//! 文字列の配列**（エスケープを含まない）だけ。テーブルを足すときは、
//! **そのテーブルを読む側と同じ PR で**ここへ足すことになる。

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::classification::ConfiguredThresholds;
use crate::domain_declaration::{
    DomainDeclaration, DomainDeclarations, DomainName, DomainPattern, DomainPatternError,
};
use crate::line_number::LineNumber;
use crate::threshold::{Threshold, ThresholdParseError};

/// 設定ファイルの名前。
pub const CONFIG_FILE_NAME: &str = "dryguard.toml";

/// 閾値を並べるテーブルの名前。
const THRESHOLDS_TABLE_NAME: &str = "thresholds";

/// ドメインの宣言を並べるテーブルの名前。
const DOMAINS_TABLE_NAME: &str = "domains";

/// 構造類似度の閾値のキー。
const STRUCTURAL_SIMILARITY_KEY: &str = "structural_similarity";

/// 依存先の重なりの閾値のキー。
const SHARED_IMPORTS_KEY: &str = "shared_imports";

/// 呼び出し元ドメインの重なりの閾値のキー。
const SHARED_CALLER_DOMAINS_KEY: &str = "shared_caller_domains";

/// 行末のコメントを始める綴り。
///
/// **文字列の中に現れたものはコメントではない**（`"src/#legacy/**"`）。
/// 読む文字列はエスケープを持たない（`\` は glob として `Err`）ので、
/// 二重引用符の開閉を数えるだけで文字列の内外を決められる。
const COMMENT_MARK: char = '#';

/// 文字列を囲む綴り。
const QUOTE: char = '"';

/// `dryguard.toml` から読んだもの一式。
#[derive(Debug, Clone, PartialEq)]
pub struct Configuration {
    thresholds: ConfiguredThresholds,
    domain_declarations: DomainDeclarations,
}

impl Configuration {
    /// 判定に当てる閾値。書かれていなかったキーは既定値のまま。
    pub fn thresholds(&self) -> ConfiguredThresholds {
        self.thresholds
    }

    /// `[domains]` が宣言したドメイン。テーブルが無ければ宣言は 0 個。
    pub fn domain_declarations(&self) -> &DomainDeclarations {
        &self.domain_declarations
    }
}

/// `directory` の `dryguard.toml` を読む。
///
/// 書かれていたキーだけが既定値を上書きする。ファイルが無ければ
/// [`ConfiguredThresholds::default`] と、宣言の無い [`DomainDeclarations`] を返す。
/// **宣言の glob の起点は `directory`**（絶対パスへ直して持つ）。
///
/// # Errors
///
/// ファイルが在るのに読めない・起点を絶対パスへ直せない（[`ConfigError::Unreadable`]）、
/// または書式が違う（[`ConfigError::Malformed`]）とき。
pub fn configuration_of(directory: &Path) -> Result<Configuration, ConfigError> {
    let path = directory.join(CONFIG_FILE_NAME);

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        // 無いことは正常。**読めなかったことと分けて扱う** —
        // 権限が無いファイルを「無い」と同じにすると、置いたのに効かない形になる
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Configuration {
                thresholds: ConfiguredThresholds::default(),
                domain_declarations: DomainDeclarations::default(),
            });
        }
        Err(error) => return Err(ConfigError::Unreadable { path, error }),
    };
    let root = std::path::absolute(directory).map_err(|error| ConfigError::Unreadable {
        path: path.clone(),
        error,
    })?;

    configuration_of_text(&text, &root)
        .map_err(|malformed| ConfigError::Malformed { path, malformed })
}

/// 設定の綴りから、判定に当てる閾値とドメインの宣言を出す。
///
/// `root` は宣言の glob の起点。ファイルを読まないので、**設定の書式そのものは
/// ここだけで確かめられる**
/// (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」と同じ形)。
///
/// # Errors
///
/// 知らないテーブル・知らないキー・同じキーの重複・値が読めないとき。
fn configuration_of_text(text: &str, root: &Path) -> Result<Configuration, MalformedConfig> {
    let mut written = WrittenThresholds::default();
    let mut declarations: Vec<DomainDeclaration> = Vec::new();
    let mut current_table: Option<Table> = None;

    for (index, raw_line) in text.lines().enumerate() {
        let line = LineNumber::from_index(index);
        let content = without_comment(raw_line).trim();
        if content.is_empty() {
            continue;
        }

        if let Some(name) = table_name_of(content) {
            let table = Table::of_name(name).ok_or_else(|| {
                MalformedConfig::new(line, MalformedReason::UnknownTable(name.to_owned()))
            })?;
            current_table = Some(table);
            continue;
        }

        let Some((key, value)) = content.split_once('=') else {
            return Err(MalformedConfig::new(
                line,
                MalformedReason::NotAKeyValue(content.to_owned()),
            ));
        };
        let key = key.trim();
        let value = value.trim();

        match current_table {
            None => {
                return Err(MalformedConfig::new(
                    line,
                    MalformedReason::KeyOutsideTable(key.to_owned()),
                ));
            }
            Some(Table::Thresholds) => written.written(key, value, line)?,
            Some(Table::Domains) => {
                let declaration = declaration_of(key, value, line)?;
                if declarations
                    .iter()
                    .any(|declared| declared.name() == declaration.name())
                {
                    return Err(MalformedConfig::new(
                        line,
                        MalformedReason::DuplicateKey(key.to_owned()),
                    ));
                }
                declarations.push(declaration);
            }
        }
    }

    Ok(Configuration {
        thresholds: written.applied_to(ConfiguredThresholds::default()),
        domain_declarations: DomainDeclarations::new(root, declarations),
    })
}

/// 読めるテーブル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Table {
    /// `[thresholds]`。
    Thresholds,
    /// `[domains]`。
    Domains,
}

impl Table {
    /// 見出しの名前から。読めないテーブルなら `None`。
    fn of_name(name: &str) -> Option<Self> {
        match name {
            THRESHOLDS_TABLE_NAME => Some(Self::Thresholds),
            DOMAINS_TABLE_NAME => Some(Self::Domains),
            _ => None,
        }
    }
}

/// `[domains]` のキーと値の組 1 つを、ドメインの宣言にする。
///
/// # Errors
///
/// キーが裸のキーでない・値が文字列の配列でない・配列が空・glob が読めないとき。
fn declaration_of(
    key: &str,
    value: &str,
    line: LineNumber,
) -> Result<DomainDeclaration, MalformedConfig> {
    let malformed = |reason| MalformedConfig::new(line, reason);

    let name = DomainName::new(key)
        .ok_or_else(|| malformed(MalformedReason::InvalidDomainName(key.to_owned())))?;
    let spellings = strings_of_array(value)
        .ok_or_else(|| malformed(MalformedReason::NotAStringArray(value.to_owned())))?;
    let patterns = spellings
        .into_iter()
        .map(|spelling| spelling.parse::<DomainPattern>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| malformed(MalformedReason::DomainPattern(error)))?;

    DomainDeclaration::new(name, patterns)
        .ok_or_else(|| malformed(MalformedReason::EmptyDomain(key.to_owned())))
}

/// 1 行に閉じた、二重引用符の文字列の配列（`["a", "b"]`）の中身。読めなければ `None`。
///
/// 末尾のカンマは許す（TOML がそう読む）。単引用符の文字列・複数行の配列・
/// 文字列でない要素は読まない。
fn strings_of_array(value: &str) -> Option<Vec<&str>> {
    let mut rest = value.strip_prefix('[')?.strip_suffix(']')?;
    let mut strings = Vec::new();

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Some(strings);
        }
        let (string, after) = rest.strip_prefix(QUOTE)?.split_once(QUOTE)?;
        strings.push(string);

        rest = after.trim_start();
        if rest.is_empty() {
            return Some(strings);
        }
        rest = rest.strip_prefix(',')?;
    }
}

/// 行末のコメントを落とした部分。文字列の中の `#` では切らない。
fn without_comment(line: &str) -> &str {
    let mut in_string = false;
    for (index, character) in line.char_indices() {
        match character {
            QUOTE => in_string = !in_string,
            COMMENT_MARK if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
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
    /// `[thresholds]` / `[domains]` 以外のテーブル。保持しているのは書かれていた名前。
    UnknownTable(String),
    /// テーブルの見出しでもキーと値でもない行。保持しているのはその行。
    NotAKeyValue(String),
    /// テーブルの見出しより前に書かれたキー。保持しているのはそのキー。
    KeyOutsideTable(String),
    /// `[thresholds]` が持たないキー。保持しているのは書かれていた綴り。
    UnknownKey(String),
    /// 同じキー（`[domains]` では同じドメインの名前）が 2 回。保持しているのはそのキー。
    DuplicateKey(String),
    /// 値を閾値として読めなかった。
    Threshold(ThresholdParseError),
    /// `[domains]` のキーがドメインの名前として読めない。保持しているのはそのキー。
    InvalidDomainName(String),
    /// `[domains]` の値が 1 行に閉じた文字列の配列でない。保持しているのはその値。
    NotAStringArray(String),
    /// `[domains]` の宣言が glob を 1 つも持たない。保持しているのはそのキー。
    EmptyDomain(String),
    /// glob として読めなかった。
    DomainPattern(DomainPatternError),
}

impl fmt::Display for MalformedReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTable(name) => write!(
                formatter,
                "知らないテーブルです（読めるのは [{THRESHOLDS_TABLE_NAME}] / \
                 [{DOMAINS_TABLE_NAME}]）: [{name}]"
            ),
            Self::NotAKeyValue(content) => {
                write!(formatter, "キーと値の組として読めません: {content}")
            }
            Self::KeyOutsideTable(key) => {
                write!(formatter, "テーブルの見出しより前に書かれています: {key}")
            }
            Self::UnknownKey(key) => write!(
                formatter,
                "知らないキーです（読めるのは {STRUCTURAL_SIMILARITY_KEY} / \
                 {SHARED_IMPORTS_KEY} / {SHARED_CALLER_DOMAINS_KEY}）: {key}"
            ),
            Self::DuplicateKey(key) => write!(formatter, "同じキーが 2 回あります: {key}"),
            Self::Threshold(error) => write!(formatter, "{error}"),
            Self::InvalidDomainName(key) => write!(
                formatter,
                "ドメインの名前に使えるのは英数字・_・- だけです: {key}"
            ),
            Self::NotAStringArray(value) => write!(
                formatter,
                "1 行に閉じた二重引用符の文字列の配列として読めません: {value}"
            ),
            Self::EmptyDomain(key) => write!(formatter, "glob を 1 つも持たない宣言です: {key}"),
            Self::DomainPattern(error) => write!(formatter, "{error}"),
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

    /// 宣言の glob の起点。テストの中では実在しなくてよい（照合は綴りだけを見る）。
    const ROOT: &str = "/repo";

    fn configuration_of_spelling(text: &str) -> Configuration {
        configuration_of_text(text, Path::new(ROOT)).expect("テストが渡す綴りは読める")
    }

    fn thresholds_of_spelling(text: &str) -> ConfiguredThresholds {
        configuration_of_spelling(text).thresholds()
    }

    fn thresholds_of(directory: &Path) -> Result<ConfiguredThresholds, ConfigError> {
        configuration_of(directory).map(|configuration| configuration.thresholds())
    }

    fn declared_domain_of_spelling(text: &str, path: &str) -> Option<String> {
        configuration_of_spelling(text)
            .domain_declarations()
            .declared_domain_of(Path::new(path))
            .expect("テストが渡すパスは 2 つの宣言に当たらない")
            .map(|domain| domain.as_str().to_owned())
    }

    fn reason_of_spelling(text: &str) -> MalformedReason {
        configuration_of_text(text, Path::new(ROOT))
            .expect_err("テストが渡す綴りは読めない")
            .reason
    }

    fn line_of_spelling(text: &str) -> usize {
        configuration_of_text(text, Path::new(ROOT))
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
            reason_of_spelling("[owners]\nbilling = \"team-a\"\n"),
            MalformedReason::UnknownTable("owners".to_owned())
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
    fn test_settings_file_that_is_present_but_unreadable_is_not_treated_as_absent() {
        // 対照は 1 つ上のテスト（無ければ既定値）。同じ入力では、読めない場合も
        // 既定値へ落とす実装が通ってしまう
        //
        // フィクスチャは dryguard.toml という名前のディレクトリ。chmod で権限を
        // 外す形にしないのは、**CI が root で走ると読めてしまう**ため
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/unreadable");

        let error = thresholds_of(&directory).expect_err("読めないものを既定値へ落とさない");

        assert!(
            matches!(error, ConfigError::Unreadable { .. }),
            "無いことと読めないことを分ける: {error}"
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

    #[test]
    fn test_no_domains_table_declares_no_domain() {
        let configuration = configuration_of_spelling("[thresholds]\nshared_imports = 0.7\n");

        assert!(configuration.domain_declarations().is_empty());
    }

    #[test]
    fn test_domains_table_declares_each_domain_by_its_globs() {
        let text = "[domains]\n\
                    billing = [\"src/billing/**\", \"src/**/invoice*.ts\"]\n\
                    inventory = [\"src/inventory/**\"]\n";

        assert_eq!(
            declared_domain_of_spelling(text, "/repo/src/services/invoiceService.ts"),
            Some("billing".to_owned())
        );
        assert_eq!(
            declared_domain_of_spelling(text, "/repo/src/inventory/stock.ts"),
            Some("inventory".to_owned())
        );
        assert_eq!(
            declared_domain_of_spelling(text, "/repo/src/shared/format.ts"),
            None
        );
    }

    #[test]
    fn test_domains_and_thresholds_tables_are_read_together() {
        // 対照として閾値が効いていることまで見る。[domains] の後ろの表を
        // 読み飛ばす実装では落ちる
        let configuration = configuration_of_spelling(
            "[domains]\nbilling = [\"src/billing/**\"]\n[thresholds]\nshared_imports = 0.7\n",
        );

        assert_eq!(
            configuration.thresholds(),
            ConfiguredThresholds::default().with_shared_imports(threshold(SHARED_IMPORTS))
        );
        assert!(!configuration.domain_declarations().is_empty());
    }

    #[test]
    fn test_hash_inside_a_glob_is_not_read_as_a_comment() {
        let text = "[domains]\nbilling = [\"src/#legacy/**\"]  # 旧い請求\n";

        assert_eq!(
            declared_domain_of_spelling(text, "/repo/src/#legacy/invoice.ts"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_trailing_comma_in_the_glob_array_is_read() {
        assert_eq!(
            declared_domain_of_spelling(
                "[domains]\nbilling = [\"src/billing/**\",]\n",
                "/repo/src/billing/invoice.ts"
            ),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_domain_value_that_is_not_a_string_array_is_reported() {
        // 配列でない 1 つの文字列を黙って読むと、TOML としては別の型を受け入れることになる
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = \"src/billing/**\"\n"),
            MalformedReason::NotAStringArray("\"src/billing/**\"".to_owned())
        );
    }

    #[test]
    fn test_glob_array_split_over_lines_is_reported() {
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = [\n  \"src/billing/**\",\n]\n"),
            MalformedReason::NotAStringArray("[".to_owned())
        );
    }

    #[test]
    fn test_single_quoted_glob_is_reported() {
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = ['src/billing/**']\n"),
            MalformedReason::NotAStringArray("['src/billing/**']".to_owned())
        );
    }

    #[test]
    fn test_domain_without_globs_is_reported() {
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = []\n"),
            MalformedReason::EmptyDomain("billing".to_owned())
        );
    }

    #[test]
    fn test_domain_name_that_is_not_a_bare_key_is_reported() {
        assert_eq!(
            reason_of_spelling("[domains]\n\"billing core\" = [\"src/**\"]\n"),
            MalformedReason::InvalidDomainName("\"billing core\"".to_owned())
        );
    }

    #[test]
    fn test_same_domain_declared_twice_is_reported() {
        // 後の行で黙って上書き・合算すると、どちらが効いたのかを綴りから読めない
        assert_eq!(
            reason_of_spelling(
                "[domains]\nbilling = [\"src/billing/**\"]\nbilling = [\"src/invoice/**\"]\n"
            ),
            MalformedReason::DuplicateKey("billing".to_owned())
        );
    }

    #[test]
    fn test_glob_that_cannot_be_read_keeps_the_reason_from_the_pattern() {
        assert_eq!(
            reason_of_spelling("[domains]\nbilling = [\"src/billing/?.ts\"]\n"),
            MalformedReason::DomainPattern(DomainPatternError::UnsupportedCharacter {
                pattern: "src/billing/?.ts".to_owned(),
                character: '?',
            })
        );
    }

    #[test]
    fn test_settings_file_on_disk_reads_globs_from_its_own_directory() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/declared");

        let configuration = configuration_of(&directory).expect("フィクスチャの設定は読める");
        let declarations = configuration.domain_declarations();

        assert_eq!(
            declarations
                .declared_domain_of(&directory.join("src/services/invoiceService.ts"))
                .expect("宣言は 1 つだけ")
                .map(DomainName::as_str),
            Some("billing")
        );
        // 対照: 起点がカレントディレクトリなら、この綴りが当たってしまう
        assert_eq!(
            declarations
                .declared_domain_of(
                    &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/services/invoiceService.ts")
                )
                .expect("宣言は 1 つだけ"),
            None
        );
    }
}
