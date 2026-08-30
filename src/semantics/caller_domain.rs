//! 参照元がどのドメインに属しているかと、2 つのチャンクの間でのその重なり。
//!
//! **数える部分は LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! サーバに尋ねるのは [`caller_domains_outcome_of`] だけで、そこは
//! `tests/semantics.rs` が実サーバで見る。判定に使うのは `classification`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lsp::{ClientError, ReferencesOutcome, Session, SourceDocument};
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
    let outcome = match session.references(document, position)? {
        ReferencesOutcome::Answered(reference_paths) => counted_outcome_of(&reference_paths),
        ReferencesOutcome::NoAnswer => CallerDomainsOutcome::NoReferences,
        ReferencesOutcome::Unreadable { .. } => CallerDomainsOutcome::UnreadableReferences,
        ReferencesOutcome::ServerStillWorking => CallerDomainsOutcome::ServerStillWorking,
        ReferencesOutcome::NotSupported => CallerDomainsOutcome::ReferencesNotProvided,
    };

    Ok(outcome)
}

/// 参照元をドメインごとに数えた結果。1 件も無ければ、その旨。
fn counted_outcome_of(reference_paths: &[PathBuf]) -> CallerDomainsOutcome {
    let Some(caller_domains) = CallerDomains::from_reference_paths(reference_paths) else {
        return CallerDomainsOutcome::NoReferences;
    };

    CallerDomainsOutcome::Counted(caller_domains)
}

/// ドメイン。そのファイルを含むディレクトリ。
///
/// **ディレクトリより上の推定をしない。** `src/billing/tax/rate.ts` を
/// 「billing のもの」と読むには、どの段が機能の境目かを決めることになり、
/// それはリポジトリのレイアウト次第で変わる。境界の宣言は `dryguard.toml`
/// （`docs/dryguard-plan.md`「ドメイン境界の自動推定は難しい」）が持つ。
///
/// **Why not（`ModuleDistance` と同じ扱いにする）**: あちらは 2 つのファイルの間の
/// 隔たりで、こちらは 1 つのファイルが属する場所。**距離では「どこに属するか」を
/// 名指せない**ので、呼び出し元の分布（`billing 3件 / inventory 5件`）を出せない。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Domain(PathBuf);

impl Domain {
    /// そのファイルが属するドメイン。
    ///
    /// ディレクトリを持たないパス（`pad.ts`）では、根の下にあるものとして空になる。
    pub fn of_path(path: &Path) -> Self {
        Self(path.parent().unwrap_or_else(|| Path::new("")).to_path_buf())
    }

    /// ドメインを表すディレクトリ。
    pub fn directory(&self) -> &Path {
        &self.0
    }
}

/// 呼び出し元が属するドメインと、そこから来ている参照元の数。
///
/// **件数を落とさない。** 出力に出るのは `呼び出し元も別機能に分布
/// (billing 3件 / inventory 5件)` という分布で、ドメインの一覧だけでは
/// どちらに寄っているかを言えない（`docs/dryguard-plan.md`「出力イメージ」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerDomains(BTreeMap<Domain, usize>);

impl CallerDomains {
    /// 参照元のファイルから、ドメインごとの件数にまとめる。
    ///
    /// `reference_paths` は `lsp::ReferencesOutcome::Answered` が持つ参照元。
    /// 1 件も無ければ作れないので `None` を返す。
    ///
    /// **空の集合を作らせない。** 参照元が 0 件のときの重なりは決められず
    /// （どのドメインとも重ならないのか、材料が無いのか）、作れてしまうと
    /// 呼び出し側がその判断を迫られる
    /// (`rules/coding.md`「生成時に検証し、不正な値を存在させない」)。
    pub fn from_reference_paths(reference_paths: &[PathBuf]) -> Option<Self> {
        let mut counts: BTreeMap<Domain, usize> = BTreeMap::new();

        for path in reference_paths {
            *counts.entry(Domain::of_path(path)).or_insert(0) += 1;
        }

        if counts.is_empty() {
            return None;
        }
        Some(Self(counts))
    }

    /// 2 つの呼び出し元集合の Jaccard 係数（共通しているドメインが、合わせたうちの何割か）。
    ///
    /// **件数では重み付けしない。** 同じドメインから 5 回呼ばれていることは、
    /// そのドメインが呼んでいるという 1 つの事実で、5 倍の証拠ではない。
    /// 件数は根拠の文（`billing 3件 / inventory 5件`）が使う。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        let shared = self
            .0
            .keys()
            .filter(|domain| other.0.contains_key(*domain))
            .count();
        let combined = self.0.len() + other.0.len() - shared;

