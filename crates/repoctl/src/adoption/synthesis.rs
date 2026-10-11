//! Project manifests, workspace commands and infrastructure synthesis.

use std::{collections::BTreeMap, fmt::Write as _, fs};

use camino::Utf8Path;
use repoctl_core::{
    Diagnostic, OwnerHandle, ProjectKind, ProjectName, RepoRelativePath, RepoctlError,
    SourceInventory,
};
use serde::Deserialize;
use serde_json::Value;

use super::placement::PlacementDecision;

pub(super) fn synthesize_project_manifest(
    source_path: &Utf8Path,
    placement: &PlacementDecision,
    inventory: &SourceInventory,
    owner: Option<&OwnerHandle>,
) -> Result<String, RepoctlError> {
    let project = project_name_for_destination(&placement.destination, &placement.kind)?;
    let owner = owner.map_or("@platform", OwnerHandle::as_str);
    let mut output = format!(
        "schema: company.project/v1\nname: {project}\nkind: {}\npath: {}\nowners:\n  - \
         \"{owner}\"\n",
        project_kind_manifest_name(&placement.kind),
        placement.destination,
    );
    let workspaces = synthesize_workspaces(inventory);
    if !workspaces.is_empty() {
        output.push_str("\nworkspaces:\n");
        for workspace in &workspaces {
            output.push_str(workspace);
        }
    }
    let tasks = synthesize_tasks(source_path, &workspaces)?;
    if !tasks.is_empty() {
        output.push_str("\ntasks:\n");
        output.push_str(&tasks);
    }
    if let Some(iac) = synthesize_iac(inventory)? {
        output.push('\n');
        output.push_str(&iac);
    }
    if inventory.top_level_dirs.iter().any(|dir| dir == "docs") {
        output.push_str("\nai:\n  docs:\n    - docs\n");
    }
    if placement.kind == ProjectKind::Tool && workspaces.is_empty() {
        output.push_str("\n# docs/spec-only tool; tasks intentionally omitted\n");
    }
    Ok(output)
}

pub(super) fn project_kind_manifest_name(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::App => "app",
        ProjectKind::Framework => "framework",
        ProjectKind::FoundationService => "foundation-service",
        ProjectKind::ProtoRoot => "proto-root",
        ProjectKind::CoreInfra => "core-infra",
        ProjectKind::CoreInfraComponent => "core-infra-component",
        ProjectKind::Tool => "tool",
    }
}

pub(super) fn project_name_for_destination(
    destination: &RepoRelativePath,
    kind: &ProjectKind,
) -> Result<ProjectName, RepoctlError> {
    let path = destination.as_str();
    let slug = path.rsplit('/').next().ok_or_else(|| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.project.name",
            "destination path must include a project slug",
        ))
    })?;
    let name = match kind {
        ProjectKind::App => format!("apps.{slug}"),
        ProjectKind::Framework => format!("frameworks.{slug}"),
        ProjectKind::FoundationService => format!("foundations.{slug}"),
        ProjectKind::CoreInfraComponent => format!("core-infra.{slug}"),
        ProjectKind::Tool => format!("tools.{slug}"),
        ProjectKind::ProtoRoot => "protos.shared".to_string(),
        ProjectKind::CoreInfra => "core-infra".to_string(),
    };
    ProjectName::new(name).map_err(RepoctlError::diagnostic)
}

fn synthesize_workspaces(inventory: &SourceInventory) -> Vec<String> {
    let mut workspaces = Vec::new();
    if inventory.manifests.iter().any(|path| path == "Cargo.toml") {
        let is_workspace = inventory
            .manifests
            .iter()
            .any(|path| path.starts_with("crates/") || path.starts_with("apps/"));
        let command = if is_workspace { "rust" } else { "service" };
        workspaces.push(format!(
            "  - name: {command}\n    language: rust\n    toolchain: cargo\n    root: .\n    \
             manifest: Cargo.toml\n{}    target_dir: \"{{repo_root}}/target/rust\"\n",
            if inventory.manifests.iter().any(|path| path == "Cargo.lock") {
                "    lockfile: Cargo.lock\n"
            } else {
                ""
            },
        ));
    }
    for manifest in inventory
        .manifests
        .iter()
        .filter(|path| path.ends_with("package.json"))
    {
        let root = manifest.strip_suffix("/package.json").unwrap_or(".");
        let root = if root.is_empty() { "." } else { root };
        let name = node_workspace_name(root);
        let (toolchain, lockfile, cache_dir) = node_toolchain(inventory, root);
        workspaces.push(format!(
            "  - name: {name}\n    language: typescript\n    toolchain: {toolchain}\n    root: \
             {root}\n    manifest: {manifest}\n{lockfile}    cache_dir: \
             \"{{repo_root}}/{cache_dir}\"\n",
        ));
    }
    for manifest in inventory
        .manifests
        .iter()
        .filter(|path| path.ends_with("pyproject.toml"))
    {
        let root = manifest.strip_suffix("/pyproject.toml").unwrap_or(".");
        let name = if root == "." {
            "python".to_string()
        } else {
            format!("python-{}", root.replace('/', "-"))
        };
        workspaces.push(format!(
            "  - name: {name}\n    language: python\n    toolchain: uv\n    root: {root}\n    \
             manifest: {manifest}\n"
        ));
    }
    if inventory.manifests.iter().any(|path| path == "Pulumi.yaml") {
        workspaces.push(
            "  - name: infra\n    language: iac\n    root: .\n    manifest: Pulumi.yaml\n"
                .to_string(),
        );
    }
    workspaces
}

