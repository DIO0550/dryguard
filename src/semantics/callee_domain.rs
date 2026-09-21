//! 呼び出し先がどのドメインに属しているかと、2 つのチャンクの間でのその重なり。
//!
//! **`caller_domain` の向きを逆にした形。** あちらは**誰が使っているか**、ここは
//! **何に依存しているか**で、1 つの集合に混ぜない
//! （`rules/naming.md`「`callee` と `reference` を混ぜない」）。
//!
//! **数える部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! サーバに尋ねるのは [`callee_domains_outcome_of`] だけで、そこは
//! `tests/semantics.rs` が実サーバで見る。判定に使うのは `classification`。

use std::path::PathBuf;

use crate::lsp::{CalleesOutcome, ClientError, Session, SourceDocument};
use crate::semantics::domain::{Domain, DomainCounts};
use crate::similarity::Similarity;
use crate::source_position::SourcePosition;

/// サーバに呼び出し先を尋ねて、ドメインごとに数えた結果。
///
/// **「取れなかった」を 1 つにまとめない。** `lsp::CalleesOutcome` が理由を分けて
/// 持っているのを、そのまま運ぶ（`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeDomainsOutcome {
    /// 呼び出し先をドメインごとに数えられた。
    Counted(CalleeDomains),
    /// その位置に呼び出し関係の起点が無かった。
    ///
    /// **[`Self::NoCallees`] と分ける。** どちらも呼び出し先が返らないが、こちらは
    /// **尋ねる位置かサーバの側**の話、あちらは**対象のコードの側**の話
    /// （本当に何も呼んでいない）。
    NoCallHierarchyItem,
    /// 起点が 2 つ以上返り、どれがそのチャンクのものか決められなかった。
    SeveralCallHierarchyItems {
        /// 返った起点の数。どれだけ曖昧だったかを出すのに要る。
        count: usize,
    },
    /// 呼び出し先が 1 件も返らなかった。
    NoCallees,
    /// 呼び出し先は返ったが、パスとして読めない URI が混じっていた。
    UnreadableCallees,
    /// サーバが作業中で、落ち着いた答えを受け取れなかった。
    ServerStillWorking,
    /// サーバが callHierarchy を提供していない。
    CallHierarchyNotProvided,
}

/// その位置にある名前が呼んでいる相手を尋ねて、ドメインごとに数える。
///
/// `document` は先に [`Session::open_document`] で開かせておく。`position` は
/// `Chunk::name_position` が指す識別子の位置。
///
/// # Errors
///
/// そのドキュメントを開かせていないとき、往復が失敗したとき。
/// **呼び出し先が無い / 読めないは `Err` にしない**（会話は成立しているので、
/// シグナルが取れなかっただけ）。
pub fn callee_domains_outcome_of(
    session: &mut Session,
    document: &SourceDocument,
    position: SourcePosition,
) -> Result<CalleeDomainsOutcome, ClientError> {
    Ok(outcome_of(session.callees(document, position)?))
}

/// callHierarchy の答えを、ドメインごとに数えた結果へ読み替える。
///
/// **サーバとの往復から切り離してある。** ここを [`callee_domains_outcome_of`] の中に
/// 置くと、読み替えの枝が実サーバのテストからしか通らなくなる
/// (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
fn outcome_of(callees: CalleesOutcome) -> CalleeDomainsOutcome {
    match callees {
        CalleesOutcome::Answered(callee_paths) => {
            match CalleeDomains::from_callee_paths(&callee_paths) {
                Some(callee_domains) => CalleeDomainsOutcome::Counted(callee_domains),
                None => CalleeDomainsOutcome::NoCallees,
            }
        }
        CalleesOutcome::NoItem => CalleeDomainsOutcome::NoCallHierarchyItem,
        CalleesOutcome::SeveralItems { count } => {
            CalleeDomainsOutcome::SeveralCallHierarchyItems { count }
        }
        CalleesOutcome::NoCallees => CalleeDomainsOutcome::NoCallees,
        CalleesOutcome::Unreadable { .. } => CalleeDomainsOutcome::UnreadableCallees,
        CalleesOutcome::ServerStillWorking => CalleeDomainsOutcome::ServerStillWorking,
        CalleesOutcome::NotSupported => CalleeDomainsOutcome::CallHierarchyNotProvided,
    }
}

/// 呼び出し先が属するドメインと、そこにある呼び出し先の数。
///
/// **[`CallerDomains`](super::caller_domain::CallerDomains) と同じ型にしない。**
/// 向きが逆なので、1 つの型で持つと**呼び出し元集合と呼び出し先集合を突き合わせる
/// 呼び出しが書けてしまう**（`rules/naming.md`「`callee` と `reference` を混ぜない」、
/// `rules/coding.md`「不正な状態を型で表現できなくする」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalleeDomains(DomainCounts);

impl CalleeDomains {
    /// 呼び出し先のファイルから、ドメインごとの件数にまとめる。
    ///
    /// `callee_paths` は `lsp::CalleesOutcome::Answered` が持つ呼び出し先。
    /// 1 件も無ければ作れないので `None` を返す。
    pub fn from_callee_paths(callee_paths: &[PathBuf]) -> Option<Self> {
        DomainCounts::from_paths(callee_paths).map(Self)
    }

