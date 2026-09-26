//! `dryguard.toml` の `[domains]` が宣言したドメインと、ファイルがどの宣言に当たるか。
//!
//! 宣言はパスの glob で、`dryguard.toml` を置いたディレクトリからの相対で書く
//! （`docs/dryguard-plan.md`「Stage 3: 分類」）。**宣言がディレクトリからの推定に勝つ** —
//! 宣言に当たったファイルは、モジュール距離・呼び出し元・呼び出し先のどれでも、
//! ディレクトリの代わりに宣言の名前で数える。
//!
//! **I/O を持たない。** 読むのは `crate::config`、当てるのは `pipeline` と
//! `semantics::domain` で、ここはパスの綴りだけを見る
//! （`rules/coding.md`「I/O を持ってよい場所」）。
//!
//! **Why not（`crate::config` に置く）**: 判定の材料（`classification::signal`）と
//! `semantics` がこの型を持つので、`config` に置くと `classification` が `config` を
//! 知ることになる（`rules/architecture.md`「依存方向のルール」）。

use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

/// パスの区切り。glob も照合する相手も `/` で段を分ける。
const SEPARATOR: char = '/';

/// 1 つの段の中の、任意の綴りに当たる印。
const WILDCARD: char = '*';

/// 0 段以上の段に当たる印。段 1 つをまるごと占めるときだけ意味を持つ。
const ANY_DEPTH: &str = "**";

/// glob として読まない文字。
///
/// **一覧に無い書き方を `Err` に倒す。** `?` や `{a,b}` を黙って文字どおりに読むと、
/// 書いた利用者には**宣言したのに当たらない**として現れ、それを確かめる手立てが無い
/// （`crate::config` が知らない綴りを `Err` にしているのと同じ向き）。
const UNSUPPORTED_CHARACTERS: [char; 6] = ['?', '[', ']', '{', '}', '\\'];

/// ドメインの名前。`[domains]` のキーに書かれた綴り。
///
/// **綴れる文字を TOML の裸のキーに閉じる**（英数字・`_`・`-`）。引用符で囲んだキーを
/// 読まない代わりに、読める綴りの外は作れないようにしておく
/// (`rules/coding.md`「生成時に検証し、不正な値を存在させない」)。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomainName(String);

impl DomainName {
    /// 綴りを確かめてドメインの名前にする。
    ///
    /// 空、または英数字・`_`・`-` 以外を含むときは作れないので `None` を返す。
    pub fn new(spelling: &str) -> Option<Self> {
        let is_bare_key = !spelling.is_empty()
            && spelling
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character));
        if !is_bare_key {
            return None;
        }
        Some(Self(spelling.to_owned()))
    }

    /// 名前の綴り。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// ドメインに属するファイルを指す glob 1 つ。
///
/// 書けるのは `**`（0 段以上）と `*`（1 つの段の中の任意の綴り）だけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainPattern {
    spelling: String,
    segments: Vec<PatternSegment>,
}

/// glob の 1 段。
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternSegment {
    /// `**`。0 段以上に当たる。
    AnyDepth,
    /// それ以外。`*` で区切った綴りの並びで、`*` の間は任意の綴りに当たる。
    ///
    /// **区切った形で持つ**のは、照合のたびに `*` を探し直さないため。
    /// `invoice*.ts` なら `["invoice", ".ts"]`。
    Name(Vec<String>),
}

impl DomainPattern {
    /// 書かれたままの綴り。
    pub fn as_str(&self) -> &str {
        &self.spelling
    }

    /// 根からの相対の段の並びに当たるか。
    fn matches(&self, components: &[&str]) -> bool {
        segments_match(&self.segments, components)
    }
}

impl FromStr for DomainPattern {
    type Err = DomainPatternError;

    fn from_str(spelling: &str) -> Result<Self, Self::Err> {
        if spelling.is_empty() {
            return Err(DomainPatternError::Empty);
        }
        if spelling.starts_with(SEPARATOR) {
            return Err(DomainPatternError::Absolute(spelling.to_owned()));
        }
        if let Some(character) = spelling
            .chars()
            .find(|character| UNSUPPORTED_CHARACTERS.contains(character))
        {
            return Err(DomainPatternError::UnsupportedCharacter {
                pattern: spelling.to_owned(),
                character,
            });
        }

        let segments = spelling
            .split(SEPARATOR)
            .map(|segment| segment_of(spelling, segment))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            spelling: spelling.to_owned(),
            segments,
        })
    }
}

