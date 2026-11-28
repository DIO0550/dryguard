//! `Package.swift` の初期ターゲット配列に静的に書かれたテストの配置。

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use super::tree::{Grammar, SyntaxTree};

pub(crate) const MANIFEST_FILE_NAME: &str = "Package.swift";

pub(crate) fn is_versioned_manifest_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("Package@swift-") && name.ends_with(".swift"))
}

const PACKAGE_INITIALIZERS: [&str; 2] = ["Package", "PackageDescription.Package"];
const TEST_TARGET_FACTORIES: [&str; 3] = [
    ".testTarget",
    "Target.testTarget",
    "PackageDescription.Target.testTarget",
];
const RESOURCE_FACTORIES: [&str; 9] = [
    ".copy",
    "Resource.copy",
    "PackageDescription.Resource.copy",
    ".process",
    "Resource.process",
    "PackageDescription.Resource.process",
    ".embedInCode",
    "Resource.embedInCode",
    "PackageDescription.Resource.embedInCode",
];

#[derive(Debug)]
pub(crate) struct SwiftTestTarget {
    name: String,
    directory: PathBuf,
    sources: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
}

impl SwiftTestTarget {
    /// パッケージの初期 `targets:` 配列から配置を確かめられるテストターゲットを読む。
    ///
    /// マニフェストの後続の処理は評価しない。静的に配置が取れたものだけを除外の根拠にする。
    pub(crate) fn from_manifest(source: &str, package_directory: &Path) -> Vec<Self> {
        let Ok(tree) = SyntaxTree::from_source(source, Grammar::Swift) else {
            return Vec::new();
        };
        Self::from_tree(&tree, package_directory)
    }

    pub(crate) fn from_tree(tree: &SyntaxTree<'_>, package_directory: &Path) -> Vec<Self> {
        if tree.has_error() {
            return Vec::new();
        }

        let Some(targets) = targets_array_of(tree) else {
            return Vec::new();
        };
        let mut cursor = targets.walk();
        targets
            .children_by_field_name("element", &mut cursor)
            .filter_map(|element| Self::of_declaration(tree, element, package_directory))
            .collect()
    }

    fn of_declaration(
        tree: &SyntaxTree<'_>,
        declaration: Node<'_>,
        package_directory: &Path,
    ) -> Option<Self> {
        let arguments = arguments_of(tree, declaration, &TEST_TARGET_FACTORIES)?;
        let name = plain_string_of(tree, *arguments.get("name")?)?;
        if Path::new(name).file_name()? != name {
            return None;
        }

        let path = arguments
            .get("path")
            .filter(|node| node.kind() != "nil_literal");
        let directory = match path {
            Some(path) => relative_path_of(tree, *path)?,
            None => Path::new("Tests").join(name),
        };
        let sources = arguments
            .get("sources")
            .filter(|node| node.kind() != "nil_literal");
        let sources = paths_of_array(sources.copied(), |node| relative_path_of(tree, node))?;
        let mut excluded = paths_of_array(arguments.get("exclude").copied(), |node| {
            relative_path_of(tree, node)
        })?;
        excluded.extend(paths_of_array(
            arguments.get("resources").copied(),
            |node| resource_path_of(tree, node),
        )?);
        // パッケージの根を指す宣言で、他のターゲットのソースまで除外しない。
        if directory.as_os_str().is_empty()
            && (sources.is_empty() || sources.iter().any(|path| path.as_os_str().is_empty()))
        {
            return None;
        }

        Some(Self {
            name: name.to_owned(),
            directory: package_directory.join(directory),
            sources,
            excluded,
        })
    }

    /// 参照元がテストターゲットの配置とソースの指定に当たるか。
    pub(crate) fn contains(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.directory) else {
            return false;
        };
        (self.sources.is_empty()
            || self
                .sources
                .iter()
                .any(|source| relative.starts_with(source)))
            && !self
                .excluded
                .iter()
                .any(|excluded| relative.starts_with(excluded))
    }
}

/// 走査と同じ版のマニフェストから確かめた SwiftPM のテストターゲット。
#[derive(Default)]
pub(crate) struct SwiftTestPackages<'path>(BTreeMap<&'path Path, ManifestLayouts>);

/// 名前が同じでも、別パッケージのテストターゲットは同一視しない。
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SwiftTestTargetId<'layout> {
    package: &'layout Path,
    name: &'layout str,
}

#[derive(Default)]
struct ManifestLayouts {
    default: Option<Vec<SwiftTestTarget>>,
    versioned: Vec<Vec<SwiftTestTarget>>,
}