        Similarity::from_shared_count(shared, combined)
    }

    /// ドメインごとの参照元の数。ドメインの綴り順に並ぶ。
    ///
    /// 並びを決めておくのは、**同じ入力に同じ出力を返させる**ため（根拠の文が
    /// 実行のたびに並び替わると、出力を突き合わせられない）。
    pub fn references_per_domain(&self) -> Vec<(&Domain, usize)> {
        self.0
            .iter()
            .map(|(domain, count)| (domain, *count))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller_domains(reference_paths: &[&str]) -> CallerDomains {
        let paths: Vec<PathBuf> = reference_paths.iter().map(PathBuf::from).collect();

        CallerDomains::from_reference_paths(&paths).expect("テストが渡す参照元は 1 件以上")
    }

    fn overlap(reference_paths_a: &[&str], reference_paths_b: &[&str]) -> f64 {
        caller_domains(reference_paths_a)
            .jaccard(&caller_domains(reference_paths_b))
            .value()
    }

    #[test]
    fn test_domain_of_a_file_is_the_directory_holding_it() {
        let domain = Domain::of_path(Path::new("/repo/src/billing/invoice.ts"));

        assert_eq!(domain.directory(), Path::new("/repo/src/billing"));
    }

    #[test]
    fn test_domain_of_files_in_the_same_directory_is_the_same_domain() {
        // ファイル名は見ない。見ていると、同じディレクトリの 2 つの呼び出し元が
        // 別ドメインとして数えられる
        let invoice = Domain::of_path(Path::new("/repo/src/billing/invoice.ts"));
        let statement = Domain::of_path(Path::new("/repo/src/billing/statement.ts"));

        assert_eq!(invoice, statement);
    }

    #[test]
    fn test_domain_of_a_nested_directory_is_not_the_directory_above_it() {
        // 対照は上のテスト。段を畳んで「billing のもの」と読むには、どの段が
        // 機能の境目かを決めることになる（それは dryguard.toml の担当）
        let rate = Domain::of_path(Path::new("/repo/src/billing/tax/rate.ts"));
        let invoice = Domain::of_path(Path::new("/repo/src/billing/invoice.ts"));

        assert_ne!(rate, invoice);
    }

    #[test]
    fn test_caller_domains_of_no_references_cannot_be_built() {
        assert_eq!(CallerDomains::from_reference_paths(&[]), None);
    }

    #[test]
    fn test_caller_domains_count_every_reference_in_the_same_domain() {
        let domains = caller_domains(&[
            "/repo/src/billing/invoice.ts",
            "/repo/src/billing/invoice.ts",
            "/repo/src/billing/statement.ts",
        ]);

        assert_eq!(
            domains.references_per_domain(),
            vec![(
                &Domain::of_path(Path::new("/repo/src/billing/invoice.ts")),
                3
            )]
        );
    }

    #[test]
    fn test_caller_domains_keep_each_domain_with_its_own_count() {
        // 対照は上のテスト。分布が出せることを見る（billing 1 件 / inventory 2 件）
        let domains = caller_domains(&[
            "/repo/src/billing/invoice.ts",
            "/repo/src/inventory/stock.ts",
            "/repo/src/inventory/restock.ts",
        ]);

        let counts: Vec<usize> = domains
            .references_per_domain()
            .iter()
            .map(|(_, count)| *count)
            .collect();
        assert_eq!(counts, vec![1, 2], "綴り順に billing / inventory");
    }

    #[test]
    fn test_caller_domains_called_only_from_the_same_domain_overlap_completely() {
        assert_eq!(
            overlap(
                &["/repo/src/report/monthly.ts"],
                &["/repo/src/report/monthly.ts", "/repo/src/report/daily.ts"]
            ),
            1.0
        );
    }

    #[test]
    fn test_caller_domains_called_from_separate_domains_do_not_overlap() {
        // 対照は上のテスト。呼び出し元のファイル数は同じで、属するドメインだけが違う
        assert_eq!(
            overlap(
                &["/repo/src/billing/invoice.ts"],
                &["/repo/src/inventory/stock.ts"]
            ),
            0.0
        );
    }

    #[test]
    fn test_caller_domains_sharing_one_of_two_domains_overlap_by_that_share() {
        // 合わせて 3 ドメイン、共通は 1 つ
        assert_eq!(
            overlap(
                &[
                    "/repo/src/billing/invoice.ts",
                    "/repo/src/report/monthly.ts"
                ],
                &["/repo/src/inventory/stock.ts", "/repo/src/report/daily.ts"]
            ),
            1.0 / 3.0
        );
    }

    #[test]
    fn test_caller_domains_overlap_does_not_weigh_how_often_a_domain_calls() {
        // 件数で重み付けしていると、片方だけ 5 件ある billing が重なりを押し下げる。
        // 上の「1 / 3」と同じ入力で、件数だけを増やしてある
        assert_eq!(
            overlap(
                &[
                    "/repo/src/billing/invoice.ts",
                    "/repo/src/billing/statement.ts",
                    "/repo/src/billing/tax.ts",
                    "/repo/src/billing/refund.ts",
                    "/repo/src/billing/credit.ts",
                    "/repo/src/report/monthly.ts",
                ],
                &["/repo/src/inventory/stock.ts", "/repo/src/report/daily.ts"]
            ),
            1.0 / 3.0
        );
    }
}
