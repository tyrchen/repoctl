//! CI setup metadata and explicit dependency installation for selected task closures.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read as _,
    path::Path,
    sync::Arc,
};

use repoctl_core::{
    ProcessCommand, ProcessOutput, ProcessRunner, RepoRelativePath, RepoSnapshot, RepoctlError,
    TaskDependency, TaskExecutionOptions, TaskRunPlan, Toolchain, WorkspaceInspectionInput,
    WorkspaceLanguage,
};
use repoctl_engine::LocalRustWorkspaceInspector;
use serde_json::{Value, json};

pub(super) fn entries(
    plan: &TaskRunPlan,
    snapshot: &RepoSnapshot,
) -> Result<Vec<Value>, RepoctlError> {
    let selected = plan.selected_tasks.iter().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for command in &plan.commands {
        if let (Some(project), Some(workspace), Some(task)) =
            (&command.project, &command.workspace, &command.task)
        {
            let id = TaskDependency {
                project: project.clone(),
                workspace: workspace.clone(),
                task: task.clone(),
            };
            if selected.contains(&id) && seen.insert(id) {
                entries.push(entry(command, &plan.commands, snapshot)?);
            }
        }
    }
    Ok(entries)
}

pub(super) fn entry(
    command: &ProcessCommand,
    commands: &[ProcessCommand],
    snapshot: &RepoSnapshot,
) -> Result<Value, RepoctlError> {
    let project = command
        .project
        .as_ref()
        .ok_or_else(|| RepoctlError::Internal("CI task project missing".to_string()))?;
    let workspace = command
        .workspace
        .as_ref()
        .ok_or_else(|| RepoctlError::Internal("CI task workspace missing".to_string()))?;
    let task = command
        .task
        .as_ref()
        .ok_or_else(|| RepoctlError::Internal("CI task name missing".to_string()))?;
    let by_id = commands
        .iter()
        .map(|c| (identity(c), c))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::from([identity(command)]);
    loop {
        let dependencies = selected
            .iter()
            .filter_map(|id| by_id.get(id))
            .flat_map(|c| c.prerequisites.iter().cloned())
            .collect::<Vec<_>>();
        let mut changed = false;
        for dependency in dependencies {
            changed |= selected.insert(dependency);
        }
        if !changed {
            break;
        }
    }
    let mut tools = BTreeSet::new();
    let mut needs_protoc = false;
    for id in selected {
        if let Some(command) = by_id.get(&id)
            && let Some(spec) = workspace_spec(command, snapshot)
        {
            let toolchain = spec.toolchain.clone().unwrap_or(match spec.language {
                WorkspaceLanguage::Rust => Toolchain::Cargo,
                WorkspaceLanguage::Python => Toolchain::Uv,
                WorkspaceLanguage::TypeScript => Toolchain::Npm,
                _ => Toolchain::Custom("external".to_string()),
            });
            tools.insert(toolchain.as_str().to_string());
            if let Some(project) = command
                .project
                .as_ref()
                .and_then(|name| snapshot.project(name))
                && spec.language == WorkspaceLanguage::Rust
            {
                needs_protoc |=
                    LocalRustWorkspaceInspector.requires_protoc(&WorkspaceInspectionInput {
                        root: &snapshot.root,
                        project,
                        workspace: spec,
                        projects: &snapshot.projects,
                    })?;
            }
        }
    }
    let needs_buf = snapshot
        .root
        .absolute
        .join(snapshot.repo_manifest.protos_root.as_str())
        .join("buf.yaml")
        .is_file();
    Ok(
        json!({"project":project,"workspace":workspace,"task":task,"cwd":command.cwd,"toolchains":tools,
        "needsNode": tools.iter().any(|t| matches!(t.as_str(),"npm"|"pnpm"|"yarn")),
        "needsBun":tools.contains("bun"),"needsPython":tools.contains("uv"),"needsProtoc":needs_protoc,"needsBuf":needs_buf}),
    )
}

