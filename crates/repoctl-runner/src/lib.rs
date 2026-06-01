#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
// Git and process adapters are synchronous CLI/facade operations for v0.2.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! Affected analysis, task runner, and CI matrix services.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    process::Command,
    sync::Arc,
};

use globset::Glob;
use repoctl_core::{
    AffectedReason, AffectedReport, AffectedRequest, AiContext, AiContextRequest, CiMatrixReport,
    CiMatrixRequest, CodegenCheckReport, CodegenCheckRequest, Diagnostic, EdgeKind,
    IacFacadeReport, IacFacadeRequest, IacProvider, PrSummary, PrSummaryRequest, ProcessCommand,
    ProcessOutput, ProcessRunner, ProjectManifest, ProjectName, ProtoFacadeReport,
    ProtoFacadeRequest, ProtoOperation, RepoRelativePath, RepoSnapshot, RepoctlError,
    TaskCommandOutput, TaskName, TaskRunPlan, TaskRunReport, TaskRunRequest, ToolchainAdapter,
    ToolchainEnvironmentInput, WorkspaceLanguage,
};
use repoctl_engine::RepoctlEngine;
use serde_json::json;

/// Provides changed files for a git range.
pub trait GitProvider: Send + Sync {
    /// Returns changed repo-relative files between two refs.
    fn changed_files(
        &self,
        repo: Option<&std::path::Path>,
        base: &str,
        head: &str,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError>;
}

/// Local git provider backed by `git diff --name-only`.
#[derive(Clone, Debug, Default)]
pub struct LocalGitProvider;

impl GitProvider for LocalGitProvider {
    fn changed_files(
        &self,
        repo: Option<&std::path::Path>,
        base: &str,
        head: &str,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        let mut command = Command::new("git");
        command.arg("diff").arg("--name-only").arg(base).arg(head);
        if let Some(repo) = repo {
            command.current_dir(repo);
        }
        let output = command.output().map_err(|source| {
            RepoctlError::Environment(format!("failed to execute git diff: {source}"))
        })?;
        if !output.status.success() {
            return Err(RepoctlError::Environment(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| RepoRelativePath::new(line.to_string()).map_err(RepoctlError::diagnostic))
            .collect()
    }
}

/// Local process runner using argv form.
#[derive(Clone, Debug, Default)]
pub struct LocalProcessRunner;

impl ProcessRunner for LocalProcessRunner {
    fn run(&self, command: &ProcessCommand) -> Result<ProcessOutput, RepoctlError> {
        let mut process = Command::new(&command.program);
        process.args(&command.args);
        process.envs(&command.env);
        if let Some(absolute_cwd) = &command.absolute_cwd {
            process.current_dir(absolute_cwd);
        } else {
            process.current_dir(command.cwd.as_str());
        }
        let output = process.output().map_err(|source| {
            RepoctlError::Environment(format!(
                "failed to execute `{}` in `{}`: {source}",
                command.program, command.cwd
            ))
        })?;
        Ok(ProcessOutput {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// Rust task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct RustToolchainAdapter;

impl ToolchainAdapter for RustToolchainAdapter {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::Rust
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(target_dir) = &input.workspace.target_dir {
            env.insert("CARGO_TARGET_DIR".to_string(), target_dir.to_string());
        }
        Ok(env)
    }
}

/// Bun task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct BunToolchainAdapter;

impl ToolchainAdapter for BunToolchainAdapter {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::TypeScript
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(cache_dir) = &input.workspace.cache_dir {
            env.insert("BUN_INSTALL_CACHE_DIR".to_string(), cache_dir.to_string());
        }
        Ok(env)
    }
}

/// uv task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct UvToolchainAdapter;

impl ToolchainAdapter for UvToolchainAdapter {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::Python
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(cache_dir) = &input.workspace.cache_dir {
            env.insert("UV_CACHE_DIR".to_string(), cache_dir.to_string());
        }
        Ok(env)
    }
}

/// Runner service for affected analysis, task execution, and CI matrix generation.
#[derive(Clone)]
pub struct RunnerService {
    engine: RepoctlEngine,
    git: Arc<dyn GitProvider>,
    process_runner: Arc<dyn ProcessRunner>,
    toolchain_adapters: Vec<Arc<dyn ToolchainAdapter>>,
}

impl std::fmt::Debug for RunnerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerService").finish_non_exhaustive()
    }
}

impl Default for RunnerService {
    fn default() -> Self {
        Self {
            engine: RepoctlEngine::with_default_adapters(),
            git: Arc::new(LocalGitProvider),
            process_runner: Arc::new(LocalProcessRunner),
            toolchain_adapters: default_toolchain_adapters(),
        }
    }
}

impl RunnerService {
    /// Creates a service with default local adapters.
    pub fn with_default_adapters() -> Self {
        Self::default()
    }

