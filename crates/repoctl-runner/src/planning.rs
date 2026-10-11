//! Stable task planning, prerequisite resolution and toolchain environments.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use repoctl_core::{
    AffectedReport, Diagnostic, ProcessCommand, ProjectManifest, ProjectName, RepoSnapshot,
    RepoctlError, TaskCommand, TaskDependency, TaskName, TaskRunRequest, ToolchainAdapter,
    ToolchainEnvironmentInput, WorkspaceName, WorkspaceSpec,
};

pub(super) fn plan_task_commands(
    snapshot: &RepoSnapshot,
    request: &TaskRunRequest,
    affected: Option<&AffectedReport>,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
) -> Result<(Vec<ProcessCommand>, Vec<TaskDependency>), RepoctlError> {
    let affected_projects = affected.map(|report| {
        report
            .directly_affected
            .iter()
            .chain(report.transitively_affected.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
    });
    let affected_workspaces =
        affected.map(|report| report.workspaces.iter().cloned().collect::<BTreeSet<_>>());
    let mut state = TaskPlanState::default();
    for project in &snapshot.projects {
        if !request.projects.is_empty() && !request.projects.contains(&project.name) {
            continue;
        }
        if let Some(affected_projects) = &affected_projects
            && !affected_projects.contains(&project.name)
        {
            continue;
        }
        for (task, task_commands) in &project.tasks {
            if !request.tasks.is_empty() && !request.tasks.contains(task) {
                continue;
            }
            for task_command in task_commands {
                let workspace_id = format!("{}:{}", project.name, task_command.workspace);
                if !request.workspaces.is_empty() && !request.workspaces.contains(&workspace_id) {
                    continue;
                }
                if let Some(affected_workspaces) = &affected_workspaces
                    && !affected_workspaces.contains(&workspace_id)
                {
                    continue;
                }
                state.selected.insert(TaskDependency {
                    project: project.name.clone(),
                    workspace: task_command.workspace.clone(),
                    task: task.clone(),
                });
                append_task_command_with_dependencies(
                    snapshot,
                    project,
                    task,
                    task_command,
                    toolchain_adapters,
                    &mut state,
                )?;
            }
        }
    }
    Ok((state.commands, state.selected.into_iter().collect()))
}

#[derive(Debug, Default)]
struct TaskPlanState {
    commands: Vec<ProcessCommand>,
    selected: BTreeSet<TaskDependency>,
    seen: BTreeSet<(ProjectName, WorkspaceName, TaskName)>,
    stack: BTreeSet<(ProjectName, WorkspaceName, TaskName)>,
}

fn append_task_command_with_dependencies(
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
    task: &TaskName,
    task_command: &TaskCommand,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
    state: &mut TaskPlanState,
) -> Result<(), RepoctlError> {
    let key = (
        project.name.clone(),
        task_command.workspace.clone(),
        task.clone(),
    );
    if state.seen.contains(&key) {
        return Ok(());
    }
    if state.stack.len() >= 256 {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "task.dependency_depth",
            "task dependency depth exceeds 256",
        )));
    }
    if !state.stack.insert(key.clone()) {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error("task.dependency_cycle", "task prerequisite cycle detected")
                .with_project(project.name.as_str()),
        ));
    }
    let commands = project
        .tasks
        .get(task)
        .into_iter()
        .flatten()
        .filter(|command| command.workspace == task_command.workspace)
        .collect::<Vec<_>>();
    let dependencies = task_prerequisites(&commands, project)?;
    let mut prerequisites = Vec::new();
    for dependency in dependencies {
        prerequisites.push(format!(
            "{}:{}:{}",
            dependency.project, dependency.workspace, dependency.task
        ));
        append_task_dependency(snapshot, &dependency, toolchain_adapters, state)?;
    }
    for command in commands {
        let mut command =
            build_process_command(snapshot, project, task, command, toolchain_adapters)?;
        command.prerequisites.clone_from(&prerequisites);
        state.commands.push(command);
    }
    state.seen.insert(key.clone());
    state.stack.remove(&key);
    Ok(())
}