/// glob の 1 段を読む。
///
/// # Errors
///
/// 空の段（`a//b` / 末尾の `/`）・`.` / `..`・`**` を段の一部に書いたとき。
fn segment_of(pattern: &str, segment: &str) -> Result<PatternSegment, DomainPatternError> {
    if segment.is_empty() {
        return Err(DomainPatternError::EmptySegment(pattern.to_owned()));
    }
    // 起点の外を指す書き方を読まない。照合する側は起点の外のファイルを
    // どの宣言にも当てないので、書けても決して当たらない
    if segment == "." || segment == ".." {
        return Err(DomainPatternError::RelativeSegment(pattern.to_owned()));
    }
    if segment == ANY_DEPTH {
        return Ok(PatternSegment::AnyDepth);
    }
    // `a**b` を `a*b` と同じに読むか、段を越えるかは glob の実装ごとに違う。
    // どちらかを黙って選ばない
    if segment.contains(ANY_DEPTH) {
        return Err(DomainPatternError::PartialAnyDepth(pattern.to_owned()));
    }

    Ok(PatternSegment::Name(
        segment.split(WILDCARD).map(str::to_owned).collect(),
    ))
}

/// glob の段の並びが、パスの段の並びに当たるか。
fn segments_match(segments: &[PatternSegment], components: &[&str]) -> bool {
    let Some((segment, rest_segments)) = segments.split_first() else {
        return components.is_empty();
    };

    match segment {
        PatternSegment::AnyDepth => (0..=components.len())
            .any(|skipped| segments_match(rest_segments, &components[skipped..])),
        PatternSegment::Name(pieces) => {
            let Some((component, rest_components)) = components.split_first() else {
                return false;
            };
            name_matches(pieces, component) && segments_match(rest_segments, rest_components)
        }
    }
}

/// `*` で区切った綴りの並びが、段の名前 1 つに当たるか。
///
/// 先頭の綴りは名前の頭に、末尾の綴りは名前の尻に、間の綴りはその間に順に現れればよい。
fn name_matches(pieces: &[String], name: &str) -> bool {
    let Some((first, rest)) = pieces.split_first() else {
        return name.is_empty();
    };
    let Some(after_first) = name.strip_prefix(first.as_str()) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        // `*` を含まない段。名前がそのまま一致しなければならない
        return after_first.is_empty();
    };

    let mut remaining = after_first;
    for piece in middle {
        let Some(found) = remaining.find(piece.as_str()) else {
            return false;
        };
        remaining = &remaining[found + piece.len()..];
    }
    remaining.ends_with(last.as_str())
}

/// glob を読めなかった理由。
///
/// **直す先ごとに分ける**（`rules/coding.md`「エラー型は原因ごとにバリアントを分ける」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainPatternError {
    /// 空の綴り。
    Empty,
    /// `/` で始まる。起点（`dryguard.toml` を置いたディレクトリ）からの相対で書く。
    Absolute(String),
    /// 空の段がある（`a//b` / 末尾の `/`）。
    EmptySegment(String),
    /// `.` か `..` の段がある。
    RelativeSegment(String),
    /// `**` を段の一部に書いた（`a**b`）。
    PartialAnyDepth(String),
    /// `**` と `*` のほかに、glob として読まない文字を書いた。
    UnsupportedCharacter {
        /// 書かれていた glob。
        pattern: String,
        /// 読まなかった文字。
        character: char,
    },
}

impl fmt::Display for DomainPatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "空の glob は書けません"),
            Self::Absolute(pattern) => write!(
                formatter,
                "glob は dryguard.toml を置いたディレクトリからの相対で書きます: {pattern}"
            ),
            Self::EmptySegment(pattern) => write!(formatter, "glob に空の段があります: {pattern}"),
            Self::RelativeSegment(pattern) => {
                write!(formatter, "glob に . / .. の段は書けません: {pattern}")
            }
            Self::PartialAnyDepth(pattern) => write!(
                formatter,
                "** は段をまるごと占めるときだけ書けます: {pattern}"
            ),
            Self::UnsupportedCharacter { pattern, character } => write!(
                formatter,
                "glob として読めない文字です（書けるのは ** と *）: {character} ({pattern})"
            ),
        }
    }
}