    /// Creates a service with explicit adapters.
    pub fn new(
        engine: RepoctlEngine,
        git: Arc<dyn GitProvider>,
        process_runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        Self {
            engine,
            git,
            process_runner,
            toolchain_adapters: default_toolchain_adapters(),
        }
    }

    /// Creates a service with explicit infrastructure and toolchain adapters.
    pub fn new_with_adapters(
        engine: RepoctlEngine,
        git: Arc<dyn GitProvider>,
        process_runner: Arc<dyn ProcessRunner>,
        toolchain_adapters: Vec<Arc<dyn ToolchainAdapter>>,
    ) -> Self {
        Self {
            engine,
            git,
            process_runner,
            toolchain_adapters,
        }
    }

    /// Computes affected projects, workspaces, tasks, risks, and reasons.
    pub fn affected(&self, request: &AffectedRequest) -> Result<AffectedReport, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let changed_files = self.changed_files(
            request.repo.as_deref(),
            request.base.as_deref(),
            request.head.as_deref(),
            &request.changed_files,
        )?;
        Ok(compute_affected(&snapshot, &changed_files, &request.tasks))
    }

    /// Plans tasks without executing them.
    pub fn plan_tasks(&self, request: &TaskRunRequest) -> Result<TaskRunPlan, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let affected = if request.affected {
            let changed_files = self.changed_files(
                request.repo.as_deref(),
                request.base.as_deref(),
                request.head.as_deref(),
                &request.changed_files,
            )?;
            Some(compute_affected(&snapshot, &changed_files, &request.tasks))
        } else {
            None
        };
        let commands = plan_task_commands(
            &snapshot,
            request,
            affected.as_ref(),
            &self.toolchain_adapters,
        )?;
        Ok(TaskRunPlan {
            commands,
            concurrency: std::num::NonZeroUsize::new(
                request.concurrency.map_or(1, std::num::NonZeroU32::get) as usize,
            )
            .ok_or_else(|| RepoctlError::Internal("invalid zero concurrency".to_string()))?,
        })
    }