impl<'path> SwiftTestPackages<'path> {
    pub(crate) fn record_manifest(&mut self, path: &'path Path, targets: Vec<SwiftTestTarget>) {
        let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if name == MANIFEST_FILE_NAME {
            self.0.entry(directory).or_default().default = Some(targets);
        } else if is_versioned_manifest_name(name) {
            self.0.entry(directory).or_default().versioned.push(targets);
        }
    }

    /// 最寄りのパッケージで、どのマニフェストでも同じ単一のテストターゲットに当たるときだけ返す。
    /// 読めない・配置が曖昧な内側のマニフェストを外側で補わない。
    pub(crate) fn target_of(&self, path: &Path) -> Option<SwiftTestTargetId<'_>> {
        let (package, layouts) = path
            .parent()?
            .ancestors()
            .find_map(|parent| self.0.get_key_value(parent))?;
        if !can_be_target_source(path.strip_prefix(package).ok()?) {
            return None;
        }
        let name = unique_target_name_of(layouts.default.as_deref()?, path)?;
        layouts
            .versioned
            .iter()
            .all(|targets| unique_target_name_of(targets, path) == Some(name))
            .then_some(SwiftTestTargetId { package, name })
    }
}

/// SwiftPM が探索で除くパスは、別ファイルの型宣言を結びつける根拠にしない。
fn can_be_target_source(relative: &Path) -> bool {
    let Some(file_name) = relative.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let at_package_root = relative.parent() == Some(Path::new(""));
    if at_package_root
        && (file_name == MANIFEST_FILE_NAME
            || (file_name.starts_with("Package@") && file_name.ends_with(".swift")))
    {
        return false;
    }
    relative.iter().all(|component| {
        component.to_str().is_some_and(|name| {
            !name.starts_with('.')
                && !name.ends_with(".xcodeproj")
                && !name.ends_with(".playground")
                && !name.ends_with(".xcworkspace")
        })
    })
}

fn unique_target_name_of<'targets>(
    targets: &'targets [SwiftTestTarget],
    path: &Path,
) -> Option<&'targets str> {
    let mut matching = targets.iter().filter(|target| target.contains(path));
    let target = matching.next()?;
    matching.next().is_none().then_some(&target.name)
}

fn targets_array_of<'tree>(tree: &'tree SyntaxTree<'_>) -> Option<Node<'tree>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut packages = root.named_children(&mut cursor).filter(|node| {
        node.kind() == "property_declaration"
            && node
                .child_by_field_name("name")
                .and_then(|name| tree.text_of(name))
                == Some("package")
    });
    let package = packages.next()?;
    if packages.next().is_some() {
        return None;
    }
    let initializer = single_field_of(package, "value")?;
    let arguments = arguments_of(tree, initializer, &PACKAGE_INITIALIZERS)?;
    let targets = *arguments.get("targets")?;
    (targets.kind() == "array_literal").then_some(targets)
}

fn arguments_of<'tree, 'source>(
    tree: &'tree SyntaxTree<'source>,
    call: Node<'tree>,
    callees: &[&str],
) -> Option<HashMap<&'source str, Node<'tree>>> {
    let arguments = matching_call_arguments(tree, call, callees)?;
    let mut cursor = arguments.walk();
    let mut values = HashMap::new();
    for argument in arguments
        .named_children(&mut cursor)
        .filter(|node| !node.is_extra())
    {
        let label = tree.text_of(argument.child_by_field_name("name")?)?;
        let value = single_field_of(argument, "value")?;
        if values.insert(label, value).is_some() {
            return None;
        }
    }
    Some(values)
}

fn matching_call_arguments<'tree>(
    tree: &SyntaxTree<'_>,
    call: Node<'tree>,
    callees: &[&str],
) -> Option<Node<'tree>> {
    if call.kind() != "call_expression" {
        return None;
    }
    let mut cursor = call.walk();
    let mut children = call
        .named_children(&mut cursor)
        .filter(|child| !child.is_extra());
    let callee = tree.text_of(children.next()?)?;
    let suffix = children.next()?;
    if suffix.kind() != "call_suffix" || children.next().is_some() {
        return None;
    }
    let spelling: String = callee.split_whitespace().collect();
    if !callees.contains(&spelling.as_str()) {
        return None;
    }
    let mut cursor = suffix.walk();
    let mut suffix_children = suffix
        .named_children(&mut cursor)
        .filter(|child| !child.is_extra());
    let arguments = suffix_children.next()?;
    if arguments.kind() != "value_arguments" || suffix_children.next().is_some() {
        return None;
    }
    Some(arguments)
}

