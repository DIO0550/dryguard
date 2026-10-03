use std::collections::BTreeSet;

fn provides_type_definition(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.type_definition_provider,
        Some(
            TypeDefinitionProviderCapability::Simple(true)
                | TypeDefinitionProviderCapability::Options(_)
        )
    )
}

fn provides_call_hierarchy(capabilities: &ServerCapabilities) -> bool {
    matches!(
        capabilities.call_hierarchy_provider,
        Some(
            CallHierarchyServerCapability::Simple(true) | CallHierarchyServerCapability::Options(_)
        )
    )
}

fn unrooted_project_text_of(markers: &[String]) -> String {
    if markers.is_empty() {
        return "プロジェクトの印が見つからない".to_owned();
    }

    format!(
        "プロジェクトの印が見つからない: {} のどれかを置くと参照元が揃う",
        markers.join(" / ")
    )
}

fn outside_project_text_of(markers: &[String]) -> String {
    if markers.is_empty() {
        return "プロジェクトの範囲から外れている".to_owned();
    }

    format!(
        "プロジェクトの範囲から外れている: {} が指す範囲を見直すと参照元が揃う",
        markers.join(" / ")
    )
}

impl ChunkType {
    fn traceable_type_names(&self) -> Option<BTreeSet<String>> {
        match self {
            Self::Callable(callable) => callable.traceable_type_names(),
            Self::Read(member_type) | Self::Written(member_type) => {
                member_type.traceable_type_names()
            }
        }
    }

    fn value_type_names(&self) -> Option<BTreeSet<String>> {
        match self {
            Self::Callable(callable) => callable.value_type_names(),
            Self::Read(member_type) | Self::Written(member_type) => {
                member_type.traceable_type_names()
            }
        }
    }
}

fn directory_of(path: &str) -> Domain {
    let declarations = DomainDeclarations::default();
    Domain::of_path(Path::new(path), &declarations).expect("宣言が無ければ食い違わない")
}

fn domain_of(file: &str) -> Domain {
    let declared = DomainDeclarations::default();
    Domain::of_path(Path::new(file), &declared).expect("宣言が無ければ食い違わない")
}
