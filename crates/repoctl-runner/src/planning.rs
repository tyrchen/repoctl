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
) -> Result<Vec<ProcessCommand>, RepoctlError> {
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
                append_task_command_with_dependencies(
                    snapshot,
                    project,
                    task,
                    task_command,
                    toolchain_adapters,
                    &mut state,
                );
            }
        }
    }
    state.commands.into_iter().collect()
}

#[derive(Debug, Default)]
struct TaskPlanState {
    commands: Vec<Result<ProcessCommand, RepoctlError>>,
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
) {
    let key = (
        project.name.clone(),
        task_command.workspace.clone(),
        task.clone(),
    );
    if state.seen.contains(&key) {
        return;
    }
    if state.stack.len() >= 256 {
        state
            .commands
            .push(Err(RepoctlError::diagnostic(Diagnostic::error(
                "task.dependency_depth",
                "task dependency depth exceeds 256",
            ))));
        return;
    }
    if !state.stack.insert(key.clone()) {
        state.commands.push(Err(RepoctlError::diagnostic(
            Diagnostic::error("task.dependency_cycle", "task prerequisite cycle detected")
                .with_project(project.name.as_str()),
        )));
        return;
    }
    let commands = project
        .tasks
        .get(task)
        .into_iter()
        .flatten()
        .filter(|command| command.workspace == task_command.workspace)
        .collect::<Vec<_>>();
    let mut prerequisites = BTreeSet::new();
    for command in &commands {
        for dependency in &command.depends_on {
            prerequisites.insert(format!(
                "{}:{}:{}",
                dependency.project, dependency.workspace, dependency.task
            ));
            append_task_dependency(snapshot, dependency, toolchain_adapters, state);
        }
    }
    for command in commands {
        state.commands.push(
            build_process_command(snapshot, project, task, command, toolchain_adapters).map(
                |mut command| {
                    command.prerequisites = prerequisites.iter().cloned().collect();
                    command
                },
            ),
        );
    }
    state.seen.insert(key.clone());
    state.stack.remove(&key);
}

fn append_task_dependency(
    snapshot: &RepoSnapshot,
    dependency: &TaskDependency,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
    state: &mut TaskPlanState,
) {
    let Some(project) = snapshot.project(&dependency.project) else {
        state.commands.push(Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "task.dependency_project_missing",
                format!(
                    "task prerequisite references missing project `{}`",
                    dependency.project
                ),
            )
            .with_project(dependency.project.as_str()),
        )));
        return;
    };
    let Some(task_commands) = project.tasks.get(&dependency.task) else {
        state.commands.push(Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "task.dependency_task_missing",
                format!(
                    "task prerequisite references missing task `{}`",
                    dependency.task
                ),
            )
            .with_project(project.name.as_str()),
        )));
        return;
    };
    for task_command in task_commands {
        if task_command.workspace != dependency.workspace {
            continue;
        }
        append_task_command_with_dependencies(
            snapshot,
            project,
            &dependency.task,
            task_command,
            toolchain_adapters,
            state,
        );
        return;
    }
    state.commands.push(Err(RepoctlError::diagnostic(
        Diagnostic::error(
            "task.dependency_workspace_missing",
            format!(
                "task prerequisite references missing workspace `{}`",
                dependency.workspace
            ),
        )
        .with_project(project.name.as_str()),
    )));
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