fn single_field_of<'tree>(node: Node<'tree>, field: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let mut values = node.children_by_field_name(field, &mut cursor);
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn plain_string_of<'source>(tree: &SyntaxTree<'source>, node: Node<'_>) -> Option<&'source str> {
    if node.kind() != "line_string_literal" {
        return None;
    }
    let text = tree.text_of(node)?.strip_prefix('"')?.strip_suffix('"')?;
    (!text.contains(['\\', '\0'])).then_some(text)
}

fn relative_path_of(tree: &SyntaxTree<'_>, node: Node<'_>) -> Option<PathBuf> {
    let path = Path::new(plain_string_of(tree, node)?);
    let path = path.strip_prefix(".").unwrap_or(path);
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        .then(|| path.to_path_buf())
}

fn resource_path_of(tree: &SyntaxTree<'_>, call: Node<'_>) -> Option<PathBuf> {
    let arguments = matching_call_arguments(tree, call, &RESOURCE_FACTORIES)?;
    let mut cursor = arguments.walk();
    let path = arguments
        .named_children(&mut cursor)
        .find(|node| !node.is_extra())?;
    if path.kind() != "value_argument" || path.child_by_field_name("name").is_some() {
        return None;
    }
    relative_path_of(tree, single_field_of(path, "value")?)
}

