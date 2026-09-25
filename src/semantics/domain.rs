//! ファイルが属するドメインと、ドメインごとの件数。
//!
//! **向き（呼び出し元 / 呼び出し先）を持たない。** 向きを持つのは
//! `caller_domain` / `callee_domain` の側で、ここはどちらからも同じ形で使える
//! 中身だけを持つ（`rules/naming.md`「`caller domain` と `callee domain` を混ぜない」）。
//!
//! **LSP を呼ばない**ので、サーバが無くても確かめられる
//! (`rules/tdd.md`「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::similarity::Similarity;

/// ドメイン。そのファイルを含むディレクトリ。
///
/// **ディレクトリより上の推定をしない。** `src/billing/tax/rate.ts` を
/// 「billing のもの」と読むには、どの段が機能の境目かを決めることになり、
/// それはリポジトリのレイアウト次第で変わる。境界の宣言は `dryguard.toml` の
/// ドメイン宣言が持つ（`docs/dryguard-plan.md`「ドメイン境界の自動推定は難しい」。
/// `[domains]` にパスの glob を並べる形に決めたが、**`crate::config` が今読むのは
/// `[thresholds]` だけ**）。
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

/// ドメインごとのファイルの件数。
///
/// **件数を落とさない。** 出力に出るのは `呼び出し元も別機能に分布
/// (billing 3件 / inventory 5件)` という分布で、ドメインの一覧だけでは
/// どちらに寄っているかを言えない（`docs/dryguard-plan.md`「出力イメージ」）。
///
/// **Why not（`CallerDomains` / `CalleeDomains` が直に `BTreeMap` を持つ）**:
/// [`Self::jaccard`] には「件数では重み付けしない」という判定の材料の作り方が埋まっている。
/// 両側に並べると、片方だけ直したときに**呼び出し元と呼び出し先で数え方が食い違う**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainCounts(BTreeMap<Domain, usize>);

impl DomainCounts {
    /// ファイルのパスから、ドメインごとの件数にまとめる。
    ///
    /// 1 件も無ければ作れないので `None` を返す。
    ///
    /// **空の集合を作らせない。** 0 件のときの重なりは決められず
    /// （どのドメインとも重ならないのか、材料が無いのか）、作れてしまうと
    /// 呼び出し側がその判断を迫られる
    /// (`rules/coding.md`「生成時に検証し、不正な値を存在させない」)。
    pub fn from_paths(paths: &[PathBuf]) -> Option<Self> {
        let mut counts: BTreeMap<Domain, usize> = BTreeMap::new();

        for path in paths {
            *counts.entry(Domain::of_path(path)).or_insert(0) += 1;
        }

        if counts.is_empty() {
            return None;
        }
        Some(Self(counts))
    }

    /// 2 つのドメイン集合の Jaccard 係数（共通しているドメインが、合わせたうちの何割か）。
    ///
    /// **件数では重み付けしない。** 同じドメインが 5 回出ていることは、
    /// そのドメインが関わっているという 1 つの事実で、5 倍の証拠ではない。
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

    /// ドメインごとの件数。ドメインの綴り順に並ぶ。
    ///
    /// 並びを決めておくのは、**同じ入力に同じ出力を返させる**ため（根拠の文が
    /// 実行のたびに並び替わると、出力を突き合わせられない）。
    pub fn per_domain(&self) -> Vec<(&Domain, usize)> {
        self.0
            .iter()
            .map(|(domain, count)| (domain, *count))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(paths: &[&str]) -> DomainCounts {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();

        DomainCounts::from_paths(&paths).expect("テストが渡すパスは 1 件以上")
    }

    fn overlap(paths_a: &[&str], paths_b: &[&str]) -> f64 {
        counts(paths_a).jaccard(&counts(paths_b)).value()
    }

    #[test]
    fn test_domain_of_a_file_is_the_directory_holding_it() {
        let domain = Domain::of_path(Path::new("/repo/src/billing/invoice.ts"));

        assert_eq!(domain.directory(), Path::new("/repo/src/billing"));
    }

    #[test]
    fn test_domain_of_files_in_the_same_directory_is_the_same_domain() {
        // ファイル名は見ない。見ていると、同じディレクトリの 2 つのファイルが
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
    fn test_domain_counts_of_no_paths_cannot_be_built() {
        assert_eq!(DomainCounts::from_paths(&[]), None);
    }

    #[test]
    fn test_domain_counts_count_every_path_in_the_same_domain() {
        let counts = counts(&[
            "/repo/src/billing/invoice.ts",
            "/repo/src/billing/invoice.ts",
            "/repo/src/billing/statement.ts",
        ]);

        assert_eq!(
            counts.per_domain(),
            vec![(
                &Domain::of_path(Path::new("/repo/src/billing/invoice.ts")),
                3
            )]
        );
    }

    #[test]
    fn test_domain_counts_keep_each_domain_with_its_own_count() {
        // 対照は上のテスト。分布が出せることを見る（billing 1 件 / inventory 2 件）
        let counts = counts(&[
            "/repo/src/billing/invoice.ts",
            "/repo/src/inventory/stock.ts",
            "/repo/src/inventory/restock.ts",
        ]);

        let per_domain: Vec<usize> = counts
            .per_domain()
            .iter()
            .map(|(_, count)| *count)
            .collect();
        assert_eq!(per_domain, vec![1, 2], "綴り順に billing / inventory");
    }

    #[test]
    fn test_domain_counts_in_one_shared_domain_overlap_completely() {
        assert_eq!(
            overlap(
                &["/repo/src/report/monthly.ts"],
                &["/repo/src/report/monthly.ts", "/repo/src/report/daily.ts"]
            ),
            1.0
        );
    }

    #[test]
    fn test_domain_counts_in_separate_domains_do_not_overlap() {
        // 対照は上のテスト。ファイル数は同じで、属するドメインだけが違う
        assert_eq!(
            overlap(
                &["/repo/src/billing/invoice.ts"],
                &["/repo/src/inventory/stock.ts"]
            ),
            0.0
        );
    }

    #[test]
    fn test_domain_counts_sharing_one_of_two_domains_overlap_by_that_share() {
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
    fn test_domain_counts_overlap_does_not_weigh_how_often_a_domain_appears() {
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
