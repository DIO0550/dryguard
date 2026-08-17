//! import 文が指す依存先と、その集合。
//!
//! 指定子を**文字列のまま比べない**。`./pad` と `../utils/pad` は綴りが違うだけで
//! 同じファイルを指しうるので、そのまま比べると共有している依存を「別物」と数える。
//! 依存先が食い違っていると誤って言うのは、このツールが最も損をする外し方
//! （共有ユーティリティに「共通化するな」と言うことになる）。

use std::path::{Component, Path};

/// 解決済みの依存先。相対指定は importer の位置から畳んである。
///
/// 区切りは常に `/`。プラットフォームの区切りをそのまま持つと、
/// 同じ依存先が OS によって別の値になる。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath(String);

impl ModulePath {
    /// import 指定子を、それを書いているファイルの位置から解決する。
    ///
    /// `specifier` は `from` の後ろに書かれた文字列、`importer` はそれを書いているファイル。
    /// 相対指定（`./` / `../`）だけを畳み、パッケージ名は書かれたまま返す
    /// （パッケージ名は importer の位置に依らない）。
    ///
    /// **ファイルシステムは見ない。** 指す先が実在するとは限らず
    /// （拡張子の省略・`tsconfig` のパスエイリアス）、`syntax` は I/O を持てない
    /// (rules/coding.md 禁止事項)。
    pub fn from_specifier(specifier: &str, importer: &Path) -> Self {
        if !is_relative(specifier) {
            return Self(specifier.to_string());
        }

        let directory = importer.parent().unwrap_or_else(|| Path::new(""));
        Self(folded_path(directory, specifier))
    }

    /// 解決済みの依存先そのもの。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// その指定子が importer の位置から解決するものか。
fn is_relative(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../") || specifier == ".."
}

/// `directory` から見た `specifier` を、`.` と `..` を畳んだ 1 本のパスにする。
fn folded_path(directory: &Path, specifier: &str) -> String {
    let mut segments: Vec<String> = directory
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => climb(&mut segments),
            name => segments.push(name.to_string()),
        }
    }

    segments.join("/")
}

/// 1 つ上のディレクトリへ移る。
///
/// 遡る先が無いときは `..` を残す。畳めなかったことを黙って捨てると、
/// ツリーの外を指す別々の依存先が同じ値になる。
fn climb(segments: &mut Vec<String>) {
    let can_pop = segments.last().is_some_and(|segment| segment != "..");
    if can_pop {
        segments.pop();
        return;
    }
    segments.push("..".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn resolved(specifier: &str, importer: &str) -> String {
        ModulePath::from_specifier(specifier, Path::new(importer))
            .as_str()
            .to_string()
    }

    #[test]
    fn test_module_path_of_a_sibling_specifier_is_relative_to_the_importer() {
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_path_of_a_parent_specifier_climbs_out_of_the_importer_directory() {
        assert_eq!(
            resolved("../utils/pad", "src/report/dateHelper.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_paths_of_two_importers_pointing_at_the_same_file_are_equal() {
        // このツールの最悪の外し方（共有ユーティリティに「共通化するな」と言う）は、
        // 指定子を文字列のまま比べたときに起きる
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            resolved("../utils/pad", "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_module_path_that_climbs_past_the_top_keeps_the_remaining_parent_steps() {
        // 遡り切れなかった `..` を捨てると、ツリーの外を指す別々の依存先が同じ値になる
        assert_eq!(resolved("../../shared", "src/pad.ts"), "../shared");
    }

    #[test]
    fn test_module_path_of_a_package_specifier_is_kept_as_written() {
        // パッケージ名は importer の位置に依らない
        assert_eq!(resolved("react", "src/utils/formatDate.ts"), "react");
    }
}