fn paths_of_array<'tree>(
    array: Option<Node<'tree>>,
    path_of: impl FnMut(Node<'tree>) -> Option<PathBuf>,
) -> Option<Vec<PathBuf>> {
    let Some(array) = array else {
        return Some(Vec::new());
    };
    if array.kind() != "array_literal" {
        return None;
    }
    let mut cursor = array.walk();
    array
        .children_by_field_name("element", &mut cursor)
        .map(path_of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swift_test_package_requires_unambiguous_and_consistent_target_membership() {
        let root = Path::new("Shop");
        let normal = r#"let package = Package(name: "Shop", targets: [
    .testTarget(name: "Unit", path: "Checks"),
    .testTarget(name: "Manual", path: "Checks/Manual")
])"#;
        let versioned = r#"let package = Package(name: "Shop", targets: [
    .testTarget(name: "Unit", path: "Checks", exclude: ["Omitted.swift", "Switched"]),
    .testTarget(name: "Switched", path: "Checks/Switched"),
    .testTarget(name: "Alternate", path: "Versioned")
])"#;
        let manifest = root.join("Package.swift");
        let alternate = root.join("Package@swift-5.9.swift");
        let mut packages = SwiftTestPackages::default();
        packages.record_manifest(&manifest, SwiftTestTarget::from_manifest(normal, root));
        packages.record_manifest(&alternate, SwiftTestTarget::from_manifest(versioned, root));

        let owner = |name| packages.target_of(&root.join(name));
        let included = owner("Checks/Included.swift").expect("両マニフェストで Unit に属する");
        assert!(owner("Checks/Subfolder/Helper.swift") == Some(included));
        assert!(owner("Checks/Manual/Overlapping.swift").is_none());
        assert!(owner("Checks/Omitted.swift").is_none());
        assert!(owner("Checks/Switched/ChangedTarget.swift").is_none());
        assert!(owner("Checks/.Hidden.swift").is_none());
        assert!(owner("Checks/.fixtures/Hidden.swift").is_none());
        assert!(owner("Checks/Demo.playground/Example.swift").is_none());
        assert!(owner("Versioned/OnlyAlternate.swift").is_none());
    }

    #[test]
    fn test_swift_test_package_keeps_target_identity_separate_across_packages() {
        let manifest_a = Path::new("Workspace/Package.swift");
        let manifest_b = Path::new("Workspace/Tests/Unit/Nested/Package.swift");
        let normal = r#"let package = Package(name: "Shop", targets: [
    .testTarget(name: "Unit")
])"#;
        let mut packages = SwiftTestPackages::default();
        for manifest in [manifest_a, manifest_b] {
            let directory = manifest.parent().expect("マニフェストに親がある");
            packages.record_manifest(manifest, SwiftTestTarget::from_manifest(normal, directory));
        }
        let outer = packages
            .target_of(Path::new("Workspace/Tests/Unit/Outer.swift"))
            .expect("外側の Unit に属する");
        let inner = packages
            .target_of(Path::new(
                "Workspace/Tests/Unit/Nested/Tests/Unit/Inner.swift",
            ))
            .expect("内側の Unit に属する");

        assert!(outer != inner, "別パッケージの同名ターゲット");
        assert!(
            packages
                .target_of(Path::new("Workspace/Tests/Unit/Nested/Unassigned.swift"))
                .is_none(),
            "内側に所属がないファイルを外側の Unit に結びつけない"
        );
    }

    #[test]
    fn test_swift_test_targets_read_literal_paths_and_source_lists() {
        let manifest = r#"
import PackageDescription
var package = PackageDescription.Package(name: "Shop", targets: [
    .testTarget(name: "UnitTests", exclude: ["Fixtures"]),
    Target . testTarget(name: "IntegrationTests", path: "./Checks/Integration Tests",
                        exclude: ["Included/Manual.swift"], sources: ["Included", "Single.swift"],
                        resources: [.copy("Included/Shared.swift"), Resource.process("Included/Assets")]),
    PackageDescription.Target.testTarget(name: "SmokeTests", path: nil, sources: []),
    .testTarget(name: "RootTests", path: ".", sources: ["RootTests/Checks.swift"]),
    .target(name: "Examples", path: "Tests/Examples"),
    .target(name: "Shared", path: "Checks/Integration Tests", sources: ["Included/Shared.swift"]),
])
let unused = Target.testTarget(name: "Unused")
"#;
        let root = Path::new("ShopPackage");
        let targets = SwiftTestTarget::from_manifest(manifest, root);
        for (file, is_test) in [
            ("Tests/UnitTests/Helpers/Check.swift", true),
            ("Tests/UnitTests/Fixtures/Check.swift", false),
            ("Tests/UnitTests/FixturesBackup/Check.swift", true),
            ("Tests/UnitTestsExtra/Check.swift", false),
            ("Tests/Examples/Check.swift", false),
            ("Tests/Unused/Check.swift", false),
            ("Checks/Integration Tests/Included/Helper.swift", true),
            ("Checks/Integration Tests/Included/Manual.swift", false),
            ("Checks/Integration Tests/Included/Shared.swift", false),
            (
                "Checks/Integration Tests/Included/Assets/Source.swift",
                false,
            ),
            ("Checks/Integration Tests/Single.swift", true),
            ("Checks/Integration Tests/Other.swift", false),
            ("Tests/SmokeTests/Helper.swift", true),
            ("RootTests/Checks.swift", true),
            ("RootTests/Other.swift", false),
            ("Sources/Shop/Checkout.swift", false),
        ] {
            assert_eq!(
                targets
                    .iter()
                    .any(|target| target.contains(&root.join(file))),
                is_test,
                "{file}"
            );
        }
    }

    #[test]
    fn test_swift_test_targets_keep_files_when_a_declaration_cannot_be_resolved() {
        let root = Path::new("ShopPackage");
        for (arguments, possible_path) in [
            (r#"path: customPath"#, "Tests/Unclear/Helper.swift"),
            (r#"path: "Tests/\(name)""#, "Tests/Unclear/Helper.swift"),
            (r#"path: "/tmp""#, "/tmp/Helper.swift"),
            (r#"path: "../Neighbor""#, "../Neighbor/Helper.swift"),
            (r#"path: ".""#, "Sources/Shop/Checkout.swift"),
            (r#"path: "", sources: ["."]"#, "Sources/Shop/Checkout.swift"),
            (
                r#"path: "Tests/Unclear", path: nil"#,
                "Tests/Unclear/Helper.swift",
            ),
            (
                r#"sources: configuredSources"#,
                "Tests/Unclear/Helper.swift",
            ),
            (
                r#"exclude: configuredExclusions"#,
                "Tests/Unclear/Helper.swift",
            ),
            (
                r#"resources: [.copy(resourcePath)]"#,
                "Tests/Unclear/Helper.swift",
            ),
        ] {
            let manifest = format!(
                r#"import PackageDescription
let package = Package(name: "Shop", targets: [
    .testTarget(name: "Unclear", {arguments}),
    .testTarget(name: "Known", sources: nil),
])"#
            );
            let targets = SwiftTestTarget::from_manifest(&manifest, root);
            assert!(
                targets
                    .iter()
                    .any(|target| target.contains(&root.join("Tests/Known/Helper.swift"))),
                "{arguments}"
            );
            assert!(
                !targets
                    .iter()
                    .any(|target| target.contains(&root.join(possible_path))),
                "{arguments}"
            );
        }
    }
}