    /// Runs selected tasks.
    pub fn run_tasks(&self, request: &TaskRunRequest) -> Result<TaskRunReport, RepoctlError> {
        let plan = self.plan_tasks(request)?;
        if request.dry_run {
            return Ok(TaskRunReport {
                commands: plan.commands,
                outputs: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        let mut diagnostics = Vec::new();
        let mut outputs = Vec::new();
        for (command, output) in
            run_process_commands(&self.process_runner, &plan.commands, plan.concurrency.get())?
        {
            if output.status != 0 {
                diagnostics.push(
                    Diagnostic::error(
                        "task.failed",
                        format!(
                            "task `{}` failed with status {}",
                            command.task.as_ref().map_or("<unknown>", TaskName::as_str),
                            output.status
                        ),
                    )
                    .with_path(command.cwd.as_str()),
                );
            }
            if let (Some(project), Some(workspace), Some(task)) =
                (&command.project, &command.workspace, &command.task)
            {
                outputs.push(TaskCommandOutput {
                    project: project.clone(),
                    workspace: workspace.clone(),
                    task: task.clone(),
                    output,
                });
            }
        }
        Ok(TaskRunReport {
            commands: plan.commands,
            outputs,
            diagnostics,
        })
    }

    /// Builds a CI matrix from affected task commands.
    pub fn ci_matrix(&self, request: &CiMatrixRequest) -> Result<CiMatrixReport, RepoctlError> {
        let task_request = TaskRunRequest {
            repo: request.repo.clone(),
            tasks: request.tasks.clone(),
            projects: Vec::new(),
            workspaces: Vec::new(),
            affected: true,
            changed_files: request.changed_files.clone(),
            base: request.base.clone(),
            head: request.head.clone(),
            concurrency: None,
            dry_run: true,
        };
        let plan = self.plan_tasks(&task_request)?;
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for command in plan.commands {
            let project = command
                .project
                .map_or_else(String::new, |value| value.to_string());
            let workspace = command
                .workspace
                .map_or_else(String::new, |value| value.to_string());
            let task = command
                .task
                .map_or_else(String::new, |value| value.to_string());
            if seen.insert((project.clone(), workspace.clone(), task.clone())) {
                entries.push(json!({
                    "project": project,
                    "workspace": workspace,
                    "task": task,
                }));
            }
        }
        let github_actions = if entries.is_empty() {
            json!({ "include": [] })
        } else {
            json!({ "include": entries.clone() })
        };
        Ok(CiMatrixReport {
            entries,
            github_actions,
        })
    }

    /// Handles proto owner, consumer, and check requests.
    pub fn proto(&self, request: &ProtoFacadeRequest) -> Result<ProtoFacadeReport, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        match request.operation {
            ProtoOperation::Owners => {
                let selector = required_selector(request)?;
                Ok(ProtoFacadeReport {
                    owners: proto_projects(&snapshot, selector, ProtoMatchKind::Owner)?,
                    consumers: Vec::new(),
                    commands: Vec::new(),
                    diagnostics: Vec::new(),
                })
            }
            ProtoOperation::Consumers => {
                let selector = required_selector(request)?;
                Ok(ProtoFacadeReport {
                    owners: Vec::new(),
                    consumers: proto_projects(&snapshot, selector, ProtoMatchKind::Consumer)?,
                    commands: Vec::new(),
                    diagnostics: Vec::new(),
                })
            }
            ProtoOperation::Check => {
                let changed_files = self.changed_files(
                    request.repo.as_deref(),
                    request.base.as_deref(),
                    request.head.as_deref(),
                    &request.changed_files,
                )?;
                let diagnostics = self.engine.policies().evaluate(&snapshot, &changed_files)?;
                let commands = proto_check_commands(&snapshot);
                Ok(ProtoFacadeReport {
                    owners: Vec::new(),
                    consumers: Vec::new(),
                    commands,
                    diagnostics,
                })
            }
        }
    }

    /// Checks generated-code direct edits.
    pub fn codegen_check(
        &self,
        request: &CodegenCheckRequest,
    ) -> Result<CodegenCheckReport, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let changed_files = self.changed_files(
            request.repo.as_deref(),
            request.base.as_deref(),
            request.head.as_deref(),
            &request.changed_files,
        )?;
        let diagnostics = self
            .engine
            .policies()
            .evaluate(&snapshot, &changed_files)?
            .into_iter()
            .filter(|diagnostic| diagnostic.code.as_ref() == "policy.generated_code_readonly")
            .collect();
        Ok(CodegenCheckReport { diagnostics })
    }

    /// Builds AI context for one project.
    pub fn ai_context(&self, request: &AiContextRequest) -> Result<AiContext, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.name == request.project)
            .ok_or_else(|| {
                RepoctlError::diagnostic(
                    Diagnostic::error(
                        "context.project.not_found",
                        format!("project `{}` was not found", request.project),
                    )
                    .with_project(request.project.as_str()),
                )
            })?;
        Ok(AiContext {
            payload: project_ai_context(&snapshot, project, &request.audience),
        })
    }

    /// Builds a Markdown and JSON PR impact summary.
    pub fn pr_summary(&self, request: &PrSummaryRequest) -> Result<PrSummary, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let changed_files = self.changed_files(
            request.repo.as_deref(),
            request.base.as_deref(),
            request.head.as_deref(),
            &request.changed_files,
        )?;
        let affected = compute_affected(&snapshot, &changed_files, &[]);
        let diagnostics = self.engine.policies().evaluate(&snapshot, &changed_files)?;
        Ok(render_pr_summary(
            &snapshot,
            &changed_files,
            &affected,
            &diagnostics,
        ))
    }

    /// Plans `IaC` provider commands.
    pub fn iac_plan(&self, request: &IacFacadeRequest) -> Result<IacFacadeReport, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let changed_files = self.changed_files(
            request.repo.as_deref(),
            request.base.as_deref(),
            request.head.as_deref(),
            &request.changed_files,
        )?;
        let targets = iac_targets(&snapshot, request, &changed_files)?;
        let commands = targets
            .iter()
            .map(|target| iac_plan_command(&snapshot, target))
            .collect::<Result<Vec<_>, _>>()?;
        let risk_flags = iac_risk_flags(&targets, &changed_files);
        Ok(IacFacadeReport {
            commands,
            risk_flags,
            diagnostics: Vec::new(),
        })
    }

    fn changed_files(
        &self,
        repo: Option<&std::path::Path>,
        base: Option<&str>,
        head: Option<&str>,
        explicit: &[RepoRelativePath],
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        if !explicit.is_empty() {
            return Ok(explicit.to_vec());
        }
        match (base, head) {
            (Some(base), Some(head)) => self.git.changed_files(repo, base, head),
            _ => Ok(Vec::new()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtoMatchKind {
    Owner,
    Consumer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IacTarget<'a> {
    project: Option<&'a ProjectManifest>,
    root: RepoRelativePath,
    provider: IacProvider,
    stack: String,
    core: bool,
}

trait ProtoToolchainAdapter {
    fn check_commands(&self, snapshot: &RepoSnapshot) -> Vec<ProcessCommand>;
}

#[derive(Clone, Debug, Default)]
struct BufProtoToolchainAdapter;

impl ProtoToolchainAdapter for BufProtoToolchainAdapter {
    fn check_commands(&self, snapshot: &RepoSnapshot) -> Vec<ProcessCommand> {
        let buf_yaml = snapshot
            .root
            .absolute
            .join(snapshot.repo_manifest.protos_root.as_str())
            .join("buf.yaml");
        if !buf_yaml.is_file() {
            return Vec::new();
        }
        vec![ProcessCommand {
            cwd: snapshot.repo_manifest.protos_root.clone(),
            absolute_cwd: Some(buf_yaml.parent().map_or_else(
                || snapshot.root.absolute.as_std_path().to_path_buf(),
                |path| path.as_std_path().to_path_buf(),
            )),
            program: "buf".to_string(),
            args: vec!["lint".to_string()],
            ..ProcessCommand::default()
        }]
    }
}

trait IacProviderAdapter {
    fn plan_command(
        &self,
        snapshot: &RepoSnapshot,
        target: &IacTarget<'_>,
    ) -> Result<ProcessCommand, RepoctlError>;
}

#[derive(Clone, Debug, Default)]
struct PulumiIacProviderAdapter;

impl IacProviderAdapter for PulumiIacProviderAdapter {
    fn plan_command(
        &self,
        snapshot: &RepoSnapshot,
        target: &IacTarget<'_>,
    ) -> Result<ProcessCommand, RepoctlError> {
        Ok(iac_command(
            snapshot,
            target,
            "pulumi",
            vec![
                "preview".to_string(),
                "--stack".to_string(),
                target.stack.clone(),
            ],
        ))
    }
}

#[derive(Clone, Debug, Default)]
struct TerraformIacProviderAdapter;

impl IacProviderAdapter for TerraformIacProviderAdapter {
    fn plan_command(
        &self,
        snapshot: &RepoSnapshot,
        target: &IacTarget<'_>,
    ) -> Result<ProcessCommand, RepoctlError> {
        Ok(iac_command(
            snapshot,
            target,
            "terraform",
            vec![
                "plan".to_string(),
                "-var".to_string(),
                format!("env={}", target.stack),
            ],
        ))
    }
}

#[derive(Clone, Debug, Default)]
struct OpenTofuIacProviderAdapter;

impl IacProviderAdapter for OpenTofuIacProviderAdapter {
    fn plan_command(
        &self,
        snapshot: &RepoSnapshot,
        target: &IacTarget<'_>,
    ) -> Result<ProcessCommand, RepoctlError> {
        Ok(iac_command(
            snapshot,
            target,
            "tofu",
            vec![
                "plan".to_string(),
                "-var".to_string(),
                format!("env={}", target.stack),
            ],
        ))
    }
}

fn required_selector(request: &ProtoFacadeRequest) -> Result<&str, RepoctlError> {
    request.selector.as_deref().ok_or_else(|| {
        RepoctlError::diagnostic(Diagnostic::error(
            "proto.selector.required",
            "proto owners and consumers require a path or package selector",
        ))
    })
}

fn proto_projects(
    snapshot: &RepoSnapshot,
    selector: &str,
    kind: ProtoMatchKind,
) -> Result<Vec<ProjectName>, RepoctlError> {
    let normalized = normalize_proto_selector(selector)?;
    let mut projects = snapshot
        .projects
        .iter()
        .filter(|project| {
            let patterns = match kind {
                ProtoMatchKind::Owner => &project.protos.owns,
                ProtoMatchKind::Consumer => &project.protos.consumes,
            };
            patterns
                .iter()
                .any(|pattern| glob_matches(pattern.as_str(), &normalized))
        })
        .map(|project| project.name.clone())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    Ok(projects)
}

fn normalize_proto_selector(selector: &str) -> Result<String, RepoctlError> {
    if selector.starts_with("protos/") {
        return Ok(selector.to_string());
    }
    if selector
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
    {
        return Ok(format!("protos/{}", selector.replace('.', "/")));
    }
    Err(RepoctlError::diagnostic(Diagnostic::error(
        "proto.selector.invalid",
        "proto selector must be a protos/ path or proto package name",
    )))
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher())
        .is_ok_and(|matcher| matcher.is_match(path))
}

fn proto_check_commands(snapshot: &RepoSnapshot) -> Vec<ProcessCommand> {
    BufProtoToolchainAdapter.check_commands(snapshot)
}

fn project_ai_context(
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
    audience: &str,
) -> serde_json::Value {
    let dependencies = snapshot
        .graph
        .edges
        .iter()
        .filter(|edge| edge.from == project.node_id())
        .map(|edge| {
            json!({
                "to": edge.to,
                "kind": format!("{:?}", edge.kind),
                "evidence": edge.evidence,
            })
        })
        .collect::<Vec<_>>();
    let commands = project
        .tasks
        .iter()
        .map(|(task, commands)| {
            json!({
                "task": task,
                "commands": commands,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "audience": audience,
        "repo": snapshot.repo_manifest.name,
        "project": project.name,
        "kind": project.kind,
        "owners": project.owners,
        "path": project.path,
        "workspaces": project.workspaces,
        "editable": project.ai.editable,
        "doNotEdit": project.ai.do_not_edit,
        "docs": project.ai.docs,
        "dependencies": dependencies,
        "commands": commands,
        "proto": {
            "owns": project.protos.owns,
            "consumes": project.protos.consumes,
        },
        "iac": project.iac,
        "policies": snapshot.repo_manifest.policies,
    })
}

fn render_pr_summary(
    snapshot: &RepoSnapshot,
    changed_files: &[RepoRelativePath],
    affected: &AffectedReport,
    diagnostics: &[Diagnostic],
) -> PrSummary {
    let mut markdown = String::new();
    markdown.push_str("# PR Impact Summary\n\n");
    markdown.push_str("## Changed Files\n");
    for file in changed_files {
        let _ = writeln!(markdown, "- `{file}`");
    }
    markdown.push_str("\n## Affected Projects\n");
    for project in affected
        .directly_affected
        .iter()
        .chain(affected.transitively_affected.iter())
    {
        let _ = writeln!(markdown, "- `{project}`");
    }
    markdown.push_str("\n## Affected Tasks\n");
    for task in &affected.tasks {
        let _ = writeln!(markdown, "- `{task}`");
    }
    markdown.push_str("\n## Risk Flags\n");
    for risk in &affected.risk_flags {
        let _ = writeln!(markdown, "- `{risk}`");
    }
    for diagnostic in diagnostics {
        let _ = writeln!(markdown, "- `{}`: {}", diagnostic.code, diagnostic.message);
    }
    markdown.push_str("\n## Suggested Reviewers\n");
    for reviewer in &affected.suggested_reviewers {
        let _ = writeln!(markdown, "- `{reviewer}`");
    }
    markdown.push_str("\n## Suggested Commands\n");
    markdown.push_str("- `repoctl graph validate`\n");
    markdown.push_str("- `repoctl affected`\n");
    let impact = json!({
        "repo": snapshot.repo_manifest.name,
        "changedFiles": changed_files,
        "affected": affected,
        "diagnostics": diagnostics,
    });
    PrSummary { markdown, impact }
}

fn iac_targets<'a>(
    snapshot: &'a RepoSnapshot,
    request: &IacFacadeRequest,
    changed_files: &[RepoRelativePath],
) -> Result<Vec<IacTarget<'a>>, RepoctlError> {
    let mut targets = Vec::new();
    if request.core {
        targets.push(IacTarget {
            project: None,
            root: snapshot.repo_manifest.core_infra_root.clone(),
            provider: IacProvider::Pulumi,
            stack: request.env.clone().unwrap_or_else(|| "dev".to_string()),
            core: true,
        });
        return Ok(targets);
    }
    for project in &snapshot.projects {
        if let Some(selected) = &request.project
            && selected != &project.name
        {
            continue;
        }
        if request.affected
            && !changed_files
                .iter()
                .any(|changed_file| project.contains_path(changed_file))
        {
            continue;
        }
        let Some(iac) = &project.iac else {
            continue;
        };
        let stacks = if let Some(env) = &request.env {
            vec![env.clone()]
        } else if iac.stacks.is_empty() {
            vec!["default".to_string()]
        } else {
            iac.stacks.clone()
        };
        let iac_root = project
            .path
            .join_project(&iac.root)
            .map_err(RepoctlError::diagnostic)?;
        targets.extend(stacks.into_iter().map(|stack| IacTarget {
            project: Some(project),
            root: iac_root.clone(),
            provider: iac.provider.clone(),
            stack,
            core: false,
        }));
    }
    if targets.is_empty() && (request.project.is_some() || request.core) {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "iac.target.not_found",
            "no matching IaC target was found",
        )));
    }
    Ok(targets)
}

