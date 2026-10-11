//! Project manifests, workspace commands and infrastructure synthesis.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use camino::Utf8Path;
use repoctl_core::{
    AdoptedSource, Diagnostic, OwnerHandle, ProcessCommand, ProjectKind, ProjectName,
    ProjectRelativePath, RepoRelativePath, RepoctlError, SourceInventory, WorkspaceName,
};
use serde::Deserialize;
use serde_json::Value;

use super::{
    node::{nearest_lockfile, node_install},
    placement::PlacementDecision,
    process_command,
    rust::workspace_roots,
    source_walk::read_text,
};

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
    let workspaces = synthesize_workspaces(source_path, inventory)?;
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

fn synthesize_workspaces(
    source: &Utf8Path,
    inventory: &SourceInventory,
) -> Result<Vec<String>, RepoctlError> {
    let mut workspaces = Vec::new();
    let mut names = BTreeSet::new();
    for manifest in workspace_roots(source, inventory)? {
        let root = manifest.strip_suffix("/Cargo.toml").unwrap_or(".");
        let base = if root == "." {
            "rust".to_string()
        } else {
            format!("rust-{}", root.replace('/', "-"))
        };
        let name = unique_workspace_name(base, "rust", &mut names);
        let lock = Utf8Path::new(&manifest).with_file_name("Cargo.lock");
        let lockfile = if inventory.manifests.iter().any(|path| path == lock.as_str()) {
            format!("    lockfile: {lock}\n")
        } else {
            String::new()
        };
        workspaces.push(format!(
            "  - name: {name}\n    language: rust\n    toolchain: cargo\n    root: {root}\n    \
             manifest: {manifest}\n{lockfile}    target_dir: \"{{repo_root}}/target/rust\"\n",
        ));
    }
    for manifest in inventory
        .manifests
        .iter()
        .filter(|path| path.ends_with("package.json"))
    {
        let root = manifest.strip_suffix("/package.json").unwrap_or(".");
        let root = if root.is_empty() { "." } else { root };
        let name = unique_workspace_name(node_workspace_name(root), "node", &mut names);
        let install = node_install(source, inventory, root)?;
        let toolchain = install.toolchain;
        let cache_dir = install.cache_dir;
        let lockfile = install
            .lockfile
            .map_or_else(String::new, |path| format!("    lockfile: {path}\n"));
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
        let name = unique_workspace_name(name, "python", &mut names);
        let lockfile = nearest_lockfile(inventory, root, &["uv.lock"])
            .map_or_else(String::new, |path| format!("    lockfile: {path}\n"));
        workspaces.push(format!(
            "  - name: {name}\n    language: python\n    toolchain: uv\n    root: {root}\n    \
             manifest: {manifest}\n{lockfile}"
        ));
    }
    if inventory.manifests.iter().any(|path| path == "Pulumi.yaml") {
        let name = unique_workspace_name("infra".to_string(), "iac", &mut names);
        workspaces.push(format!(
            "  - name: {name}\n    language: iac\n    root: .\n    manifest: Pulumi.yaml\n"
        ));
    }
    Ok(workspaces)
}

fn node_workspace_name(root: &str) -> String {
    match root {
        "infra" => "infra".to_string(),
        "." => "node".to_string(),
        "frontend" => "frontend".to_string(),
        value if value.contains("client") => "client-ts".to_string(),
        value => value.replace('/', "-"),
    }
}

fn unique_workspace_name(base: String, language: &str, names: &mut BTreeSet<String>) -> String {
    let base = if base.len() <= 100 && WorkspaceName::new(&base).is_ok() {
        base
    } else {
        language.to_string()
    };
    let mut name = base.clone();
    let mut suffix = 2_usize;
    while !names.insert(name.clone()) {
        name = format!("{base}-{suffix}");
        suffix = suffix.saturating_add(1);
    }
    name
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
                    ("check", "cargo check --workspace".to_string()),
                    ("test", "cargo test --workspace".to_string()),
                    ("build", "cargo build --workspace".to_string()),
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
    let content = read_text(&manifest)?;
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

pub(super) fn append_node_build_verification(
    destination: &Utf8Path,
    source: &AdoptedSource,
    commands: &mut Vec<ProcessCommand>,
) -> Result<(), RepoctlError> {
    for manifest in source
        .inventory
        .manifests
        .iter()
        .filter(|path| path.ends_with("package.json"))
    {
        let root = manifest.strip_suffix("/package.json").unwrap_or(".");
        let install = node_install(&source.source_path, &source.inventory, root)?;
        let workspace = TaskWorkspace {
            name: node_workspace_name(root),
            language: "typescript".to_string(),
            manifest: manifest.clone(),
            toolchain: install.toolchain.to_string(),
        };
        if node_tasks(&source.source_path, &workspace)?
            .iter()
            .any(|(task, _)| *task == "build")
        {
            let cwd = source
                .destination_path
                .join_project(&ProjectRelativePath::new(root).map_err(RepoctlError::diagnostic)?)
                .map_err(RepoctlError::diagnostic)?;
            commands.push(process_command(
                destination,
                &cwd,
                install.toolchain,
                ["run", "build"],
            ));
        }
    }
    Ok(())
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