pub(super) fn install(
    runner: &Arc<dyn ProcessRunner>,
    commands: &[ProcessCommand],
    snapshot: &RepoSnapshot,
) -> Result<Vec<ProcessOutput>, RepoctlError> {
    let mut seen = BTreeSet::new();
    let mut outputs = Vec::new();
    for command in commands {
        let Some(spec) = workspace_spec(command, snapshot) else {
            continue;
        };
        let setup = installation_cwd(command, snapshot, spec)?;
        let path = snapshot.root.join(&setup.cwd);
        let toolchain = spec.toolchain.clone().unwrap_or(match spec.language {
            WorkspaceLanguage::Rust => Toolchain::Cargo,
            WorkspaceLanguage::Python => Toolchain::Uv,
            WorkspaceLanguage::TypeScript => Toolchain::Npm,
            _ => Toolchain::Custom("external".to_string()),
        });
        if !seen.insert((setup.cwd.clone(), toolchain.as_str().to_string())) {
            continue;
        }
        let Some((program, args)) = dependency_command(&toolchain, path.as_std_path())? else {
            continue;
        };
        let setup = ProcessCommand {
            program: program.to_string(),
            args: args.into_iter().map(ToString::to_string).collect(),
            execution: TaskExecutionOptions::default(),
            prerequisites: Vec::new(),
            ..setup
        };
        let output = runner.run(&setup)?;
        let failed = output.status != 0;
        outputs.push(output);
        if failed {
            break;
        }
    }
    Ok(outputs)
}

fn installation_cwd(
    command: &ProcessCommand,
    snapshot: &RepoSnapshot,
    spec: &repoctl_core::WorkspaceSpec,
) -> Result<ProcessCommand, RepoctlError> {
    let mut setup = command.clone();
    if let Some(lockfile) = &spec.lockfile
        && let Some(project) = command
            .project
            .as_ref()
            .and_then(|name| snapshot.project(name))
    {
        let lockfile = project
            .path
            .join_project(lockfile)
            .map_err(RepoctlError::diagnostic)?;
        let lock_path = snapshot.root.join(&lockfile);
        if lock_path.is_file()
            && let Some(parent) = lock_path.parent()
        {
            let relative = parent
                .strip_prefix(&snapshot.root.absolute)
                .map_err(|error| RepoctlError::Environment(error.to_string()))?;
            setup.cwd =
                RepoRelativePath::new(relative.as_str()).map_err(RepoctlError::diagnostic)?;
            setup.absolute_cwd = Some(parent.as_std_path().to_path_buf());
        }
    }
    Ok(setup)
}

type InstallCommand = (&'static str, Vec<&'static str>);

fn dependency_command(
    toolchain: &Toolchain,
    path: &Path,
) -> Result<Option<InstallCommand>, RepoctlError> {
    let command = match toolchain {
        Toolchain::Cargo => (
            "cargo",
            if path.join("Cargo.lock").is_file() {
                vec!["fetch", "--locked"]
            } else {
                vec!["fetch"]
            },
        ),
        Toolchain::Npm => (
            "npm",
            if path.join("package-lock.json").is_file() {
                vec!["ci"]
            } else {
                vec!["install"]
            },
        ),
        Toolchain::Pnpm => (
            "pnpm",
            if path.join("pnpm-lock.yaml").is_file() {
                vec!["install", "--frozen-lockfile"]
            } else {
                vec!["install", "--no-frozen-lockfile"]
            },
        ),
        Toolchain::Yarn => (
            "yarn",
            if path.join("yarn.lock").is_file() {
                if read_prefix(&path.join("yarn.lock"))?.starts_with("# yarn lockfile v1")
                    || read_prefix(&path.join("yarn.lock"))?.contains("\n# yarn lockfile v1")
                {
                    vec!["install", "--frozen-lockfile"]
                } else {
                    vec!["install", "--immutable"]
                }
            } else {
                vec!["install"]
            },
        ),
        Toolchain::Bun => (
            "bun",
            if path.join("bun.lock").is_file() || path.join("bun.lockb").is_file() {
                vec!["install", "--frozen-lockfile"]
            } else {
                vec!["install"]
            },
        ),
        Toolchain::Uv => (
            "uv",
            if path.join("uv.lock").is_file() {
                vec!["sync", "--frozen"]
            } else {
                vec!["sync"]
            },
        ),
        Toolchain::Custom(_) => return Ok(None),
    };
    Ok(Some(command))
}

fn read_prefix(path: &Path) -> Result<String, RepoctlError> {
    let file = fs::File::open(path)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    let mut bytes = Vec::new();
    file.take(8192)
        .read_to_end(&mut bytes)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    String::from_utf8(bytes).map_err(|error| RepoctlError::Environment(error.to_string()))
}

fn identity(command: &ProcessCommand) -> String {
    format!(
        "{}:{}:{}",
        command.project.as_ref().map_or("", |v| v.as_str()),
        command.workspace.as_ref().map_or("", |v| v.as_str()),
        command.task.as_ref().map_or("", |v| v.as_str())
    )
}

fn workspace_spec<'a>(
    command: &ProcessCommand,
    snapshot: &'a RepoSnapshot,
) -> Option<&'a repoctl_core::WorkspaceSpec> {
    snapshot
        .project(command.project.as_ref()?)?
        .workspaces
        .iter()
        .find(|w| Some(&w.name) == command.workspace.as_ref())
}