fn node_workspace_name(root: &str) -> String {
    match root {
        "infra" => "infra".to_string(),
        "." | "frontend" => "frontend".to_string(),
        value if value.contains("client") => "client-ts".to_string(),
        value => value.replace('/', "-"),
    }
}

fn node_toolchain(inventory: &SourceInventory, root: &str) -> (&'static str, String, &'static str) {
    let prefix = if root == "." { "" } else { root };
    let lock_path = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        }
    };
    if inventory.manifests.contains(&lock_path("pnpm-lock.yaml")) {
        (
            "pnpm",
            format!("    lockfile: {}\n", lock_path("pnpm-lock.yaml")),
            "target/pnpm",
        )
    } else if inventory.manifests.contains(&lock_path("yarn.lock")) {
        (
            "yarn",
            format!("    lockfile: {}\n", lock_path("yarn.lock")),
            "target/yarn",
        )
    } else if inventory.manifests.contains(&lock_path("bun.lock")) {
        (
            "bun",
            format!("    lockfile: {}\n", lock_path("bun.lock")),
            "target/bun",
        )
    } else {
        (
            "npm",
            format!("    lockfile: {}\n", lock_path("package-lock.json")),
            "target/npm",
        )
    }
}

#[derive(Debug, Deserialize)]
struct TaskWorkspace {
    name: String,
    language: String,
    manifest: String,
    #[serde(default)]
    toolchain: String,
}

fn synthesize_tasks(source_path: &Utf8Path, workspaces: &[String]) -> Result<String, RepoctlError> {
    let mut tasks = BTreeMap::<&str, Vec<String>>::new();
    for text in workspaces {
        let specs: Vec<TaskWorkspace> = serde_norway::from_str(text)
            .map_err(|error| RepoctlError::Internal(error.to_string()))?;
        for spec in specs {
            let commands = match spec.language.as_str() {
                "rust" => vec![
                    ("check", "cargo check".to_string()),
                    ("test", "cargo test".to_string()),
                    ("build", "cargo build".to_string()),
                ],
                "python" => vec![("check", "uv run python -m compileall -q .".to_string())],
                "typescript" => node_tasks(source_path, &spec)?,
                _ => Vec::new(),
            };
            for (task, command) in commands {
                tasks.entry(task).or_default().push(format!(
                    "    - workspace: {}\n      command: {command}\n",
                    spec.name
                ));
            }
        }
    }
    let mut output = String::new();
    for (task, commands) in tasks {
        let _ = writeln!(output, "  {task}:");
        output.push_str(&commands.concat());
    }
    Ok(output)
}

fn node_tasks(
    source: &Utf8Path,
    spec: &TaskWorkspace,
) -> Result<Vec<(&'static str, String)>, RepoctlError> {
    let manifest = source.join(&spec.manifest);
    let content =
        fs::read_to_string(&manifest).map_err(|error| RepoctlError::io(manifest.clone(), error))?;
    let package: Value = serde_json::from_str(&content)
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    let program = if spec.toolchain.is_empty() {
        "npm"
    } else {
        &spec.toolchain
    };
    let mut tasks = Vec::new();
    for task in ["check", "test", "build"] {
        if package
            .get("scripts")
            .and_then(|scripts| scripts.get(task))
            .and_then(Value::as_str)
            .is_some()
        {
            tasks.push((task, format!("{program} run {task}")));
        }
    }
    if !tasks.iter().any(|(task, _)| *task == "check")
        && manifest
            .parent()
            .is_some_and(|root| root.join("tsconfig.json").is_file())
    {
        let command = match program {
            "npm" => "npm exec -- tsc --noEmit".to_string(),
            "bun" => "bun x tsc --noEmit".to_string(),
            _ => format!("{program} exec tsc --noEmit"),
        };
        tasks.push(("check", command));
    }
    Ok(tasks)
}

fn synthesize_iac(inventory: &SourceInventory) -> Result<Option<String>, RepoctlError> {
    if !inventory
        .manifests
        .iter()
        .any(|path| path == "Pulumi.yaml" || path == "infra/Pulumi.yaml")
    {
        return Ok(None);
    }
    let root = if inventory
        .manifests
        .iter()
        .any(|path| path == "infra/Pulumi.yaml")
    {
        "infra"
    } else {
        "."
    };
    let stacks = inventory
        .manifests
        .iter()
        .filter_map(|path| {
            path.strip_prefix(&format!("{root}/Pulumi."))
                .or_else(|| path.strip_prefix("Pulumi."))
        })
        .filter_map(|path| path.strip_suffix(".yaml"))
        .collect::<Vec<_>>();
    let mut output = format!("iac:\n  root: {root}\n  provider: pulumi\n");
    if !stacks.is_empty() {
        output.push_str("  stacks:\n");
        for stack in stacks {
            writeln!(output, "    - {stack}").map_err(|source| {
                RepoctlError::Internal(format!("failed to render iac: {source}"))
            })?;
        }
    }
    Ok(Some(output))
}
