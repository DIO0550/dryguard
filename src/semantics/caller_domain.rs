//! 参照元がどのドメインに属しているかと、2 つのチャンクの間でのその重なり。
//!
//! **数える部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! サーバに尋ねるのは [`caller_domains_outcome_of`] だけで、そこは
//! `tests/semantics.rs` が実サーバで見る。判定に使うのは `classification`。

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::codebase::source_of;
use crate::syntax::rust_test_scope::RustTestScopes;
use crate::syntax::tree::Grammar;

use crate::domain_declaration::{AmbiguousDomain, DomainDeclarations};
use crate::lsp::{ClientError, Reference, ReferencesOutcome, Session, SourceDocument};
use crate::semantics::domain::{Domain, DomainCounts};
use crate::similarity::Similarity;
use crate::source_position::SourcePosition;

/// Rust の参照位置をテスト範囲と照合できなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceSourceError {
    /// ファイルを UTF-8 ソースとして読めなかった。
    Unreadable { path: PathBuf },
    /// Rust 構文木を作れない、または構文エラーが残った。
    Unparsable { path: PathBuf },
    /// 応答の行・UTF-16 列がソースの文字を指していない。
    InvalidPosition {
        path: PathBuf,
        position: SourcePosition,
    },
}

impl ReferenceSourceError {
    /// 照合できなかった参照元ファイル。
    pub fn path(&self) -> &Path {
        match self {
            Self::Unreadable { path }
            | Self::Unparsable { path }
            | Self::InvalidPosition { path, .. } => path,
        }
    }
}

impl fmt::Display for ReferenceSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path } => {
                write!(f, "参照元のファイルを読めない: {}", path.display())
            }
            Self::Unparsable { path } => {
                write!(f, "参照元の Rust 構文を読めない: {}", path.display())
            }
            Self::InvalidPosition { path, position } => write!(
                f,
                "参照元の位置がソースの文字を指していない: {}:{}:{}",
                path.display(),
                position.line().get(),
                position.character()
            ),
        }
    }
}

impl std::error::Error for ReferenceSourceError {}

/// compare / scan の間だけ共有する、Rust 参照元ファイルごとのテスト範囲。
///
/// 同じファイルから別のチャンクへの参照が返っても、読み込みと構文解析は繰り返さない。
#[derive(Debug, Default)]
pub struct ReferenceSources {
    rust: HashMap<PathBuf, Result<RustTestScopes, ReferenceSourceError>>,
}

impl ReferenceSources {
    /// テスト内の Rust 参照を除いたパス。重複は参照の件数として残す。
    ///
    /// # Errors
    /// Rust の参照元を読めない・解析できない・位置を照合できないとき。
    fn production_paths_of(
        &mut self,
        references: &[Reference],
    ) -> Result<Vec<PathBuf>, ReferenceSourceError> {
        let mut paths = Vec::new();
        for reference in references {
            if !self.is_test(reference)? {
                paths.push(reference.path().to_path_buf());
            }
        }
        Ok(paths)
    }

    /// Rust 以外は従来どおり数える。Rust は確認できたテスト参照だけを除く。
    ///
    /// # Errors
    /// 参照元ソースまたは参照位置を読み取れないとき。
    fn is_test(&mut self, reference: &Reference) -> Result<bool, ReferenceSourceError> {
        let path = reference.path();
        if Grammar::of_path(path) != Some(Grammar::Rust) {
            return Ok(false);
        }
        let scopes = self.rust.entry(path.to_path_buf()).or_insert_with(|| {
            let source = source_of(path).map_err(|_| ReferenceSourceError::Unreadable {
                path: path.to_path_buf(),
            })?;
            RustTestScopes::from_source(source).ok_or_else(|| ReferenceSourceError::Unparsable {
                path: path.to_path_buf(),
            })
        });
        let scopes = scopes.as_ref().map_err(Clone::clone)?;
        scopes
            .contains(reference.position())
            .ok_or_else(|| ReferenceSourceError::InvalidPosition {
                path: path.to_path_buf(),
                position: reference.position(),
            })
    }
}

/// サーバに参照元を尋ねて、ドメインごとに数えた結果。
///
/// **「取れなかった」を 1 つにまとめない。** `lsp::ReferencesOutcome` が理由を分けて
/// 持っているのを、そのまま運ぶ（`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerDomainsOutcome {
    /// 参照元をドメインごとに数えられた。
    Counted(CallerDomains),
    /// 参照元が返らない、または Rust のテスト内の参照しかなかった。
    NoReferences,
    /// 参照元は返ったが、パスとして読めない URI が混じっていた。
    UnreadableReferences,
    /// Rust の参照元がテスト内か確認できなかった。部分集計は返さない。
    UnclassifiedReference(ReferenceSourceError),
    /// サーバが作業中で、落ち着いた答えを受け取れなかった。
    ServerStillWorking,
    /// サーバが references を提供していない。
    ReferencesNotProvided,
    /// 参照元のファイルが、`dryguard.toml` の名前の違う 2 つの宣言に当たった。
    ///
    /// **そのファイルを落として数えない。** 落とすと、残った参照元だけで重なりを出し、
    /// 材料が欠けたことが判定から見えなくなる。**`Err` にもしない** — 参照元は
    /// サーバが答えるまで分からず、`scan` の途中で落とすと出せていた候補ペアまで捨てる。
    AmbiguousDomain(AmbiguousDomain),
}