fn task_prerequisites(
    commands: &[&TaskCommand],
    project: &ProjectManifest,
) -> Result<BTreeSet<TaskDependency>, RepoctlError> {
    let mut dependencies = BTreeSet::new();
    for dependency in commands.iter().flat_map(|command| &command.depends_on) {
        dependencies.insert(dependency.clone());
        if dependencies.len() > 256 {
            return Err(RepoctlError::diagnostic(
                Diagnostic::error(
                    "task.too_many_prerequisites",
                    "task prerequisite union exceeds 256",
                )
                .with_project(project.name.as_str()),
            ));
        }
    }
    Ok(dependencies)
}

fn append_task_dependency(
    snapshot: &RepoSnapshot,
    dependency: &TaskDependency,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
    state: &mut TaskPlanState,
) -> Result<(), RepoctlError> {
    let project = snapshot.project(&dependency.project).ok_or_else(|| {
        RepoctlError::diagnostic(
            Diagnostic::error(
                "task.dependency_project_missing",
                format!(
                    "task prerequisite references missing project `{}`",
                    dependency.project
                ),
            )
            .with_project(dependency.project.as_str()),
        )
    })?;
    let task_commands = project.tasks.get(&dependency.task).ok_or_else(|| {
        RepoctlError::diagnostic(
            Diagnostic::error(
                "task.dependency_task_missing",
                format!(
                    "task prerequisite references missing task `{}`",
                    dependency.task
                ),
            )
            .with_project(project.name.as_str()),
        )
    })?;
    let task_command = task_commands
        .iter()
        .find(|command| command.workspace == dependency.workspace)
        .ok_or_else(|| {
            RepoctlError::diagnostic(
                Diagnostic::error(
                    "task.dependency_workspace_missing",
                    format!(
                        "task prerequisite references missing workspace `{}`",
                        dependency.workspace
                    ),
                )
                .with_project(project.name.as_str()),
            )
        })?;
    append_task_command_with_dependencies(
        snapshot,
        project,
        &dependency.task,
        task_command,
        toolchain_adapters,
        state,
    )
}

fn build_process_command(
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
    task: &TaskName,
    task_command: &TaskCommand,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
) -> Result<ProcessCommand, RepoctlError> {
    let workspace = project
        .workspaces
        .iter()
        .find(|workspace| workspace.name == task_command.workspace)
        .ok_or_else(|| {
            RepoctlError::diagnostic(
                Diagnostic::error(
                    "task.workspace_missing",
                    format!(
                        "task `{task}` references missing workspace `{}`",
                        task_command.workspace
                    ),
                )
                .with_project(project.name.as_str()),
            )
        })?;
    let cwd = project
        .path
        .join_project(&workspace.root)
        .map_err(RepoctlError::diagnostic)?;
    let absolute_cwd = Some(
        snapshot
            .root
            .absolute
            .join(cwd.as_str())
            .as_std_path()
            .to_path_buf(),
    );
    Ok(ProcessCommand {
        prerequisite_fingerprints: Vec::new(),
        execution: task_command.execution.clone(),
        prerequisites: Vec::new(),
        repo_root: Some(snapshot.root.absolute.as_std_path().to_path_buf()),
        project: Some(project.name.clone()),
        workspace: Some(workspace.name.clone()),
        task: Some(task.clone()),
        cwd,
        absolute_cwd,
        program: task_command.command.program.clone(),
        args: task_command.command.args.clone(),
        env: toolchain_env(workspace, toolchain_adapters)?,
    })
}

fn toolchain_env(
    workspace: &WorkspaceSpec,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
) -> Result<BTreeMap<String, String>, RepoctlError> {
    let mut env = BTreeMap::new();
    let Some(toolchain) = &workspace.toolchain else {
        return Ok(env);
    };
    for adapter in toolchain_adapters {
        if adapter.toolchain() == *toolchain {
            env.extend(adapter.environment(&ToolchainEnvironmentInput { workspace })?);
        }
    }
    Ok(env)
}