impl Error for DomainPatternError {}

/// ドメインの宣言 1 つ。名前と、そのドメインに属するファイルを指す glob。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainDeclaration {
    name: DomainName,
    patterns: Vec<DomainPattern>,
}

impl DomainDeclaration {
    /// 名前と glob を組にする。
    ///
    /// glob が 1 つも無ければ作れないので `None` を返す。**何も指さない宣言を
    /// 作らせない** — 書いた利用者には宣言したのに効かないとして現れる。
    pub fn new(name: DomainName, patterns: Vec<DomainPattern>) -> Option<Self> {
        if patterns.is_empty() {
            return None;
        }
        Some(Self { name, patterns })
    }

    /// 宣言したドメインの名前。
    pub fn name(&self) -> &DomainName {
        &self.name
    }
}

/// `[domains]` が宣言したドメインの一覧と、glob の起点。
///
/// 宣言が 1 つも無いときも作れる（[`Self::default`]）。そのときはどのファイルも
/// 宣言に当たらず、**ディレクトリからの推定だけで今までと同じ判定になる**。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainDeclarations {
    root: PathBuf,
    declarations: Vec<DomainDeclaration>,
}

impl DomainDeclarations {
    /// 起点と宣言の一覧を組にする。
    ///
    /// `root` は glob の起点（`dryguard.toml` を置いたディレクトリ）の絶対パス。
    /// 綴りの上で `.` / `..` を畳んでから持つ。
    pub fn new(root: &Path, declarations: Vec<DomainDeclaration>) -> Self {
        Self {
            root: lexically_normalized(root).unwrap_or_else(|| root.to_path_buf()),
            declarations,
        }
    }

    /// 宣言が 1 つも無いか。
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }

    /// そのファイルが当たる宣言の名前。どの宣言にも当たらなければ `None`。
    ///
    /// `path` は絶対パスか、**起点からの相対パス**。`dryguard` は設定をカレントディレクトリ
    /// から読み（`main.rs` の `CONFIG_DIRECTORY`）、引数のパスもカレントディレクトリからの
    /// 相対なので、相対パスは起点からの相対として読めば揃う。
    ///
    /// 起点の外のファイル・段の名前が UTF-8 でないファイルは、どの宣言にも当たらない。
    ///
    /// # Errors
    ///
    /// 名前の違う 2 つの宣言に当たったとき。**黙ってどちらかを選ばない**
    /// （書いた順で決めると、並べ替えただけで判定が変わる）。
    pub fn declared_domain_of(&self, path: &Path) -> Result<Option<&DomainName>, AmbiguousDomain> {
        if self.declarations.is_empty() {
            return Ok(None);
        }
        let Some(components) = self.components_from_root_of(path) else {
            return Ok(None);
        };
        let components: Vec<&str> = components.iter().map(String::as_str).collect();

        let mut matched = self.declarations.iter().filter(|declaration| {
            declaration
                .patterns
                .iter()
                .any(|pattern| pattern.matches(&components))
        });
        let Some(first) = matched.next() else {
            return Ok(None);
        };
        if let Some(second) = matched.find(|declaration| declaration.name != first.name) {
            return Err(AmbiguousDomain {
                path: path.to_path_buf(),
                domains: [first.name.clone(), second.name.clone()],
            });
        }
        Ok(Some(&first.name))
    }

    /// 起点からの相対の段の並び。起点の外・UTF-8 でない段を含むときは `None`。
    fn components_from_root_of(&self, path: &Path) -> Option<Vec<String>> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let normalized = lexically_normalized(&absolute)?;
        let relative = normalized.strip_prefix(&self.root).ok()?;

        relative
            .components()
            .map(|component| component.as_os_str().to_str().map(str::to_owned))
            .collect()
    }
}

/// `.` を落とし、`..` で 1 段戻した綴り。ファイルシステムを見ない。
///
/// 根より上へ戻ろうとしたときは `None`。
///
/// **Why not（`fs::canonicalize`）**: I/O を持つことになり、しかも**存在しないファイルを
/// 照合できなくなる**（参照元のパスはサーバが返した綴りで、ここで確かめる理由が無い）。
fn lexically_normalized(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let has_parent = normalized.parent().is_some() && normalized.pop();
                if !has_parent {
                    return None;
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Some(normalized)
}

/// 1 つのファイルが、名前の違う 2 つの宣言に当たった。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousDomain {
    path: PathBuf,
    domains: [DomainName; 2],
}