/// その位置にある名前の参照元を尋ねて、ドメインごとに数える。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`position` は
/// `Chunk::name_position` が指す識別子の位置。`declarations` は参照元のファイルを
/// どのドメインに数えるかを決める `dryguard.toml` の宣言。
/// `sources` は同じ compare / scan 内で共有する参照元ソースの解析結果。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
/// **参照元が無い / 読めないは `Err` にしない**（会話は成立しているので、
/// シグナルが取れなかっただけ）。
pub fn caller_domains_outcome_of(
    session: &mut Session,
    document: &SourceDocument,
    position: SourcePosition,
    declarations: &DomainDeclarations,
    sources: &mut ReferenceSources,
) -> Result<CallerDomainsOutcome, ClientError> {
    Ok(outcome_of(
        session.references(document, position)?,
        declarations,
        sources,
    ))
}

/// references の答えを、ドメインごとに数えた結果へ読み替える。
///
/// **サーバとの往復から切り離してある。** ここを [`caller_domains_outcome_of`] の中に
/// 置くと、読み替えの枝が実サーバのテストからしか通らなくなる
/// (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
fn outcome_of(
    references: ReferencesOutcome,
    declarations: &DomainDeclarations,
    sources: &mut ReferenceSources,
) -> CallerDomainsOutcome {
    match references {
        ReferencesOutcome::Answered(references) => {
            let paths = match sources.production_paths_of(&references) {
                Ok(paths) => paths,
                Err(cause) => return CallerDomainsOutcome::UnclassifiedReference(cause),
            };
            match CallerDomains::from_reference_paths(&paths, declarations) {
                Ok(Some(caller_domains)) => CallerDomainsOutcome::Counted(caller_domains),
                Ok(None) => CallerDomainsOutcome::NoReferences,
                Err(ambiguous) => CallerDomainsOutcome::AmbiguousDomain(ambiguous),
            }
        }
        ReferencesOutcome::NoAnswer => CallerDomainsOutcome::NoReferences,
        ReferencesOutcome::Unreadable { .. } => CallerDomainsOutcome::UnreadableReferences,
        ReferencesOutcome::ServerStillWorking => CallerDomainsOutcome::ServerStillWorking,
        ReferencesOutcome::NotSupported => CallerDomainsOutcome::ReferencesNotProvided,
    }
}

/// 呼び出し元が属するドメインと、そこから来ている参照元の数。
///
/// **[`CalleeDomains`](super::callee_domain::CalleeDomains) と同じ型にしない。**
/// 向きが逆なので、1 つの型で持つと**呼び出し元集合と呼び出し先集合を突き合わせる
/// 呼び出しが書けてしまう**（`rules/naming.md`「`callee` と `reference` を混ぜない」、
/// `rules/coding.md`「不正な状態を型で表現できなくする」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerDomains(DomainCounts);

impl CallerDomains {
    /// 参照元のファイルから、ドメインごとの件数にまとめる。
    ///
    /// `reference_paths` はテスト範囲の除外を終えた参照元、
    /// `declarations` は `dryguard.toml` のドメインの宣言。
    /// 1 件も無ければ作れないので `Ok(None)` を返す。
    ///
    /// # Errors
    ///
    /// どれかの参照元が、名前の違う 2 つの宣言に当たったとき。
    pub fn from_reference_paths(
        reference_paths: &[PathBuf],
        declarations: &DomainDeclarations,
    ) -> Result<Option<Self>, AmbiguousDomain> {
        Ok(DomainCounts::from_paths(reference_paths, declarations)?.map(Self))
    }

