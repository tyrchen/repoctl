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
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use globset::Glob;
use repoctl_core::{
    AffectedReason, AffectedReport, AffectedRequest, AiContext, AiContextRequest, CdnCheck,
    CiFallback, CiMatrixReport, CiMatrixRequest, CodegenCheckReport, CodegenCheckRequest,
    Diagnostic, DnsOperation, EdgeKind, IacFacadeReport, IacFacadeRequest, IacOperation,
    IacProvider, ManualStateRecord, OpsJournalAction, OpsJournalReport, OpsJournalRequest, OpsPlan,
    OpsPlanRequest, OpsReconcileReport, OpsReconcileRequest, OpsVerifyReport, OpsVerifyRequest,
    PrSummary, PrSummaryRequest, ProbeSpec, ProcessCommand, ProcessOutput, ProcessRunner,
    ProjectManifest, ProjectName, ProtoFacadeReport, ProtoFacadeRequest, ProtoOperation,
    ProviderCapabilityReport, ProviderCapabilityRequest, RepoRelativePath, RepoSnapshot,
    RepoctlError, SessionEntry, SessionJournal, TaskCommandOutput, TaskDependency, TaskName,
    TaskRunPlan, TaskRunReport, TaskRunRequest, Toolchain, ToolchainAdapter,
    ToolchainEnvironmentInput,
};
use repoctl_engine::RepoctlEngine;
use serde::{Serialize, de::DeserializeOwned};
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
    fn toolchain(&self) -> Toolchain {
        Toolchain::Cargo
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

/// npm task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct NpmToolchainAdapter;

impl ToolchainAdapter for NpmToolchainAdapter {
    fn toolchain(&self) -> Toolchain {
        Toolchain::Npm
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(cache_dir) = &input.workspace.cache_dir {
            env.insert("NPM_CONFIG_CACHE".to_string(), cache_dir.to_string());
        }
        Ok(env)
    }
}

/// pnpm task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct PnpmToolchainAdapter;

impl ToolchainAdapter for PnpmToolchainAdapter {
    fn toolchain(&self) -> Toolchain {
        Toolchain::Pnpm
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(cache_dir) = &input.workspace.cache_dir {
            env.insert("PNPM_STORE_DIR".to_string(), cache_dir.to_string());
        }
        Ok(env)
    }
}

/// Yarn task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct YarnToolchainAdapter;

impl ToolchainAdapter for YarnToolchainAdapter {
    fn toolchain(&self) -> Toolchain {
        Toolchain::Yarn
    }

    fn environment(
        &self,
        input: &ToolchainEnvironmentInput<'_>,
    ) -> Result<BTreeMap<String, String>, RepoctlError> {
        let mut env = BTreeMap::new();
        if let Some(cache_dir) = &input.workspace.cache_dir {
            env.insert("YARN_CACHE_FOLDER".to_string(), cache_dir.to_string());
        }
        Ok(env)
    }
}

/// Bun task environment adapter.
#[derive(Clone, Debug, Default)]
pub struct BunToolchainAdapter;

impl ToolchainAdapter for BunToolchainAdapter {
    fn toolchain(&self) -> Toolchain {
        Toolchain::Bun
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
    fn toolchain(&self) -> Toolchain {
        Toolchain::Uv
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
            let diagnostics = if plan.commands.is_empty() {
                self.no_task_plan_diagnostics(request)?
            } else {
                Vec::new()
            };
            return Ok(TaskRunReport {
                commands: plan.commands,
                outputs: Vec::new(),
                diagnostics,
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
        if request.changed_files.is_empty() && request.base.is_none() && request.head.is_none() {
            match request.fallback {
                CiFallback::All => return self.ci_matrix_all(request),
                CiFallback::None => {
                    return Ok(CiMatrixReport {
                        entries: Vec::new(),
                        github_actions: json!({ "include": [] }),
                    });
                }
                CiFallback::Error => {
                    return Err(RepoctlError::diagnostic(Diagnostic::error(
                        "ci.matrix.no_changed_files",
                        "ci matrix cannot determine affected files without base/head or changed \
                         files",
                    )));
                }
            }
        }
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

    fn ci_matrix_all(&self, request: &CiMatrixRequest) -> Result<CiMatrixReport, RepoctlError> {
        let task_request = TaskRunRequest {
            repo: request.repo.clone(),
            tasks: request.tasks.clone(),
            projects: Vec::new(),
            workspaces: Vec::new(),
            affected: false,
            changed_files: Vec::new(),
            base: None,
            head: None,
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
        Ok(CiMatrixReport {
            github_actions: json!({ "include": entries.clone() }),
            entries,
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
        let dns = ops_dns_operations(&snapshot, &affected);
        let cdn = ops_cdn_checks(&snapshot, &affected);
        let provider_capabilities = provider_capability_reports(&snapshot, None, &changed_files)?;
        let production_gaps = ops_production_gaps(&snapshot);
        Ok(render_pr_summary(
            &snapshot,
            &changed_files,
            &affected,
            &diagnostics,
            &PrOperationalContext {
                dns,
                cdn,
                provider_capabilities,
                production_gaps,
            },
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

    /// Builds a non-mutating operations plan.
    pub fn ops_plan(&self, request: &OpsPlanRequest) -> Result<OpsPlan, RepoctlError> {
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
        let environments = if request.environments.is_empty() {
            vec!["staging".to_string()]
        } else {
            request.environments.clone()
        };
        let affected = compute_affected(&snapshot, &changed_files, &request.tasks);
        let task_request = TaskRunRequest {
            repo: request.repo.clone(),
            tasks: request.tasks.clone(),
            projects: Vec::new(),
            workspaces: Vec::new(),
            affected: true,
            changed_files: changed_files.clone(),
            base: request.base.clone(),
            head: request.head.clone(),
            concurrency: None,
            dry_run: true,
        };
        let task_plan = self.run_tasks(&task_request)?;
        let iac = ops_iac_operations(&snapshot, &affected, &changed_files, &environments)?;
        let dns = ops_dns_operations(&snapshot, &affected);
        let cdn = ops_cdn_checks(&snapshot, &affected);
        let probes = ops_probes(&snapshot, &affected);
        let manual_reconciliation = ops_manual_state(&snapshot, &affected);
        let provider_capabilities = provider_capability_reports(&snapshot, None, &changed_files)?;
        let diagnostics = ops_plan_diagnostics(
            request,
            &changed_files,
            &affected,
            &task_plan,
            &dns,
            &cdn,
            &provider_capabilities,
        );
        let plan = OpsPlan {
            id: new_artifact_id("ops-plan")?,
            repo_root: Some(snapshot.root.clone()),
            base: request.base.clone(),
            head: request.head.clone(),
            environments,
            affected,
            task_plan,
            iac,
            dns,
            cdn,
            provider_capabilities,
            probes,
            manual_reconciliation,
            required_env: vec![
                "AWS_PROFILE".to_string(),
                "AWS_REGION".to_string(),
                "AWS_DEFAULT_REGION".to_string(),
                "CLOUDFLARE_API_TOKEN".to_string(),
                "CLOUDFLARE_ZONE_ID".to_string(),
            ],
            production_gaps: ops_production_gaps(&snapshot),
            diagnostics,
        };
        if let Some(path) = &request.output {
            write_json_artifact(path, &plan)?;
        }
        Ok(plan)
    }

    /// Plans non-mutating verification from an operations plan.
    pub fn ops_verify(&self, request: &OpsVerifyRequest) -> Result<OpsVerifyReport, RepoctlError> {
        let plan = read_json_artifact::<OpsPlan>(&request.plan)?;
        let mut commands = vec![
            ProcessCommand {
                program: "repoctl".to_string(),
                args: vec!["graph".to_string(), "validate".to_string()],
                ..ProcessCommand::default()
            },
            ProcessCommand {
                program: "repoctl".to_string(),
                args: ops_affected_args(&plan),
                ..ProcessCommand::default()
            },
        ];
        commands.extend(plan.task_plan.commands.clone());
        commands.extend(
            plan.iac
                .iter()
                .map(|operation| operation.preview_command.clone()),
        );
        commands.extend(
            plan.dns
                .iter()
                .flat_map(|operation| operation.verification.clone()),
        );
        commands.extend(plan.cdn.iter().map(|check| check.verification.clone()));
        commands.extend(plan.probes.iter().map(probe_command));
        let skipped_mutating_commands = plan
            .iac
            .iter()
            .filter_map(|operation| operation.apply_command.clone())
            .chain(
                plan.manual_reconciliation
                    .iter()
                    .filter_map(|record| record.cleanup_command.clone()),
            )
            .collect::<Vec<_>>();
        Ok(OpsVerifyReport {
            commands,
            skipped_mutating_commands,
            diagnostics: Vec::new(),
        })
    }

    /// Builds manual-state reconciliation report from an operations plan.
    pub fn ops_reconcile(
        &self,
        request: &OpsReconcileRequest,
    ) -> Result<OpsReconcileReport, RepoctlError> {
        let plan = read_json_artifact::<OpsPlan>(&request.plan)?;
        let cleanup_commands = plan
            .manual_reconciliation
            .iter()
            .filter(|record| record.status != "removed" && record.status != "reconciled")
            .filter_map(|record| record.cleanup_command.clone())
            .collect::<Vec<_>>();
        let diagnostics = if plan.manual_reconciliation.is_empty() {
            vec![Diagnostic::warning(
                "ops.reconcile.no_manual_state",
                "the plan does not record manual state to reconcile",
            )]
        } else if cleanup_commands.is_empty() {
            Vec::new()
        } else {
            vec![Diagnostic::warning(
                "ops.reconcile.cleanup_pending",
                "temporary manual state still has cleanup commands to review",
            )]
        };
        Ok(OpsReconcileReport {
            records: plan.manual_reconciliation,
            cleanup_commands,
            diagnostics,
        })
    }

    /// Inspects provider package capabilities for selected workspaces.
    pub fn provider_capabilities(
        &self,
        request: &ProviderCapabilityRequest,
    ) -> Result<Vec<ProviderCapabilityReport>, RepoctlError> {
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
        provider_capability_reports(&snapshot, request.workspace.as_deref(), &changed_files)
    }

    /// Manages local operations session journals.
    pub fn ops_journal(
        &self,
        request: &OpsJournalRequest,
    ) -> Result<OpsJournalReport, RepoctlError> {
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        let session_dir = snapshot
            .root
            .absolute
            .join("target/repoctl/sessions")
            .as_std_path()
            .to_path_buf();
        fs::create_dir_all(&session_dir).map_err(|source| {
            RepoctlError::Environment(format!(
                "failed to create session directory `{}`: {source}",
                session_dir.display()
            ))
        })?;
        match &request.action {
            OpsJournalAction::Start { name, plan_id } => {
                let journal = SessionJournal {
                    id: new_artifact_id("session")?,
                    name: sanitize_session_name(name),
                    plan_id: plan_id.clone(),
                    entries: Vec::new(),
                };
                let path = session_dir.join(format!("{}.json", journal.id));
                write_json_artifact(&path, &journal)?;
                Ok(OpsJournalReport {
                    path: Some(path),
                    journal: Some(journal),
                    markdown: None,
                    diagnostics: Vec::new(),
                })
            }
            OpsJournalAction::AddCommand {
                session,
                command,
                exit_status,
            } => {
                let (path, mut journal) = load_session_journal(&session_dir, session)?;
                journal.entries.push(SessionEntry {
                    kind: "command".to_string(),
                    timestamp: unix_timestamp()?,
                    command: Some(redact_secret_like_values(command)),
                    exit_status: *exit_status,
                    message: None,
                    plan_id: journal.plan_id.clone(),
                });
                write_json_artifact(&path, &journal)?;
                Ok(OpsJournalReport {
                    path: Some(path),
                    journal: Some(journal),
                    markdown: None,
                    diagnostics: Vec::new(),
                })
            }
            OpsJournalAction::AddNote {
                session,
                note_kind,
                message,
            } => {
                let (path, mut journal) = load_session_journal(&session_dir, session)?;
                journal.entries.push(SessionEntry {
                    kind: note_kind.clone(),
                    timestamp: unix_timestamp()?,
                    command: None,
                    exit_status: None,
                    message: Some(redact_secret_like_values(message)),
                    plan_id: journal.plan_id.clone(),
                });
                write_json_artifact(&path, &journal)?;
                Ok(OpsJournalReport {
                    path: Some(path),
                    journal: Some(journal),
                    markdown: None,
                    diagnostics: Vec::new(),
                })
            }
            OpsJournalAction::Summary { session } => {
                let (path, journal) = load_session_journal(&session_dir, session)?;
                let markdown = render_session_summary(&journal);
                Ok(OpsJournalReport {
                    path: Some(path),
                    journal: Some(journal),
                    markdown: Some(markdown),
                    diagnostics: Vec::new(),
                })
            }
        }
    }

    fn changed_files(
        &self,
        repo: Option<&Path>,
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

    fn no_task_plan_diagnostics(
        &self,
        request: &TaskRunRequest,
    ) -> Result<Vec<Diagnostic>, RepoctlError> {
        if request.affected
            && request.changed_files.is_empty()
            && (request.base.is_none() || request.head.is_none())
        {
            return Ok(vec![Diagnostic::warning(
                "task.plan.no_base_head",
                "affected task planning requires --base and --head or --changed-file",
            )]);
        }
        let snapshot = self
            .engine
            .discovery()
            .discover(&repoctl_core::DiscoverRequest {
                repo: request.repo.clone(),
            })?;
        if request.affected {
            let changed_files = self.changed_files(
                request.repo.as_deref(),
                request.base.as_deref(),
                request.head.as_deref(),
                &request.changed_files,
            )?;
            if changed_files.is_empty() {
                return Ok(vec![Diagnostic::warning(
                    "task.plan.no_changed_files",
                    "no changed files were found for affected task planning",
                )]);
            }
            let affected = compute_affected(&snapshot, &changed_files, &request.tasks);
            if affected.directly_affected.is_empty() && affected.transitively_affected.is_empty() {
                return Ok(vec![Diagnostic::warning(
                    "task.plan.no_affected_projects",
                    "changed files did not match any project or graph-wide surface",
                )]);
            }
            if affected.tasks.is_empty() {
                return Ok(vec![Diagnostic::warning(
                    "task.plan.no_matching_task",
                    "affected projects do not declare the requested task",
                )]);
            }
            return Ok(vec![Diagnostic::error(
                "task.plan.unresolved_command",
                "affected report included task ids, but no runnable command could be resolved",
            )]);
        }
        Ok(vec![Diagnostic::warning(
            "task.plan.no_matching_task",
            "no project or workspace declares the requested task",
        )])
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

fn ops_iac_operations(
    snapshot: &RepoSnapshot,
    affected: &AffectedReport,
    changed_files: &[RepoRelativePath],
    environments: &[String],
) -> Result<Vec<IacOperation>, RepoctlError> {
    let affected_projects = affected_project_set(affected);
    let mut operations = Vec::new();
    if changed_files.iter().any(|file| {
        file.as_str()
            .starts_with(snapshot.repo_manifest.core_infra_root.as_str())
    }) {
        for environment in environments {
            let target = IacTarget {
                project: None,
                root: snapshot.repo_manifest.core_infra_root.clone(),
                provider: IacProvider::Pulumi,
                stack: environment.clone(),
                core: true,
            };
            operations.push(iac_operation(
                snapshot,
                &target,
                environment,
                changed_files,
            )?);
        }
    }
    let mut projects = snapshot
        .projects
        .iter()
        .filter(|project| affected_projects.contains(&project.name))
        .filter(|project| project.iac.is_some())
        .collect::<Vec<_>>();
    projects.sort_by_key(|project| (ops_project_rank(project), project.name.to_string()));
    for project in projects {
        let Some(iac) = &project.iac else {
            continue;
        };
        let root = project
            .path
            .join_project(&iac.root)
            .map_err(RepoctlError::diagnostic)?;
        for environment in environments {
            let target = IacTarget {
                project: Some(project),
                root: root.clone(),
                provider: iac.provider.clone(),
                stack: environment.clone(),
                core: false,
            };
            operations.push(iac_operation(
                snapshot,
                &target,
                environment,
                changed_files,
            )?);
        }
    }
    Ok(operations)
}

fn iac_operation(
    snapshot: &RepoSnapshot,
    target: &IacTarget<'_>,
    environment: &str,
    changed_files: &[RepoRelativePath],
) -> Result<IacOperation, RepoctlError> {
    let preview_command = iac_plan_command(snapshot, target)?;
    let apply_command = Some(iac_apply_command(snapshot, target));
    Ok(IacOperation {
        project: target.project.map(|project| project.name.clone()),
        workspace: target.root.to_string(),
        provider: target.provider.clone(),
        environment: environment.to_string(),
        stack: target.stack.clone(),
        preview_command,
        apply_command,
        risk: iac_risk_flags(std::slice::from_ref(target), changed_files),
    })
}

fn iac_apply_command(snapshot: &RepoSnapshot, target: &IacTarget<'_>) -> ProcessCommand {
    match target.provider {
        IacProvider::Pulumi => iac_command(
            snapshot,
            target,
            "pulumi",
            vec![
                "up".to_string(),
                "--stack".to_string(),
                target.stack.clone(),
                "--yes".to_string(),
            ],
        ),
        IacProvider::Terraform => iac_command(
            snapshot,
            target,
            "terraform",
            vec![
                "apply".to_string(),
                "-var".to_string(),
                format!("env={}", target.stack),
                "-auto-approve".to_string(),
            ],
        ),
        IacProvider::OpenTofu => iac_command(
            snapshot,
            target,
            "tofu",
            vec![
                "apply".to_string(),
                "-var".to_string(),
                format!("env={}", target.stack),
                "-auto-approve".to_string(),
            ],
        ),
    }
}

fn ops_dns_operations(snapshot: &RepoSnapshot, affected: &AffectedReport) -> Vec<DnsOperation> {
    selected_projects(snapshot, affected)
        .into_iter()
        .flat_map(|project| {
            let provider = project
                .dns
                .provider
                .clone()
                .unwrap_or_else(|| "dns".to_string());
            project
                .dns
                .records
                .iter()
                .map(|record| DnsOperation {
                    zone: infer_dns_zone(&record.name),
                    provider: provider.clone(),
                    record: record.name.clone(),
                    expected_target: record.target.clone(),
                    expected_proxied: record.proxied,
                    verification: dns_verification_commands(&provider, record),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn dns_verification_commands(
    provider: &str,
    record: &repoctl_core::DnsRecordSpec,
) -> Vec<ProcessCommand> {
    let mut commands = vec![ProcessCommand {
        program: "dig".to_string(),
        args: vec![
            "+short".to_string(),
            record.record_type.clone(),
            record.name.clone(),
        ],
        ..ProcessCommand::default()
    }];
    if provider == "cloudflare" {
        commands.push(ProcessCommand {
            program: "curl".to_string(),
            args: vec![
                "--fail".to_string(),
                "--silent".to_string(),
                "--show-error".to_string(),
                "-H".to_string(),
                "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}".to_string(),
                format!(
                    "https://api.cloudflare.com/client/v4/zones/${{CLOUDFLARE_ZONE_ID}}/dns_records?name={}",
                    record.name
                ),
            ],
            ..ProcessCommand::default()
        });
    }
    commands
}

fn ops_cdn_checks(snapshot: &RepoSnapshot, affected: &AffectedReport) -> Vec<CdnCheck> {
    selected_projects(snapshot, affected)
        .into_iter()
        .filter_map(|project| project.cdn.as_ref())
        .flat_map(|cdn| {
            cdn.aliases
                .iter()
                .map(|alias| CdnCheck {
                    provider: cdn.provider.clone(),
                    alias: alias.clone(),
                    expected_response_headers: cdn.expected_response_headers.clone(),
                    verification: ProcessCommand {
                        program: "curl".to_string(),
                        args: vec![
                            "--head".to_string(),
                            "--fail".to_string(),
                            "--silent".to_string(),
                            "--show-error".to_string(),
                            format!("https://{alias}"),
                        ],
                        ..ProcessCommand::default()
                    },
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn ops_probes(snapshot: &RepoSnapshot, affected: &AffectedReport) -> Vec<ProbeSpec> {
    selected_projects(snapshot, affected)
        .into_iter()
        .flat_map(|project| project.ops.probes.clone())
        .collect()
}

fn ops_manual_state(snapshot: &RepoSnapshot, affected: &AffectedReport) -> Vec<ManualStateRecord> {
    selected_projects(snapshot, affected)
        .into_iter()
        .flat_map(|project| project.ops.manual_state.clone())
        .collect()
}

fn selected_projects<'a>(
    snapshot: &'a RepoSnapshot,
    affected: &AffectedReport,
) -> Vec<&'a ProjectManifest> {
    let mut names = affected_project_set(affected);
    let runtime_dependencies = snapshot
        .projects
        .iter()
        .filter(|project| names.contains(&project.name))
        .flat_map(|project| {
            project
                .ops
                .runtime_dependencies
                .iter()
                .map(|dependency| dependency.project.clone())
        })
        .collect::<Vec<_>>();
    names.extend(runtime_dependencies);
    snapshot
        .projects
        .iter()
        .filter(|project| names.contains(&project.name))
        .collect()
}

fn affected_project_set(affected: &AffectedReport) -> BTreeSet<ProjectName> {
    affected
        .directly_affected
        .iter()
        .chain(affected.transitively_affected.iter())
        .cloned()
        .collect()
}

fn ops_project_rank(project: &ProjectManifest) -> u8 {
    match project.kind {
        repoctl_core::ProjectKind::CoreInfra | repoctl_core::ProjectKind::CoreInfraComponent => 0,
        repoctl_core::ProjectKind::Framework => 1,
        repoctl_core::ProjectKind::FoundationService => 2,
        repoctl_core::ProjectKind::App => 3,
        repoctl_core::ProjectKind::ProtoRoot => 4,
        repoctl_core::ProjectKind::Tool => 5,
    }
}

fn infer_dns_zone(record: &str) -> String {
    let parts = record.split('.').collect::<Vec<_>>();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        record.to_string()
    }
}

fn probe_command(probe: &ProbeSpec) -> ProcessCommand {
    let mut args = vec![
        "--fail-with-body".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        "--request".to_string(),
        probe.method.clone(),
    ];
    args.push(probe.url.clone());
    ProcessCommand {
        program: "curl".to_string(),
        args,
        ..ProcessCommand::default()
    }
}

fn ops_plan_diagnostics(
    request: &OpsPlanRequest,
    changed_files: &[RepoRelativePath],
    affected: &AffectedReport,
    task_plan: &TaskRunReport,
    dns: &[DnsOperation],
    cdn: &[CdnCheck],
    provider_capabilities: &[ProviderCapabilityReport],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if request.changed_files.is_empty()
        && changed_files.is_empty()
        && (request.base.is_none() || request.head.is_none())
    {
        diagnostics.push(Diagnostic::warning(
            "ops.plan.no_base_head",
            "ops plan could not compare a git range without --base/--head or --changed-file",
        ));
    }
    if affected.directly_affected.is_empty() && affected.transitively_affected.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "ops.plan.no_affected_projects",
            "no projects were affected by the selected change set",
        ));
    }
    diagnostics.extend(task_plan.diagnostics.clone());
    if !dns.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "ops.plan.dns_review_required",
            "DNS intent is present; verify authoritative provider, record target, and proxy mode",
        ));
    }
    if !cdn.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "ops.plan.cdn_review_required",
            "CDN intent is present; verify response headers prove the expected serving layer",
        ));
    }
    diagnostics.extend(
        provider_capabilities
            .iter()
            .flat_map(|report| report.diagnostics.clone()),
    );
    diagnostics
}

fn ops_production_gaps(snapshot: &RepoSnapshot) -> Vec<String> {
    snapshot
        .projects
        .iter()
        .filter(|project| !project.dns.records.is_empty() || project.cdn.is_some())
        .filter(|project| {
            project
                .iac
                .as_ref()
                .is_none_or(|iac| !iac.stacks.iter().any(|stack| stack == "prod"))
        })
        .map(|project| {
            format!(
                "{} has DNS/CDN intent but no prod IaC stack declared",
                project.name
            )
        })
        .collect()
}

fn ops_affected_args(plan: &OpsPlan) -> Vec<String> {
    let mut args = vec!["affected".to_string()];
    if let Some(base) = &plan.base {
        args.push("--base".to_string());
        args.push(base.clone());
    }
    if let Some(head) = &plan.head {
        args.push("--head".to_string());
        args.push(head.clone());
    }
    args
}

fn provider_capability_reports(
    snapshot: &RepoSnapshot,
    selector: Option<&str>,
    changed_files: &[RepoRelativePath],
) -> Result<Vec<ProviderCapabilityReport>, RepoctlError> {
    let mut reports = Vec::new();
    for project in &snapshot.projects {
        for workspace in &project.workspaces {
            let workspace_id = format!("{}:{}", project.name, workspace.name);
            let workspace_root = project
                .path
                .join_project(&workspace.root)
                .map_err(RepoctlError::diagnostic)?;
            if let Some(selector) = selector
                && selector != workspace_id
                && selector != workspace_root.as_str()
            {
                continue;
            }
            let package_json = snapshot
                .root
                .absolute
                .join(workspace_root.as_str())
                .join("package.json");
            if !package_json.is_file() {
                continue;
            }
            let Some(version) = package_version(package_json.as_std_path(), "@pulumi/aws")? else {
                continue;
            };
            let uses_function_url_field = workspace_uses_text(
                snapshot
                    .root
                    .absolute
                    .join(workspace_root.as_str())
                    .as_std_path(),
                "invokedViaFunctionUrl",
            )?;
            let provider_major_changed = changed_files.iter().any(|file| {
                file.as_str().ends_with("package.json")
                    || file.as_str().ends_with("package-lock.json")
                    || file.as_str().ends_with("pnpm-lock.yaml")
                    || file.as_str().ends_with("yarn.lock")
            });
            if uses_function_url_field && !version_supports_invoked_via_function_url(&version) {
                reports.push(ProviderCapabilityReport {
                    workspace: workspace_id,
                    package: "@pulumi/aws".to_string(),
                    version,
                    resource: "aws.lambda.Permission".to_string(),
                    field: "invokedViaFunctionUrl".to_string(),
                    status: "missing".to_string(),
                    advice: "avoid a blind provider major upgrade; use a compatibility adapter or \
                             preview every impacted stack before upgrading"
                        .to_string(),
                    diagnostics: vec![Diagnostic::warning(
                        "provider.capability.missing",
                        "local Pulumi AWS package may not support invokedViaFunctionUrl",
                    )],
                });
            } else if provider_major_changed {
                reports.push(ProviderCapabilityReport {
                    workspace: workspace_id,
                    package: "@pulumi/aws".to_string(),
                    version,
                    resource: "provider-migration".to_string(),
                    field: "major-version".to_string(),
                    status: "review".to_string(),
                    advice: "run ordered Pulumi previews for all stacks before accepting provider \
                             migration state churn"
                        .to_string(),
                    diagnostics: vec![Diagnostic::warning(
                        "provider.capability.major_upgrade_review",
                        "provider package or lockfile changed and may require broad stack previews",
                    )],
                });
            }
        }
    }
    Ok(reports)
}

fn package_version(path: &Path, package: &str) -> Result<Option<String>, RepoctlError> {
    let content = fs::read_to_string(path).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to read package manifest `{}`: {source}",
            path.display()
        ))
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&content).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to parse package manifest `{}`: {source}",
            path.display()
        ))
    })?;
    Ok(["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .find_map(|section| {
            value
                .get(section)?
                .get(package)?
                .as_str()
                .map(ToString::to_string)
        }))
}

fn version_supports_invoked_via_function_url(version: &str) -> bool {
    let digits = version
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse::<u64>().is_ok_and(|major| major >= 7)
}

fn workspace_uses_text(root: &Path, needle: &str) -> Result<bool, RepoctlError> {
    if !root.is_dir() {
        return Ok(false);
    }
    for path in source_files(root)? {
        let content = fs::read_to_string(&path).map_err(|source| {
            RepoctlError::Environment(format!("failed to read `{}`: {source}", path.display()))
        })?;
        if content.contains(needle) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn source_files(root: &Path) -> Result<Vec<PathBuf>, RepoctlError> {
    let mut files = Vec::new();
    collect_source_files(root, &mut files)?;
    Ok(files)
}

fn collect_source_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), RepoctlError> {
    for entry in fs::read_dir(root).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to read directory `{}`: {source}",
            root.display()
        ))
    })? {
        let entry = entry.map_err(|source| {
            RepoctlError::Environment(format!(
                "failed to read directory entry under `{}`: {source}",
                root.display()
            ))
        })?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if file_name == "node_modules" || file_name == ".git" || file_name == "dist" {
            continue;
        }
        if path.is_dir() {
            collect_source_files(&path, files)?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| matches!(ext, "ts" | "tsx" | "js" | "jsx" | "json"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn write_json_artifact<T: Serialize>(path: &Path, value: &T) -> Result<(), RepoctlError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            RepoctlError::Environment(format!(
                "failed to create artifact directory `{}`: {source}",
                parent.display()
            ))
        })?;
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|source| RepoctlError::Internal(format!("failed to serialize JSON: {source}")))?;
    fs::write(path, content).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to write artifact `{}`: {source}",
            path.display()
        ))
    })
}

