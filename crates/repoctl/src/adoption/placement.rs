//! Generic placement heuristics and explicit lane overrides.

use repoctl_core::{ProjectKind, RepoRelativePath, SourceInventory};

#[derive(Clone, Debug)]
pub(super) struct PlacementDecision {
    pub(super) destination: RepoRelativePath,
    pub(super) kind: ProjectKind,
    pub(super) confidence: f32,
    pub(super) reasons: Vec<String>,
    pub(super) override_applied: bool,
}

pub(super) fn infer_placement(
    name: &str,
    inventory: &SourceInventory,
    override_path: Option<RepoRelativePath>,
    override_kind: Option<ProjectKind>,
) -> PlacementDecision {
    if let Some(destination) = override_path {
        let kind = override_kind.unwrap_or_else(|| kind_from_destination(&destination));
        return PlacementDecision {
            destination,
            kind,
            confidence: 1.0,
            reasons: vec!["explicit placement override".to_string()],
            override_applied: true,
        };
    }
    let text = format!(
        "{} {} {}",
        name,
        inventory.readme_summary.clone().unwrap_or_default(),
        inventory.top_level_dirs.join(" ")
    )
    .to_lowercase();
    let (lane, kind, confidence, reason) = if text.contains("tool")
        || text.contains("skill")
        || text.contains("generator")
        || name == "skills"
    {
        (
            "tools",
            ProjectKind::Tool,
            0.88,
            "developer tooling indicators",
        )
    } else if text.contains("infrastructure")
        || text.contains("eks")
        || text.contains("iam")
        || text.contains("network")
        || inventory.manifests.iter().any(|path| path == "Pulumi.yaml")
            && !inventory.manifests.iter().any(|path| path == "Cargo.toml")
    {
        (
            "core-infra",
            ProjectKind::CoreInfraComponent,
            0.86,
            "platform infrastructure indicators",
        )
    } else if text.contains("framework") || text.contains("sdk") || text.contains("runtime") {
        (
            "frameworks",
            ProjectKind::Framework,
            0.90,
            "reusable framework indicators",
        )
    } else if text.contains("identity")
        || text.contains("registry")
        || text.contains("billing")
        || text.contains("auth")
    {
        (
            "foundations",
            ProjectKind::FoundationService,
            0.84,
            "foundation service indicators",
        )
    } else {
        (
            "apps",
            ProjectKind::App,
            0.78,
            "default product application lane",
        )
    };
    let kind = override_kind.unwrap_or(kind);
    PlacementDecision {
        destination: RepoRelativePath::new(format!("{lane}/{name}"))
            .unwrap_or_else(|_| RepoRelativePath::root()),
        kind,
        confidence,
        reasons: vec![reason.to_string()],
        override_applied: false,
    }
}

fn kind_from_destination(destination: &RepoRelativePath) -> ProjectKind {
    let path = destination.as_str();
    if path.starts_with("frameworks/") {
        ProjectKind::Framework
    } else if path.starts_with("foundations/") {
        ProjectKind::FoundationService
    } else if path.starts_with("core-infra/") {
        ProjectKind::CoreInfraComponent
    } else if path.starts_with("tools/") {
        ProjectKind::Tool
    } else {
        ProjectKind::App
    }
}