    /// 2 つの呼び出し先集合の Jaccard 係数。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        self.0.jaccard(&other.0)
    }

    /// ドメインごとの呼び出し先の数。ドメインの綴り順に並ぶ。
    pub fn callees_per_domain(&self) -> Vec<(&Domain, usize)> {
        self.0.per_domain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::lsp::UriPathError;

    fn answered(callee_paths: &[&str]) -> CalleesOutcome {
        CalleesOutcome::Answered(callee_paths.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn test_callee_domains_outcome_of_answers_counts_them_per_domain() {
        let outcome = outcome_of(answered(&[
            "/repo/src/utils/formatDate.ts",
            "/repo/src/report/dateHelper.ts",
            "/repo/src/report/label.ts",
        ]));

        let CalleeDomainsOutcome::Counted(callees) = outcome else {
            panic!("呼び出し先を数えられる: {outcome:?}");
        };
        assert_eq!(
            callees.callees_per_domain(),
            vec![
                (
                    &Domain::of_path(Path::new("/repo/src/report/dateHelper.ts")),
                    2
                ),
                (
                    &Domain::of_path(Path::new("/repo/src/utils/formatDate.ts")),
                    1
                ),
            ]
        );
    }

    #[test]
    fn test_callee_domains_outcome_of_no_callees_is_not_a_counted_empty_set() {
        // 対照は上のテスト。`Counted` にすると、後段は 0 ドメインを
        // 「依存先が食い違っていない」と読む
        assert_eq!(
            outcome_of(CalleesOutcome::NoCallees),
            CalleeDomainsOutcome::NoCallees
        );
    }

    #[test]
    fn test_callee_domains_outcome_of_an_empty_answer_is_not_a_counted_empty_set() {
        // `lsp` は空配列を `NoCallees` に倒すが、ここでも受けておかないと
        // 0 件の `Answered` が「数えられた」側へ落ちる
        assert_eq!(outcome_of(answered(&[])), CalleeDomainsOutcome::NoCallees);
    }

    #[test]
    fn test_callee_domains_outcome_of_no_start_is_not_an_absent_callee_set() {
        // 対照は上の `NoCallees` のテスト。畳むと、尋ねる位置が悪かったことと
        // 本当に何も呼んでいないことが同じ答えになる
        assert_eq!(
            outcome_of(CalleesOutcome::NoItem),
            CalleeDomainsOutcome::NoCallHierarchyItem
        );
    }

    #[test]
    fn test_callee_domains_outcome_of_several_starts_keeps_how_many_came_back() {
        assert_eq!(
            outcome_of(CalleesOutcome::SeveralItems { count: 3 }),
            CalleeDomainsOutcome::SeveralCallHierarchyItems { count: 3 }
        );
    }

    #[test]
    fn test_callee_domains_outcome_of_an_unreadable_uri_is_not_no_callees() {
        // 「読めなかった」を「1 件も無い」に畳むと、利用者が直す先が消える
        let outcome = outcome_of(CalleesOutcome::Unreadable {
            cause: UriPathError::NotAFileUri {
                uri: "untitled:Untitled-1".to_owned(),
            },
        });

        assert_eq!(outcome, CalleeDomainsOutcome::UnreadableCallees);
    }

    #[test]
    fn test_callee_domains_outcome_of_a_working_server_is_not_no_callees() {
        assert_eq!(
            outcome_of(CalleesOutcome::ServerStillWorking),
            CalleeDomainsOutcome::ServerStillWorking
        );
    }

    #[test]
    fn test_callee_domains_outcome_of_a_server_without_call_hierarchy_is_not_no_callees() {
        assert_eq!(
            outcome_of(CalleesOutcome::NotSupported),
            CalleeDomainsOutcome::CallHierarchyNotProvided
        );
    }

    #[test]
    fn test_callee_domains_of_no_callees_cannot_be_built() {
        assert_eq!(CalleeDomains::from_callee_paths(&[]), None);
    }

    #[test]
    fn test_callee_domains_calling_into_separate_domains_do_not_overlap() {
        let billing =
            CalleeDomains::from_callee_paths(&[PathBuf::from("/repo/src/billing/invoice.ts")])
                .expect("テストが渡す呼び出し先は 1 件以上");
        let inventory =
            CalleeDomains::from_callee_paths(&[PathBuf::from("/repo/src/inventory/stock.ts")])
                .expect("テストが渡す呼び出し先は 1 件以上");

        assert_eq!(billing.jaccard(&inventory).value(), 0.0);
    }

    #[test]
    fn test_callee_domains_calling_into_one_shared_domain_overlap_completely() {
        // 対照は上のテスト。呼び出し先のファイル数は同じで、属するドメインだけが揃っている。
        // 上は集合が持つドメインで名付けているが、ここは両方 utils なので、
        // 見分けが付く呼び出し先のほうで名付ける
        let calls_format_date =
            CalleeDomains::from_callee_paths(&[PathBuf::from("/repo/src/utils/formatDate.ts")])
                .expect("テストが渡す呼び出し先は 1 件以上");
        let calls_pad =
            CalleeDomains::from_callee_paths(&[PathBuf::from("/repo/src/utils/pad.ts")])
                .expect("テストが渡す呼び出し先は 1 件以上");

        assert_eq!(calls_format_date.jaccard(&calls_pad).value(), 1.0);
    }
}