impl AmbiguousDomain {
    /// 2 つの宣言に当たったファイル。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 当たった宣言のうち、書かれた順で先の 2 つ。
    pub fn domains(&self) -> &[DomainName; 2] {
        &self.domains
    }
}

impl fmt::Display for AmbiguousDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [first, second] = &self.domains;
        write!(
            formatter,
            "{} が dryguard.toml の 2 つのドメインの宣言に当たります: {first} / {second}",
            self.path.display()
        )
    }
}

impl Error for AmbiguousDomain {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::declarations_of;

    fn name(spelling: &str) -> DomainName {
        DomainName::new(spelling).expect("テストが渡す名前は裸のキー")
    }

    fn declarations(declared: &[(&str, &[&str])]) -> DomainDeclarations {
        declarations_of(declared)
    }

    fn declared(declarations: &DomainDeclarations, path: &str) -> Option<String> {
        declarations
            .declared_domain_of(Path::new(path))
            .expect("テストが渡すパスは 2 つの宣言に当たらない")
            .map(|domain| domain.as_str().to_owned())
    }

    fn billing_by(patterns: &[&str], path: &str) -> Option<String> {
        declared(&declarations(&[("billing", patterns)]), path)
    }

    #[test]
    fn test_domain_name_accepts_a_bare_key() {
        assert_eq!(name("billing-core_2").as_str(), "billing-core_2");
    }

    #[test]
    fn test_domain_name_rejects_a_spelling_that_is_not_a_bare_key() {
        // 対照は上のテスト。`.` を含むと TOML ではドットつきのキーになる
        assert_eq!(DomainName::new("billing.core"), None);
        assert_eq!(DomainName::new(""), None);
    }