fn iac_plan_command(
    snapshot: &RepoSnapshot,
    target: &IacTarget<'_>,
) -> Result<ProcessCommand, RepoctlError> {
    match target.provider {
        IacProvider::Pulumi => PulumiIacProviderAdapter.plan_command(snapshot, target),
        IacProvider::Terraform => TerraformIacProviderAdapter.plan_command(snapshot, target),
        IacProvider::OpenTofu => OpenTofuIacProviderAdapter.plan_command(snapshot, target),
    }
}

fn iac_command(
    snapshot: &RepoSnapshot,
    target: &IacTarget<'_>,
    program: &str,
    args: Vec<String>,
) -> ProcessCommand {
    ProcessCommand {
        project: target.project.map(|project| project.name.clone()),
        workspace: None,
        task: None,
        cwd: target.root.clone(),
        absolute_cwd: Some(
            snapshot
                .root
                .absolute
                .join(target.root.as_str())
                .as_std_path()
                .to_path_buf(),
        ),
        program: program.to_string(),
        args,
        env: BTreeMap::new(),
    }
}

fn iac_risk_flags(targets: &[IacTarget<'_>], changed_files: &[RepoRelativePath]) -> Vec<String> {
    let mut flags = BTreeSet::new();
    for target in targets {
        if target.core {
            flags.insert("core-infra".to_string());
        }
        if target.stack == "prod" {
            flags.insert("prod-iac".to_string());
        }
    }
    for file in changed_files {
        if file.as_str().starts_with("core-infra/") {
            flags.insert("core-infra".to_string());
        }
        if file.as_str().contains("/iac/stacks/prod") {
            flags.insert("prod-iac".to_string());
        }
    }
    flags.into_iter().collect()
}

fn compute_affected(
    snapshot: &RepoSnapshot,
    changed_files: &[RepoRelativePath],
    requested_tasks: &[TaskName],
) -> AffectedReport {
    let mut direct = BTreeSet::new();
    let mut transitive = BTreeSet::new();
    let mut direct_workspaces = BTreeSet::new();
    let mut reasons = Vec::new();
    let mut risk_flags = Vec::new();
    let mut reviewers = BTreeSet::new();
    for changed_file in changed_files {
        if changed_file.as_str() == "repo.yaml"
            || changed_file.as_str().starts_with("templates/")
            || changed_file.as_str().starts_with(".agents/skills/")
            || changed_file.as_str().starts_with(".claude/skills/")
            || changed_file.as_str().starts_with("crates/")
            || changed_file.as_str().starts_with("apps/repoctl-cli/")
        {
            for project in &snapshot.projects {
                direct.insert(project.name.clone());
                reasons.push(reason(changed_file, &project.name, "repo-wide change"));
            }
            continue;
        }
        if changed_file.as_str().starts_with("core-infra/") {
            risk_flags.push("core-infra".to_string());
        }
        if changed_file.as_str().contains("/iac/stacks/prod") {
            risk_flags.push("prod-iac".to_string());
        }
        for project in &snapshot.projects {
            if project.contains_path(changed_file) {
                direct.insert(project.name.clone());
                collect_matching_workspaces(project, changed_file, &mut direct_workspaces);
                reasons.push(reason(changed_file, &project.name, "project file changed"));
                reviewers.extend(project.owners.iter().cloned());
            }
            if proto_change_affects_project(project, changed_file) {
                direct.insert(project.name.clone());
                reasons.push(reason(
                    changed_file,
                    &project.name,
                    "proto ownership or consumption",
                ));
                reviewers.extend(project.owners.iter().cloned());
            }
        }
    }
    propagate_reverse_dependencies(snapshot, &direct, &mut transitive, &mut reasons);
    let all_projects = direct
        .iter()
        .chain(transitive.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    let workspaces = affected_workspaces(snapshot, &all_projects, &direct_workspaces);
    let tasks = affected_tasks(snapshot, &all_projects, &workspaces, requested_tasks);
    AffectedReport {
        directly_affected: direct.into_iter().collect(),
        transitively_affected: transitive.into_iter().collect(),
        workspaces,
        tasks,
        risk_flags: unique_strings(risk_flags),
        reasons,
        suggested_reviewers: reviewers.into_iter().collect(),
        diagnostics: Vec::new(),
    }
}

fn reason(path: &RepoRelativePath, project: &ProjectName, message: &str) -> AffectedReason {
    AffectedReason {
        source: path.to_string(),
        target: project.to_string(),
        reason: message.to_string(),
    }
}

fn proto_change_affects_project(
    project: &ProjectManifest,
    changed_file: &RepoRelativePath,
) -> bool {
    changed_file.as_str().starts_with("protos/")
        && project
            .protos
            .owns
            .iter()
            .chain(project.protos.consumes.iter())
            .any(|pattern| glob_match(pattern.as_str(), changed_file.as_str()))
}

fn glob_match(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        path.starts_with(prefix)
    } else {
        pattern == path
    }
}

fn propagate_reverse_dependencies(
    snapshot: &RepoSnapshot,
    direct: &BTreeSet<ProjectName>,
    transitive: &mut BTreeSet<ProjectName>,
    reasons: &mut Vec<AffectedReason>,
) {
    let mut changed = true;
    while changed {
        changed = false;
        for edge in &snapshot.graph.edges {
            if !matches!(
                edge.kind,
                EdgeKind::DependsOnProject
                    | EdgeKind::UsesFrameworkFacade
                    | EdgeKind::UsesFoundationClient
                    | EdgeKind::ConsumesProto
            ) {
                continue;
            }
            let Some(target_project) = project_from_node(&edge.to) else {
                continue;
            };
            let Ok(target) = ProjectName::new(target_project) else {
                continue;
            };
            if !direct.contains(&target) && !transitive.contains(&target) {
                continue;
            }
            let Some(source_project) = project_from_node(&edge.from) else {
                continue;
            };
            let Ok(source) = ProjectName::new(source_project) else {
                continue;
            };
            if direct.contains(&source) || transitive.contains(&source) {
                continue;
            }
            reasons.push(AffectedReason {
                source: target.to_string(),
                target: source.to_string(),
                reason: "reverse dependency propagation".to_string(),
            });
            transitive.insert(source);
            changed = true;
        }
    }
}

fn project_from_node(node: &str) -> Option<String> {
    node.strip_prefix("project:").map(ToString::to_string)
}

fn collect_matching_workspaces(
    project: &ProjectManifest,
    changed_file: &RepoRelativePath,
    direct_workspaces: &mut BTreeSet<String>,
) {
    let mut matched = false;
    for workspace in &project.workspaces {
        if project
            .path
            .join_project(&workspace.root)
            .is_ok_and(|root| changed_file.starts_with(&root))
        {
            direct_workspaces.insert(format!("{}:{}", project.name, workspace.name));
            matched = true;
        }
    }
    if !matched {
        for workspace in &project.workspaces {
            direct_workspaces.insert(format!("{}:{}", project.name, workspace.name));
        }
    }
}

fn affected_workspaces(
    snapshot: &RepoSnapshot,
    projects: &BTreeSet<ProjectName>,
    direct_workspaces: &BTreeSet<String>,
) -> Vec<String> {
    let mut workspaces = Vec::new();
    for project in &snapshot.projects {
        if !projects.contains(&project.name) {
            continue;
        }
        for workspace in &project.workspaces {
            let id = format!("{}:{}", project.name, workspace.name);
            let has_specific_workspaces = direct_workspaces
                .iter()
                .any(|workspace| workspace.starts_with(&format!("{}:", project.name)));
            if direct_workspaces.is_empty()
                || direct_workspaces.contains(&id)
                || !has_specific_workspaces
            {
                workspaces.push(id);
            }
        }
    }
    workspaces
}

fn affected_tasks(
    snapshot: &RepoSnapshot,
    projects: &BTreeSet<ProjectName>,
    workspaces: &[String],
    requested_tasks: &[TaskName],
) -> Vec<String> {
    let workspace_set = workspaces.iter().cloned().collect::<BTreeSet<_>>();
    let mut tasks = Vec::new();
    for project in &snapshot.projects {
        if !projects.contains(&project.name) {
            continue;
        }
        for (task, commands) in &project.tasks {
            if !requested_tasks.is_empty() && !requested_tasks.contains(task) {
                continue;
            }
            for command in commands {
                let workspace_id = format!("{}:{}", project.name, command.workspace);
                if !workspace_set.is_empty() && !workspace_set.contains(&workspace_id) {
                    continue;
                }
                tasks.push(format!("{}:{}:{}", project.name, command.workspace, task));
            }
        }
    }
    tasks
}

fn plan_task_commands(
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
    let mut commands = Vec::new();
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
                commands.push(ProcessCommand {
                    project: Some(project.name.clone()),
                    workspace: Some(workspace.name.clone()),
                    task: Some(task.clone()),
                    cwd,
                    absolute_cwd,
                    program: task_command.command.program.clone(),
                    args: task_command.command.args.clone(),
                    env: toolchain_env(workspace, toolchain_adapters)?,
                });
            }
        }
    }
    Ok(commands)
}