fn read_json_artifact<T: DeserializeOwned>(path: &Path) -> Result<T, RepoctlError> {
    let content = fs::read_to_string(path).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to read artifact `{}`: {source}",
            path.display()
        ))
    })?;
    serde_json::from_str(&content).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to parse artifact `{}`: {source}",
            path.display()
        ))
    })
}

fn new_artifact_id(prefix: &str) -> Result<String, RepoctlError> {
    Ok(format!("{prefix}-{}", unix_timestamp()?))
}

fn unix_timestamp() -> Result<u64, RepoctlError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| {
            RepoctlError::Internal(format!("system clock before Unix epoch: {source}"))
        })
}

fn sanitize_session_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn load_session_journal(
    session_dir: &Path,
    session: &str,
) -> Result<(PathBuf, SessionJournal), RepoctlError> {
    let direct_path = session_dir.join(format!("{session}.json"));
    if direct_path.is_file() {
        let journal = read_json_artifact(&direct_path)?;
        return Ok((direct_path, journal));
    }
    for entry in fs::read_dir(session_dir).map_err(|source| {
        RepoctlError::Environment(format!(
            "failed to read session directory `{}`: {source}",
            session_dir.display()
        ))
    })? {
        let entry = entry.map_err(|source| {
            RepoctlError::Environment(format!(
                "failed to read session directory entry `{}`: {source}",
                session_dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let journal = read_json_artifact::<SessionJournal>(&path)?;
        if journal.name == session || journal.id == session {
            return Ok((path, journal));
        }
    }
    Err(RepoctlError::diagnostic(Diagnostic::error(
        "ops.journal.session_not_found",
        format!("session `{session}` was not found"),
    )))
}

fn redact_secret_like_values(value: &str) -> String {
    let mut redacted = Vec::new();
    let mut skip_parts = 0_u8;
    for part in value.split_whitespace() {
        if skip_parts > 0 {
            redacted.push("[REDACTED]".to_string());
            skip_parts = skip_parts.saturating_sub(1);
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if lower.contains("authorization") || lower.contains("cookie") {
            redacted.push("[REDACTED]".to_string());
            skip_parts = 2;
        } else if lower.contains("token")
            || lower.contains("secret")
            || lower.contains("api_key")
            || lower.contains("apikey")
        {
            redacted.push("[REDACTED]".to_string());
            if !part.contains('=') && !part.contains(':') {
                skip_parts = 1;
            }
        } else {
            redacted.push(part.to_string());
        }
    }
    redacted.join(" ")
}

fn render_session_summary(journal: &SessionJournal) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Operations Session: {}", journal.name);
    if let Some(plan_id) = &journal.plan_id {
        let _ = writeln!(markdown, "\nPlan: `{plan_id}`");
    }
    markdown.push_str("\n## Evidence\n");
    for entry in &journal.entries {
        let detail = entry
            .command
            .as_ref()
            .or(entry.message.as_ref())
            .map_or("", String::as_str);
        let status = entry
            .exit_status
            .map(|status| format!(" status={status}"))
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- `{}` at {}{}: {}",
            entry.kind, entry.timestamp, status, detail
        );
    }
    markdown
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

#[derive(Debug)]
struct PrOperationalContext {
    dns: Vec<DnsOperation>,
    cdn: Vec<CdnCheck>,
    provider_capabilities: Vec<ProviderCapabilityReport>,
    production_gaps: Vec<String>,
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
    operations: &PrOperationalContext,
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
    markdown.push_str("\n## Deploy Surface\n");
    if operations.dns.is_empty()
        && operations.cdn.is_empty()
        && operations.provider_capabilities.is_empty()
    {
        markdown.push_str("- No declared DNS, CDN, or provider-capability surface found.\n");
    }
    for operation in &operations.dns {
        let proxied = operation
            .expected_proxied
            .map_or_else(|| "unknown".to_string(), |value| value.to_string());
        let _ = writeln!(
            markdown,
            "- DNS `{}` via `{}` target `{}` proxied `{}`",
            operation.record, operation.provider, operation.expected_target, proxied
        );
    }
    for check in &operations.cdn {
        let _ = writeln!(
            markdown,
            "- CDN `{}` serves `{}` with headers `{}`",
            check.provider,
            check.alias,
            check.expected_response_headers.join(", ")
        );
    }
    for report in &operations.provider_capabilities {
        let _ = writeln!(
            markdown,
            "- Provider `{}` `{}` in `{}`: `{}` `{}`",
            report.package, report.version, report.workspace, report.status, report.field
        );
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
    markdown.push_str("- `repoctl affected --tasks check,test,build`\n");
    markdown.push_str("- `repoctl run check --affected --dry-run`\n");
    markdown
        .push_str("- `repoctl ops plan --env staging --tasks check,test,build --format json`\n");
    markdown.push_str("- `repoctl ops verify --plan target/repoctl/ops-plan.json`\n");
    markdown.push_str("- `repoctl provider capabilities`\n");
    if !operations.production_gaps.is_empty() {
        markdown.push_str("\n## Unresolved Gaps\n");
        for gap in &operations.production_gaps {
            let _ = writeln!(markdown, "- {gap}");
        }
    }
    let impact = json!({
        "repo": snapshot.repo_manifest.name,
        "changedFiles": changed_files,
        "affected": affected,
        "diagnostics": diagnostics,
        "deploySurface": {
            "dns": &operations.dns,
            "cdn": &operations.cdn,
            "providerCapabilities": &operations.provider_capabilities,
        },
        "productionGaps": &operations.production_gaps,
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
    let mut tasks = BTreeSet::new();
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
                tasks.insert(format!("{}:{}:{}", project.name, command.workspace, task));
            }
        }
    }
    tasks.into_iter().collect()
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
    seen: BTreeSet<(ProjectName, repoctl_core::WorkspaceName, TaskName)>,
    stack: BTreeSet<(ProjectName, repoctl_core::WorkspaceName, TaskName)>,
}

fn append_task_command_with_dependencies(
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
    task: &TaskName,
    task_command: &repoctl_core::TaskCommand,
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
    if !state.stack.insert(key.clone()) {
        state.commands.push(Err(RepoctlError::diagnostic(
            Diagnostic::error("task.dependency_cycle", "task prerequisite cycle detected")
                .with_project(project.name.as_str()),
        )));
        return;
    }
    for dependency in &task_command.depends_on {
        append_task_dependency(snapshot, dependency, toolchain_adapters, state);
    }
    state.commands.push(build_process_command(
        snapshot,
        project,
        task,
        task_command,
        toolchain_adapters,
    ));
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
    task_command: &repoctl_core::TaskCommand,
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
    workspace: &repoctl_core::WorkspaceSpec,
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

fn default_toolchain_adapters() -> Vec<Arc<dyn ToolchainAdapter>> {
    vec![
        Arc::new(RustToolchainAdapter),
        Arc::new(NpmToolchainAdapter),
        Arc::new(PnpmToolchainAdapter),
        Arc::new(YarnToolchainAdapter),
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
        RepoPolicySet, RepoRoot, SchemaId, TaskCommand, WorkspaceLanguage, WorkspaceName,
        WorkspaceSpec,
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

    #[test]
    fn test_should_redact_secret_marker_values() {
        let redacted = redact_secret_like_values(
            "curl -H Authorization: Bearer abc123 --token def456 --cookie session=ghi",
        );
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("def456"));
        assert!(!redacted.contains("session=ghi"));
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
            inspection: repoctl_core::RepoInspectionConfig::default(),
        };
        let mut tasks = BTreeMap::new();
        tasks.insert(
            TaskName::new("check").expect("task"),
            vec![TaskCommand {
                workspace: WorkspaceName::new("api").expect("workspace"),
                command: CommandSpec::parse("cargo check --workspace").expect("command"),
                depends_on: Vec::new(),
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
                toolchain: Some(Toolchain::Cargo),
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
            dns: repoctl_core::ProjectDnsSpec::default(),
            cdn: None,
            ops: repoctl_core::ProjectOpsSpec::default(),
            protos: repoctl_core::ProjectProtoSpec::default(),
            ai: repoctl_core::ProjectAiSpec::default(),
            areas: repoctl_core::ProjectAreas::default(),
            policies: BTreeMap::new(),
            source: RepoRelativePath::new("apps/catalog/project.yaml").expect("path"),
        };
        RepoSnapshot::new(root, repo_manifest, vec![project], RepoGraph::default())
    }
}