    /// 2 つの呼び出し元集合の Jaccard 係数。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        self.0.jaccard(&other.0)
    }

    /// ドメインごとの参照元の数。ドメインの綴り順に並ぶ。
    pub fn references_per_domain(&self) -> Vec<(&Domain, usize)> {
        self.0.per_domain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::lsp::UriPathError;
    use crate::test_support::declarations_of;

    /// フィクスチャの参照。綴りを確かめ、行番号が古くなったテストを落とす。
    fn fixture_reference(file: &str, number: usize, prefix: &str) -> Reference {
        let path = crate::test_support::repository_path(&format!(
            "tests/fixtures/rust-test-references/src/{file}"
        ));
        let source = source_of(&path).expect("フィクスチャを読める");
        assert!(
            source
                .lines()
                .nth(number - 1)
                .expect("参照行がある")
                .starts_with(prefix)
        );
        Reference::new(
            path,
            SourcePosition::from_preceding_text(crate::test_support::line(number), prefix),
        )
    }

    #[test]
    fn test_caller_domains_outcome_with_only_rust_tests_has_no_references() {
        let outcome = undeclared_outcome_of(ReferencesOutcome::Answered(vec![
            fixture_reference("lib.rs", 17, "    assert_eq!("),
            fixture_reference("report.rs", 4, "    crate::"),
        ]));
        assert_eq!(outcome, CallerDomainsOutcome::NoReferences);
    }

    #[test]
    fn test_caller_domains_outcome_ignores_ambiguous_domains_of_test_only_references() {
        let production = fixture_reference("lib.rs", 12, "    ");
        let report = fixture_reference("report.rs", 4, "    crate::");
        let root = production.path().parent().expect("親がある");
        let declarations = crate::test_support::declarations_at(
            root,
            &[
                ("production", &["lib.rs"]),
                ("test-a", &["report.rs"]),
                ("test-b", &["report.rs"]),
            ],
        );
        let expected =
            CallerDomains::from_reference_paths(&[production.path().to_path_buf()], &declarations)
                .expect("本番の宣言は一意")
                .expect("本番参照がある");
        let outcome = outcome_of(
            ReferencesOutcome::Answered(vec![production, report]),
            &declarations,
            &mut ReferenceSources::default(),
        );
        assert_eq!(outcome, CallerDomainsOutcome::Counted(expected));
    }

    #[test]
    fn test_caller_domains_outcome_does_not_return_partial_counts_for_unreadable_rust_sources() {
        let missing = crate::test_support::repository_path("tests/fixtures/no-such-reference.rs");
        let outcome = undeclared_outcome_of(ReferencesOutcome::Answered(vec![
            fixture_reference("lib.rs", 12, "    "),
            Reference::new(
                missing.clone(),
                SourcePosition::from_preceding_text(crate::test_support::line(1), ""),
            ),
        ]));
        assert_eq!(
            outcome,
            CallerDomainsOutcome::UnclassifiedReference(ReferenceSourceError::Unreadable {
                path: missing
            })
        );
    }

    #[test]
    fn test_caller_domains_outcome_does_not_return_partial_counts_for_malformed_rust_sources() {
        let broken =
            crate::test_support::repository_path("tests/fixtures/rust-reference-invalid.rs");
        let outcome = undeclared_outcome_of(ReferencesOutcome::Answered(vec![
            fixture_reference("lib.rs", 12, "    "),
            Reference::new(
                broken.clone(),
                SourcePosition::from_preceding_text(crate::test_support::line(2), "fn "),
            ),
        ]));
        assert_eq!(
            outcome,
            CallerDomainsOutcome::UnclassifiedReference(ReferenceSourceError::Unparsable {
                path: broken
            })
        );
    }

    #[test]
    fn test_caller_domains_outcome_does_not_return_partial_counts_for_invalid_rust_positions() {
        let valid = fixture_reference("lib.rs", 12, "    ");
        let position = SourcePosition::from_preceding_text(crate::test_support::line(999), "");
        let path = valid.path().to_path_buf();
        let outcome = undeclared_outcome_of(ReferencesOutcome::Answered(vec![
            valid,
            Reference::new(path.clone(), position),
        ]));
        assert_eq!(
            outcome,
            CallerDomainsOutcome::UnclassifiedReference(ReferenceSourceError::InvalidPosition {
                path,
                position
            })
        );
    }

    #[test]
    fn test_caller_domains_outcome_excludes_rust_tests_before_counting_domains() {
        let path =
            crate::test_support::repository_path("tests/fixtures/rust-test-references/src/lib.rs");
        let report = path.with_file_name("report.rs");
        let reference = |path: PathBuf, line, prefix| {
            crate::lsp::Reference::new(
                path,
                SourcePosition::from_preceding_text(crate::test_support::line(line), prefix),
            )
        };
        let outcome = undeclared_outcome_of(ReferencesOutcome::Answered(vec![
            reference(path.clone(), 12, "    "),
            reference(path.clone(), 12, "    first(1) + "),
            reference(path.clone(), 17, "    assert_eq!("),
            reference(report, 4, "    crate::"),
        ]));
        let expected = CallerDomains::from_reference_paths(
            &[path.clone(), path],
            &DomainDeclarations::default(),
        )
        .expect("宣言がない")
        .expect("本番参照がある");
        assert_eq!(outcome, CallerDomainsOutcome::Counted(expected));
    }

    fn undeclared_outcome_of(references: ReferencesOutcome) -> CallerDomainsOutcome {
        outcome_of(
            references,
            &DomainDeclarations::default(),
            &mut ReferenceSources::default(),
        )
    }

    fn directory_of(path: &str) -> Domain {
        Domain::of_path(Path::new(path), &DomainDeclarations::default())
            .expect("宣言が無ければ食い違わない")
    }

    fn undeclared_callers(reference_path: &str) -> CallerDomains {
        CallerDomains::from_reference_paths(
            &[PathBuf::from(reference_path)],
            &DomainDeclarations::default(),
        )
        .expect("宣言が無ければ食い違わない")
        .expect("テストが渡す参照元は 1 件以上")
    }

    fn answered(reference_paths: &[&str]) -> ReferencesOutcome {
        ReferencesOutcome::Answered(
            reference_paths
                .iter()
                .map(|path| {
                    crate::lsp::Reference::new(
                        PathBuf::from(path),
                        SourcePosition::from_preceding_text(crate::test_support::line(1), ""),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn test_caller_domains_outcome_of_answers_counts_them_per_domain() {
        let outcome = undeclared_outcome_of(answered(&[
            "/repo/src/billing/invoice.ts",
            "/repo/src/inventory/stock.ts",
            "/repo/src/inventory/restock.ts",
        ]));

        let CallerDomainsOutcome::Counted(callers) = outcome else {
            panic!("参照元を数えられる: {outcome:?}");
        };
        assert_eq!(
            callers.references_per_domain(),
            vec![
                (&directory_of("/repo/src/billing/invoice.ts"), 1),
                (&directory_of("/repo/src/inventory/stock.ts"), 2),
            ]
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_no_answer_is_not_a_counted_empty_set() {
        // 対照は上のテスト。`Counted` にすると、後段は 0 ドメインを
        // 「別ドメインに散っていない」と読む
        assert_eq!(
            undeclared_outcome_of(ReferencesOutcome::NoAnswer),
            CallerDomainsOutcome::NoReferences
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_an_empty_answer_is_not_a_counted_empty_set() {
        // `lsp` は空配列を `NoAnswer` に倒すが、ここでも受けておかないと
        // 0 件の `Answered` が「数えられた」側へ落ちる
        assert_eq!(
            undeclared_outcome_of(answered(&[])),
            CallerDomainsOutcome::NoReferences
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_an_unreadable_uri_is_not_no_references() {
        // 「読めなかった」を「1 件も無い」に畳むと、利用者が直す先が消える
        let outcome = undeclared_outcome_of(ReferencesOutcome::Unreadable {
            cause: UriPathError::NotAFileUri {
                uri: "untitled:Untitled-1".to_owned(),
            },
        });

        assert_eq!(outcome, CallerDomainsOutcome::UnreadableReferences);
    }

    #[test]
    fn test_caller_domains_outcome_of_a_working_server_is_not_no_references() {
        assert_eq!(
            undeclared_outcome_of(ReferencesOutcome::ServerStillWorking),
            CallerDomainsOutcome::ServerStillWorking
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_a_server_without_references_is_not_no_references() {
        assert_eq!(
            undeclared_outcome_of(ReferencesOutcome::NotSupported),
            CallerDomainsOutcome::ReferencesNotProvided
        );
    }

    #[test]
    fn test_caller_domains_of_no_references_cannot_be_built() {
        assert_eq!(
            CallerDomains::from_reference_paths(&[], &DomainDeclarations::default()),
            Ok(None)
        );
    }

    #[test]
    fn test_caller_domains_called_from_separate_domains_do_not_overlap() {
        let billing = undeclared_callers("/repo/src/billing/invoice.ts");
        let inventory = undeclared_callers("/repo/src/inventory/stock.ts");

        assert_eq!(billing.jaccard(&inventory).value(), 0.0);
    }

    #[test]
    fn test_caller_domains_outcome_of_a_reference_matching_two_declarations_is_not_counted() {
        // 対照は 1 つ目のテスト。そのファイルを落として数えると、材料が欠けたことが
        // 重なりの値からは見えない
        let declarations = declarations_of(&[
            ("billing", &["src/billing/**"]),
            ("reporting", &["src/**/report*.ts"]),
        ]);

        let outcome = outcome_of(
            answered(&[
                "/repo/src/billing/invoice.ts",
                "/repo/src/billing/report.ts",
            ]),
            &declarations,
            &mut ReferenceSources::default(),
        );

        let CallerDomainsOutcome::AmbiguousDomain(ambiguous) = outcome else {
            panic!("2 つの宣言に当たる参照元は数えない: {outcome:?}");
        };
        assert_eq!(ambiguous.path(), Path::new("/repo/src/billing/report.ts"));
    }
}