    #[test]
    fn test_any_depth_matches_files_at_every_depth_below_the_directory() {
        assert_eq!(
            billing_by(&["src/billing/**"], "/repo/src/billing/invoice.ts"),
            Some("billing".to_owned())
        );
        assert_eq!(
            billing_by(&["src/billing/**"], "/repo/src/billing/tax/rate.ts"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_any_depth_does_not_match_a_sibling_directory() {
        assert_eq!(
            billing_by(&["src/billing/**"], "/repo/src/inventory/stock.ts"),
            None
        );
    }

    #[test]
    fn test_any_depth_in_the_middle_matches_zero_directories() {
        // `src/**/invoice*.ts` は `src/invoice.ts` にも当たる
        assert_eq!(
            billing_by(&["src/**/invoice*.ts"], "/repo/src/invoice.ts"),
            Some("billing".to_owned())
        );
        assert_eq!(
            billing_by(
                &["src/**/invoice*.ts"],
                "/repo/src/services/invoiceService.ts"
            ),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_wildcard_does_not_cross_a_directory() {
        // `*` は 1 つの段の中だけ。越えるなら `**` を書く
        assert_eq!(
            billing_by(&["src/*.ts"], "/repo/src/billing/invoice.ts"),
            None
        );
        assert_eq!(
            billing_by(&["src/*.ts"], "/repo/src/invoice.ts"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_wildcard_keeps_the_text_on_both_sides() {
        assert_eq!(
            billing_by(
                &["src/**/invoice*.ts"],
                "/repo/src/services/productService.ts"
            ),
            None
        );
        assert_eq!(
            billing_by(&["src/**/invoice*.ts"], "/repo/src/models/invoice.tsx"),
            None,
            "末尾の綴りは名前の尻に当たる"
        );
    }

    #[test]
    fn test_wildcards_between_texts_match_them_in_order() {
        assert_eq!(
            billing_by(&["src/*voice*Service*"], "/repo/src/invoiceService.ts"),
            Some("billing".to_owned())
        );
        assert_eq!(
            billing_by(&["src/*Service*voice*"], "/repo/src/invoiceService.ts"),
            None
        );
    }

    #[test]
    fn test_wildcard_does_not_let_prefix_and_suffix_overlap() {
        // `ab*ba` に `aba` は当たらない（頭の ab と尻の ba が a を取り合う）
        assert_eq!(billing_by(&["ab*ba"], "/repo/aba"), None);
        assert_eq!(
            billing_by(&["ab*ba"], "/repo/abba"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_relative_path_is_read_from_the_root() {
        assert_eq!(
            billing_by(&["src/billing/**"], "src/billing/invoice.ts"),
            Some("billing".to_owned())
        );
        assert_eq!(
            billing_by(&["src/billing/**"], "./src/inventory/../billing/invoice.ts"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_file_outside_the_root_matches_no_declaration() {
        // 起点の外から相対で書いた綴りに当ててしまうと、別のリポジトリの
        // 同じ名前のディレクトリが宣言に入る
        assert_eq!(
            billing_by(&["**"], "/elsewhere/src/billing/invoice.ts"),
            None
        );
        assert_eq!(billing_by(&["**"], "../elsewhere/invoice.ts"), None);
        assert_eq!(
            billing_by(&["**"], "/repo/src/invoice.ts"),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_file_matching_no_declaration_has_no_declared_domain() {
        let declarations = declarations(&[
            ("billing", &["src/billing/**"]),
            ("inventory", &["src/inventory/**"]),
        ]);

        assert_eq!(declared(&declarations, "/repo/src/shared/format.ts"), None);
        assert_eq!(
            declared(&declarations, "/repo/src/inventory/stock.ts"),
            Some("inventory".to_owned())
        );
    }

    #[test]
    fn test_no_declarations_match_no_file() {
        let declarations = DomainDeclarations::default();

        assert!(declarations.is_empty());
        assert_eq!(declared(&declarations, "src/billing/invoice.ts"), None);
    }

    #[test]
    fn test_file_matching_two_declarations_is_an_error_naming_both() {
        let declarations = declarations(&[
            ("billing", &["src/billing/**"]),
            ("reporting", &["src/**/report*.ts"]),
        ]);

        let error = declarations
            .declared_domain_of(Path::new("/repo/src/billing/report.ts"))
            .expect_err("2 つの宣言に当たるファイルを黙ってどちらかへ寄せない");

        assert_eq!(error.domains(), &[name("billing"), name("reporting")]);
        assert_eq!(error.path(), Path::new("/repo/src/billing/report.ts"));
    }

    #[test]
    fn test_file_matching_two_patterns_of_one_declaration_is_not_an_error() {
        // 対照は上のテスト。名前が同じなら食い違いは無い
        assert_eq!(
            billing_by(
                &["src/billing/**", "src/**/invoice*.ts"],
                "/repo/src/billing/invoice.ts"
            ),
            Some("billing".to_owned())
        );
    }

    #[test]
    fn test_declaration_without_patterns_cannot_be_built() {
        assert_eq!(DomainDeclaration::new(name("billing"), Vec::new()), None);
    }

    #[test]
    fn test_pattern_that_is_empty_is_an_error() {
        assert_eq!("".parse::<DomainPattern>(), Err(DomainPatternError::Empty));
    }

    #[test]
    fn test_pattern_starting_from_the_filesystem_root_is_an_error() {
        assert_eq!(
            "/src/**".parse::<DomainPattern>(),
            Err(DomainPatternError::Absolute("/src/**".to_owned()))
        );
    }

    #[test]
    fn test_pattern_with_an_empty_segment_is_an_error() {
        assert_eq!(
            "src/billing/".parse::<DomainPattern>(),
            Err(DomainPatternError::EmptySegment("src/billing/".to_owned()))
        );
    }

    #[test]
    fn test_pattern_with_a_relative_segment_is_an_error() {
        assert_eq!(
            "../src/**".parse::<DomainPattern>(),
            Err(DomainPatternError::RelativeSegment("../src/**".to_owned()))
        );
    }

    #[test]
    fn test_pattern_with_any_depth_inside_a_segment_is_an_error() {
        assert_eq!(
            "src/a**b".parse::<DomainPattern>(),
            Err(DomainPatternError::PartialAnyDepth("src/a**b".to_owned()))
        );
    }

    #[test]
    fn test_pattern_with_a_character_this_glob_does_not_read_is_an_error() {
        // 黙って文字どおりに読むと、宣言したのに当たらない形で現れる
        assert_eq!(
            "src/{billing,invoice}/**".parse::<DomainPattern>(),
            Err(DomainPatternError::UnsupportedCharacter {
                pattern: "src/{billing,invoice}/**".to_owned(),
                character: '{',
            })
        );
    }
}