fn toolchain_env(
    workspace: &repoctl_core::WorkspaceSpec,
    toolchain_adapters: &[Arc<dyn ToolchainAdapter>],
) -> Result<BTreeMap<String, String>, RepoctlError> {
    let mut env = BTreeMap::new();
    for adapter in toolchain_adapters {
        if adapter.language() == workspace.language {
            env.extend(adapter.environment(&ToolchainEnvironmentInput { workspace })?);
        }
    }
    Ok(env)
}

fn default_toolchain_adapters() -> Vec<Arc<dyn ToolchainAdapter>> {
    vec![
        Arc::new(RustToolchainAdapter),
        Arc::new(BunToolchainAdapter),
        Arc::new(UvToolchainAdapter),
    ]
}

fn run_process_commands(
    runner: &Arc<dyn ProcessRunner>,
    commands: &[ProcessCommand],
    concurrency: usize,
) -> Result<Vec<(ProcessCommand, ProcessOutput)>, RepoctlError> {
    let limit = concurrency.max(1);
    let mut outputs = Vec::with_capacity(commands.len());
    for chunk in commands.chunks(limit) {
        let mut handles = Vec::with_capacity(chunk.len());
        for command in chunk.iter().cloned() {
            let runner = Arc::clone(runner);
            handles.push(std::thread::spawn(move || {
                let output = runner.run(&command)?;
                Ok::<_, RepoctlError>((command, output))
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(result) => outputs.push(result?),
                Err(_) => {
                    return Err(RepoctlError::Internal(
                        "task worker panicked while running a command".to_string(),
                    ));
                }
            }
        }
    }
    Ok(outputs)
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use repoctl_core::{
        CommandSpec, ProjectKind, ProjectRelativePath, RepoGraph, RepoManifest, RepoName,
        RepoPolicySet, RepoRoot, SchemaId, TaskCommand, WorkspaceName, WorkspaceSpec,
    };

    use super::*;

    #[test]
    fn test_should_compute_affected_project_and_task_runner_inputs() {
        let snapshot = fixture_snapshot();
        let report = compute_affected(
            &snapshot,
            &[RepoRelativePath::new("apps/catalog/src/lib.rs").expect("path")],
            &[TaskName::new("check").expect("task")],
        );
        assert_eq!(report.directly_affected[0].as_str(), "apps.catalog");
        assert!(
            report
                .tasks
                .iter()
                .any(|task| task == "apps.catalog:api:check")
        );
    }

    #[test]
    fn test_should_create_empty_ci_matrix_github_matrix() {
        let report = CiMatrixReport {
            entries: Vec::new(),
            github_actions: json!({ "include": [] }),
        };
        assert_eq!(report.github_actions["include"], json!([]));
    }

    fn fixture_snapshot() -> RepoSnapshot {
        let root = RepoRoot {
            absolute: camino::Utf8PathBuf::from("/tmp/repoctl-test"),
        };
        let repo_manifest = RepoManifest {
            schema: SchemaId::new("company.repo/v1").expect("schema"),
            name: RepoName::new("acme").expect("repo"),
            layout: repoctl_core::RepoLayout::Functional,
            default_owner: None,
            protos_root: RepoRelativePath::new("protos").expect("path"),
            core_infra_root: RepoRelativePath::new("core-infra").expect("path"),
            agent_skills_root: RepoRelativePath::new(".agents/skills").expect("path"),
            claude_skills_root: RepoRelativePath::new(".claude/skills").expect("path"),
            context_output: RepoRelativePath::new("target/repoctl/context").expect("path"),
            generated_code_policy: repoctl_core::GeneratedCodePolicy::ConsumerLocal,
            policies: RepoPolicySet::default(),
        };
        let mut tasks = BTreeMap::new();
        tasks.insert(
            TaskName::new("check").expect("task"),
            vec![TaskCommand {
                workspace: WorkspaceName::new("api").expect("workspace"),
                command: CommandSpec::parse("cargo check --workspace").expect("command"),
            }],
        );
        let project = ProjectManifest {
            schema: SchemaId::new("company.project/v1").expect("schema"),
            name: ProjectName::new("apps.catalog").expect("project"),
            kind: ProjectKind::App,
            path: RepoRelativePath::new("apps/catalog").expect("path"),
            owners: Vec::new(),
            visibility: repoctl_core::Visibility::Internal,
            workspaces: vec![WorkspaceSpec {
                name: WorkspaceName::new("api").expect("workspace"),
                language: WorkspaceLanguage::Rust,
                toolchain: None,
                root: ProjectRelativePath::new(".").expect("path"),
                manifest: ProjectRelativePath::new("Cargo.toml").expect("path"),
                lockfile: None,
                target_dir: Some(RepoRelativePath::new("target/rust").expect("path")),
                cache_dir: None,
            }],
            depends_on: Vec::new(),
            tasks,
            iac: None,
            deploy: None,
            protos: repoctl_core::ProjectProtoSpec::default(),
            ai: repoctl_core::ProjectAiSpec::default(),
            areas: repoctl_core::ProjectAreas::default(),
            policies: BTreeMap::new(),
            source: RepoRelativePath::new("apps/catalog/project.yaml").expect("path"),
        };
        RepoSnapshot::new(root, repo_manifest, vec![project], RepoGraph::default())
    }
}
