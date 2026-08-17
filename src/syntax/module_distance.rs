//! 2 つのファイルの間の、ディレクトリツリー上の隔たり。
//!
//! **ドメイン境界の代理指標でしかない。** ディレクトリが分かれていることと
//! ドメインが違うことは別で、本格的な宣言は `dryguard.toml`（Phase 3）で行う
//! （`docs/dryguard-plan.md`「ドメイン境界の自動推定は難しい」）。
//! Phase 0 では代理指標と割り切り、ディレクトリ構造だけでどこまで言えるかを見る材料にする。

use std::path::{Component, Path};

/// 2 つのファイルの間の、ディレクトリツリー上の段数。同じディレクトリなら 0。
///
/// 0.0-1.0 に正規化しないのは、**何段を「遠い」とみなすかが判定だから**。
/// ここで正規化すると閾値の判断が `syntax` に漏れる
/// (rules/architecture.md「判定は 1 箇所にだけ置く」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleDistance(usize);

impl ModuleDistance {
    /// 2 つのファイルを隔てているディレクトリの段数。
    ///
    /// 共通の親までさかのぼる段数と、そこから下りる段数の合計。
    /// ファイル名は見ない（同じディレクトリにある別々のファイルは 0）。
    ///
    /// `new` ではなく `between` と呼ぶのは、**2 つのパスの間の関係**であって
    /// パスがこの値の材料ではないため（`Location::new(path, line)` とは形が違う）。
    pub fn between(path_a: &Path, path_b: &Path) -> Self {
        let directories_a = directories_of(path_a);
        let directories_b = directories_of(path_b);

        let shared = directories_a
            .iter()
            .zip(directories_b.iter())
            .take_while(|(a, b)| a == b)
            .count();

        Self((directories_a.len() - shared) + (directories_b.len() - shared))
    }

    /// 隔たりの段数そのもの。
    pub fn steps(self) -> usize {
        self.0
    }
}

/// そのファイルを含むディレクトリを、上から順に並べたもの。
fn directories_of(path: &Path) -> Vec<&str> {
    let directory = path.parent().unwrap_or_else(|| Path::new(""));

    directory
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn steps(path_a: &str, path_b: &str) -> usize {
        ModuleDistance::between(Path::new(path_a), Path::new(path_b)).steps()
    }

    #[test]
    fn test_module_distance_of_two_files_in_the_same_directory_is_zero() {
        assert_eq!(
            steps("src/billing/discount.ts", "src/billing/invoice.ts"),
            0
        );
    }

    #[test]
    fn test_module_distance_of_two_files_in_sibling_directories_counts_both_steps() {
        // 共通の親は src/。billing から 1 段上がって inventory へ 1 段下りる
        assert_eq!(
            steps("src/billing/discount.ts", "src/inventory/reorder.ts"),
            2
        );
    }

    #[test]
    fn test_module_distance_of_a_file_nested_below_the_other_counts_only_the_descent() {
        // 片側がもう片側のディレクトリの下にあるので、上がる分は 0
        assert_eq!(
            steps("src/billing/tax/rate.ts", "src/billing/invoice.ts"),
            1
        );
    }

    #[test]
    fn test_module_distance_ignores_the_file_name() {
        // 名前が違うだけの同じディレクトリを離れていると数えない
        assert_eq!(steps("src/a/x.ts", "src/a/y.ts"), 0);
    }

    #[test]
    fn test_module_distance_of_files_sharing_no_directory_counts_every_step() {
        assert_eq!(steps("billing/discount.ts", "inventory/reorder.ts"), 2);
    }
}
