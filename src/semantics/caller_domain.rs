//! 参照元がどのドメインに属しているかと、2 つのチャンクの間でのその重なり。
//!
//! **数える部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! サーバに尋ねるのは [`caller_domains_outcome_of`] だけで、そこは
//! `tests/semantics.rs` が実サーバで見る。判定に使うのは `classification`。

use std::path::PathBuf;

use crate::lsp::{ClientError, ReferencesOutcome, Session, SourceDocument};
use crate::semantics::domain::{Domain, DomainCounts};
use crate::similarity::Similarity;
use crate::source_position::SourcePosition;

/// サーバに参照元を尋ねて、ドメインごとに数えた結果。
///
/// **「取れなかった」を 1 つにまとめない。** `lsp::ReferencesOutcome` が理由を分けて
/// 持っているのを、そのまま運ぶ（`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerDomainsOutcome {
    /// 参照元をドメインごとに数えられた。
    Counted(CallerDomains),
    /// 参照元が 1 件も返らなかった。
    NoReferences,
    /// 参照元は返ったが、パスとして読めない URI が混じっていた。
    UnreadableReferences,
    /// サーバが作業中で、落ち着いた答えを受け取れなかった。
    ServerStillWorking,
    /// サーバが references を提供していない。
    ReferencesNotProvided,
}

/// その位置にある名前の参照元を尋ねて、ドメインごとに数える。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`position` は
/// `Chunk::name_position` が指す識別子の位置。
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
) -> Result<CallerDomainsOutcome, ClientError> {
    Ok(outcome_of(session.references(document, position)?))
}

/// references の答えを、ドメインごとに数えた結果へ読み替える。
///
/// **サーバとの往復から切り離してある。** ここを [`caller_domains_outcome_of`] の中に
/// 置くと、読み替えの枝が実サーバのテストからしか通らなくなる
/// (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
fn outcome_of(references: ReferencesOutcome) -> CallerDomainsOutcome {
    match references {
        ReferencesOutcome::Answered(reference_paths) => {
            match CallerDomains::from_reference_paths(&reference_paths) {
                Some(caller_domains) => CallerDomainsOutcome::Counted(caller_domains),
                None => CallerDomainsOutcome::NoReferences,
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
    /// `reference_paths` は `lsp::ReferencesOutcome::Answered` が持つ参照元。
    /// 1 件も無ければ作れないので `None` を返す。
    pub fn from_reference_paths(reference_paths: &[PathBuf]) -> Option<Self> {
        DomainCounts::from_paths(reference_paths).map(Self)
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

    fn answered(reference_paths: &[&str]) -> ReferencesOutcome {
        ReferencesOutcome::Answered(reference_paths.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn test_caller_domains_outcome_of_answers_counts_them_per_domain() {
        let outcome = outcome_of(answered(&[
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
                (
                    &Domain::of_path(Path::new("/repo/src/billing/invoice.ts")),
                    1
                ),
                (
                    &Domain::of_path(Path::new("/repo/src/inventory/stock.ts")),
                    2
                ),
            ]
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_no_answer_is_not_a_counted_empty_set() {
        // 対照は上のテスト。`Counted` にすると、後段は 0 ドメインを
        // 「別ドメインに散っていない」と読む
        assert_eq!(
            outcome_of(ReferencesOutcome::NoAnswer),
            CallerDomainsOutcome::NoReferences
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_an_empty_answer_is_not_a_counted_empty_set() {
        // `lsp` は空配列を `NoAnswer` に倒すが、ここでも受けておかないと
        // 0 件の `Answered` が「数えられた」側へ落ちる
        assert_eq!(
            outcome_of(answered(&[])),
            CallerDomainsOutcome::NoReferences
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_an_unreadable_uri_is_not_no_references() {
        // 「読めなかった」を「1 件も無い」に畳むと、利用者が直す先が消える
        let outcome = outcome_of(ReferencesOutcome::Unreadable {
            cause: UriPathError::NotAFileUri {
                uri: "untitled:Untitled-1".to_owned(),
            },
        });

        assert_eq!(outcome, CallerDomainsOutcome::UnreadableReferences);
    }

    #[test]
    fn test_caller_domains_outcome_of_a_working_server_is_not_no_references() {
        assert_eq!(
            outcome_of(ReferencesOutcome::ServerStillWorking),
            CallerDomainsOutcome::ServerStillWorking
        );
    }

    #[test]
    fn test_caller_domains_outcome_of_a_server_without_references_is_not_no_references() {
        assert_eq!(
            outcome_of(ReferencesOutcome::NotSupported),
            CallerDomainsOutcome::ReferencesNotProvided
        );
    }

    #[test]
    fn test_caller_domains_of_no_references_cannot_be_built() {
        assert_eq!(CallerDomains::from_reference_paths(&[]), None);
    }

    #[test]
    fn test_caller_domains_called_from_separate_domains_do_not_overlap() {
        let billing =
            CallerDomains::from_reference_paths(&[PathBuf::from("/repo/src/billing/invoice.ts")])
                .expect("テストが渡す参照元は 1 件以上");
        let inventory =
            CallerDomains::from_reference_paths(&[PathBuf::from("/repo/src/inventory/stock.ts")])
                .expect("テストが渡す参照元は 1 件以上");

        assert_eq!(billing.jaccard(&inventory).value(), 0.0);
    }
}
