#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
// Facade methods intentionally own request DTOs so frontends can build-and-call without lifetimes.
#![allow(clippy::needless_pass_by_value)]
// Adoption and hygiene facade operations are synchronous CLI orchestration, not async services.
#![allow(clippy::disallowed_methods)]

//! Public repoctl facade consumed by CLI and other frontends.

mod adoption;
mod ci;
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use adoption::{apply_adoption_plan, plan_adoption, read_adoption_plan};
use ci::render_github_actions_workflow;
pub use repoctl_core::*;
use repoctl_engine::{
    DefaultGraphBuilder, DefaultRepoLocator, DiscoveryService, LocalRepoFileSystem, RepoctlEngine,
};
pub use repoctl_engine::{affected_mermaid, graph_mermaid};
use repoctl_inspect::InspectorService;
use repoctl_runner::RunnerService;
use repoctl_scaffold::ScaffoldService;
use serde_json::Value;

/// Typed repoctl facade.
#[derive(Clone, Debug)]
pub struct Repoctl {
    engine: RepoctlEngine,
    scaffold: ScaffoldService,
    runner: RunnerService,
    inspector: InspectorService,
    proto: ProtoFacade,
    iac: IacFacade,
    skills: SkillsFacade,
}

impl Repoctl {
    /// Creates a facade with default local adapters.
    pub fn with_default_adapters() -> Result<Self, RepoctlError> {
        Ok(Self {
            engine: RepoctlEngine::with_default_adapters(),
            scaffold: ScaffoldService::with_default_adapters(),
            runner: RunnerService::with_default_adapters(),
            inspector: InspectorService::with_default_adapters(),
            proto: ProtoFacade {
                runner: RunnerService::with_default_adapters(),
            },
            iac: IacFacade {
                runner: RunnerService::with_default_adapters(),
            },
            skills: SkillsFacade,
        })
    }

    /// Discovers a repository snapshot.
    pub fn discover(&self, request: DiscoverRequest) -> Result<RepoSnapshot, RepoctlError> {
        self.engine.discovery().discover(&request)
    }

    /// Validates graph construction and boundary policies.
    pub fn validate_graph(
        &self,
        request: GraphValidateRequest,
    ) -> Result<ValidationReport, RepoctlError> {
        let snapshot = if request.mode == ValidationMode::Structural {
            structural_discovery().discover(&DiscoverRequest { repo: request.repo })?
        } else if request.mode == ValidationMode::Full {
            DiscoveryService::new(
                Arc::new(DefaultRepoLocator),
                Arc::new(LocalRepoFileSystem),
                Arc::new(YamlManifestParser),
                Arc::new(DefaultGraphBuilder::new(vec![
                    Arc::new(repoctl_engine::RustWorkspaceInspector),
                    Arc::new(repoctl_engine::NodeWorkspaceInspector),
                    Arc::new(repoctl_engine::PythonWorkspaceInspector),
                ])),
            )
            .discover(&DiscoverRequest { repo: request.repo })?
        } else {
            self.discover(DiscoverRequest { repo: request.repo })?
        };
        let diagnostics = self
            .engine
            .policies()
            .evaluate(&snapshot, &request.changed_files)?;
        Ok(ValidationReport::new(diagnostics))
    }

    /// Returns a graph print report.
    pub fn graph_print(
        &self,
        request: GraphPrintRequest,
    ) -> Result<GraphPrintReport, RepoctlError> {
        let snapshot = self.discover(DiscoverRequest { repo: request.repo })?;
        Ok(GraphPrintReport { snapshot })
    }

    /// Explains a project selector or graph node id.
    pub fn explain(&self, request: ExplainRequest) -> Result<ExplainReport, RepoctlError> {
        let snapshot = self.discover(DiscoverRequest { repo: request.repo })?;
        let nodes = snapshot
            .graph
            .nodes
            .iter()
            .filter(|node| {
                node.id == request.selector
                    || node.label == request.selector
                    || node
                        .project
                        .as_ref()
                        .is_some_and(|project| project.as_str() == request.selector)
            })
            .cloned()
            .collect::<Vec<_>>();
        let node_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
        let edges = snapshot
            .graph
            .edges
            .iter()
            .filter(|edge| node_ids.contains(&edge.from) || node_ids.contains(&edge.to))
            .cloned()
            .collect::<Vec<_>>();
        let diagnostics = if nodes.is_empty() {
            vec![Diagnostic::error(
                "explain.selector.not_found",
                format!("no graph node matched `{}`", request.selector),
            )]
        } else {
            Vec::new()
        };
        Ok(ExplainReport {
            selector: request.selector,
            nodes,
            edges,
            diagnostics,
        })
    }

    /// Lints boundary policies.
    pub fn lint_boundaries(
        &self,
        request: BoundaryLintRequest,
    ) -> Result<BoundaryLintReport, RepoctlError> {
        let report = self.validate_graph(GraphValidateRequest {
            repo: request.repo,
            changed_files: request.changed_files,
            mode: ValidationMode::Structural,
        })?;
        Ok(BoundaryLintReport {
            diagnostics: report.diagnostics,
        })
    }

    /// Plans repository initialization.
    pub fn init(&self, request: InitRequest) -> Result<InitPlan, RepoctlError> {
        self.scaffold.init(&request)
    }

    /// Plans a new project.
    pub fn new_project(&self, request: NewProjectRequest) -> Result<RenderPlan, RepoctlError> {
        let plan = self.scaffold.new_project(&request)?;
        if !request.dry_run {
            let report = self.validate_graph(GraphValidateRequest {
                repo: request.repo,
                changed_files: Vec::new(),
                mode: ValidationMode::Structural,
            })?;
            if !report.is_success() {
                return Err(RepoctlError::Diagnostics {
                    diagnostics: report.diagnostics,
                });
            }
        }
        Ok(plan)
    }

    /// Computes affected projects.
    pub fn affected(&self, request: AffectedRequest) -> Result<AffectedReport, RepoctlError> {
        self.runner.affected(&request)
    }

    /// Runs a repo task.
    pub fn run_task(&self, request: TaskRunRequest) -> Result<TaskRunReport, RepoctlError> {
        self.runner.run_tasks(&request)
    }

    /// Executes a selected CI task with optional explicit dependency installation.
    ///
    /// ```no_run
    /// use repoctl::{Repoctl, TaskRunRequest};
    /// # fn main() -> Result<(), repoctl::RepoctlError> {
    /// let report = Repoctl::with_default_adapters()?.ci_run_step(TaskRunRequest::default(), true)?;
    /// # let _ = report;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns an error when discovery, task planning or dependency installation fails.
    pub fn ci_run_step(
        &self,
        request: TaskRunRequest,
        install: bool,
    ) -> Result<TaskRunReport, RepoctlError> {
        self.runner.run_ci_step(&request, install)
    }

    /// Builds a CI matrix.
    pub fn ci_matrix(&self, request: CiMatrixRequest) -> Result<CiMatrixReport, RepoctlError> {
        self.runner.ci_matrix(&request)
    }

    /// Renders a CI workflow.
    pub fn ci_workflow(
        &self,
        request: CiWorkflowRequest,
    ) -> Result<CiWorkflowReport, RepoctlError> {
        let snapshot = self.discover(DiscoverRequest {
            repo: request.repo.clone(),
        })?;
        let path = RepoRelativePath::new(".github/workflows/repoctl.yml")
            .map_err(RepoctlError::diagnostic)?;
        let content = render_github_actions_workflow(&snapshot);
        let operation = FileOperation {
            path: path.clone(),
            operation: if request.write {
                "write-file".to_string()
            } else {
                "plan-file".to_string()
            },
            content: Some(content.clone()),
        };
        if request.write {
            let root = snapshot.root.absolute.as_std_path();
            write_utf8_file(root, &path, &content)?;
        }
        Ok(CiWorkflowReport {
            path,
            content,
            operations: vec![operation],
            diagnostics: Vec::new(),
        })
    }

    /// Checks generated artifact hygiene.
    pub fn hygiene_check(
        &self,
        request: HygieneCheckRequest,
    ) -> Result<HygieneReport, RepoctlError> {
        let root = locate_repo_path(request.repo.as_deref())?;
        hygiene_report(&root, false)
    }

    /// Plans or applies generated artifact cleanup.
    pub fn hygiene_clean(
        &self,
        request: HygieneCleanRequest,
    ) -> Result<HygieneReport, RepoctlError> {
        let root = locate_repo_path(request.repo.as_deref())?;
        let report = hygiene_report(&root, true)?;
        if !request.dry_run {
            for operation in &report.operations {
                let target = root.join(operation.path.as_str());
                if target.is_dir() {
                    fs::remove_dir_all(&target).map_err(|source| {
                        RepoctlError::Environment(format!(
                            "failed to remove `{}`: {source}",
                            target.display()
                        ))
                    })?;
                } else if target.is_file() {
                    fs::remove_file(&target).map_err(|source| {
                        RepoctlError::Environment(format!(
                            "failed to remove `{}`: {source}",
                            target.display()
                        ))
                    })?;
                }
            }
        }
        Ok(report)
    }

    /// Creates a reviewed adoption plan.
    pub fn adopt_plan(&self, request: AdoptionPlanRequest) -> Result<AdoptionPlan, RepoctlError> {
        plan_adoption(&request, self)
    }

    /// Applies a reviewed adoption plan.
    pub fn adopt_apply(&self, request: AdoptionApplyRequest) -> Result<AdoptionPlan, RepoctlError> {
        if request.refresh {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.refresh.unsupported",
                "refresh is not supported by apply; rerun adopt plan and review the new plan",
            )));
        }
        let plan = read_adoption_plan(&request.plan)?;
        apply_adoption_plan(&plan)?;
        Ok(plan)
    }

    /// Verifies a reviewed adoption plan without copying files.
    pub fn adopt_verify(
        &self,
        request: AdoptionVerifyRequest,
    ) -> Result<ValidationReport, RepoctlError> {
        let plan = read_adoption_plan(&request.plan)?;
        let report = self.validate_graph(GraphValidateRequest {
            repo: Some(plan.dest_root.as_std_path().to_path_buf()),
            changed_files: Vec::new(),
            mode: ValidationMode::Structural,
        })?;
        let hygiene = self.hygiene_check(HygieneCheckRequest {
            repo: Some(plan.dest_root.as_std_path().to_path_buf()),
        })?;
        let mut diagnostics = report.diagnostics;
        diagnostics.extend(hygiene.diagnostics);
        Ok(ValidationReport::new(diagnostics))
    }

    /// Lists available templates.
    pub fn template_list(
        &self,
        request: TemplateListRequest,
    ) -> Result<TemplateListReport, RepoctlError> {
        self.scaffold.list_templates(&request)
    }

    /// Renders a template.
    pub fn template_render(
        &self,
        request: TemplateRenderRequest,
    ) -> Result<RenderPlan, RepoctlError> {
        self.scaffold.render_template(&request)
    }

    /// Checks generated-code direct edits.
    pub fn codegen_check(
        &self,
        request: CodegenCheckRequest,
    ) -> Result<CodegenCheckReport, RepoctlError> {
        self.runner.codegen_check(&request)
    }

    /// Runs syntax-aware code-size inspection.
    pub fn inspect_code_size(
        &self,
        request: CodeSizeInspectionRequest,
    ) -> Result<CodeSizeInspectionReport, RepoctlError> {
        self.inspector.inspect_size(&request)
    }

    /// Builds a PR summary.
    pub fn pr_summary(&self, request: PrSummaryRequest) -> Result<PrSummary, RepoctlError> {
        let mut summary = self.runner.pr_summary(&request)?;
        let inspection = self.inspector.inspect_size(&CodeSizeInspectionRequest {
            repo: request.repo,
            scope: CodeSizeScope::Changed,
            base: request.base,
            head: request.head,
            changed_files: request.changed_files,
            include_transitive: false,
            languages: Vec::new(),
            rules: Vec::new(),
            fail_on: InspectionFailOn::Never,
        })?;
        append_code_size_pr_section(&mut summary, &inspection);
        Ok(summary)
    }

    /// Builds AI context.
    pub fn ai_context(&self, request: AiContextRequest) -> Result<AiContext, RepoctlError> {
        self.runner.ai_context(&request)
    }

    /// Returns the proto facade.
    pub fn proto(&self) -> &ProtoFacade {
        &self.proto
    }

    /// Returns the `IaC` facade.
    pub fn iac(&self) -> &IacFacade {
        &self.iac
    }

    /// Builds an operations plan.
    pub fn ops_plan(&self, request: OpsPlanRequest) -> Result<OpsPlan, RepoctlError> {
        self.runner.ops_plan(&request)
    }

    /// Builds a non-mutating operations verification report.
    pub fn ops_verify(&self, request: OpsVerifyRequest) -> Result<OpsVerifyReport, RepoctlError> {
        self.runner.ops_verify(&request)
    }

    /// Builds a manual-state reconciliation report.
    pub fn ops_reconcile(
        &self,
        request: OpsReconcileRequest,
    ) -> Result<OpsReconcileReport, RepoctlError> {
        self.runner.ops_reconcile(&request)
    }

    /// Manages local operations session journals.
    pub fn ops_journal(
        &self,
        request: OpsJournalRequest,
    ) -> Result<OpsJournalReport, RepoctlError> {
        self.runner.ops_journal(&request)
    }

    /// Inspects provider capabilities.
    pub fn provider_capabilities(
        &self,
        request: ProviderCapabilityRequest,
    ) -> Result<Vec<ProviderCapabilityReport>, RepoctlError> {
        self.runner.provider_capabilities(&request)
    }

    /// Runs read-only repository health checks.
    pub fn doctor(&self, request: DoctorRequest) -> DoctorReport {
        let repo = request.repo.clone();
        let changed_files = request.changed_files.clone();
        let base = request.base.clone();
        let head = request.head.clone();
        let tasks = if request.tasks.is_empty() {
            default_doctor_tasks()
        } else {
            request.tasks.clone()
        };

        if let Err(error) = locate_repo_path(repo.as_deref()) {
            let diagnostics = diagnostics_from_error(error);
            return DoctorReport {
                status: DoctorStatus::Blocked,
                command_succeeded: false,
                has_errors: true,
                sections: vec![DoctorSection {
                    name: "repo".to_string(),
                    status: DoctorStatus::Blocked,
                    summary: "repoctl repo root could not be located".to_string(),
                    diagnostics,
                }],
            };
        }

        let snapshot = match self.discover(DiscoverRequest { repo: repo.clone() }) {
            Ok(snapshot) => Arc::new(snapshot),
            Err(error) => {
                let section = doctor_section("graph", Err(error));
                return DoctorReport {
                    status: section.status.clone(),
                    command_succeeded: section.status != DoctorStatus::Blocked,
                    has_errors: true,
                    sections: vec![section],
                };
            }
        };
        let scoped = self.with_snapshot(snapshot);
        let sections = scoped.run_doctor_sections(repo, base, head, changed_files, tasks);

        let has_blocked = sections
            .iter()
            .any(|section| section.status == DoctorStatus::Blocked);
        let has_errors = sections.iter().any(|section| {
            section
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        });
        let has_diagnostics = sections
            .iter()
            .any(|section| !section.diagnostics.is_empty());
        let status = if has_blocked {
            DoctorStatus::Blocked
        } else if has_errors || has_diagnostics {
            DoctorStatus::Diagnostics
        } else {
            DoctorStatus::Ok
        };
        DoctorReport {
            status,
            command_succeeded: !has_blocked,
            has_errors,
            sections,
        }
    }

    fn with_snapshot(&self, snapshot: Arc<RepoSnapshot>) -> Self {
        let engine = RepoctlEngine::from_snapshot(snapshot);
        let runner = RunnerService::new(
            engine.clone(),
            Arc::new(repoctl_runner::LocalGitProvider),
            Arc::new(repoctl_runner::LocalProcessRunner),
        );
        Self {
            engine: engine.clone(),
            scaffold: self.scaffold.clone(),
            inspector: InspectorService::new(engine, runner.clone()),
            proto: ProtoFacade {
                runner: runner.clone(),
            },
            iac: IacFacade {
                runner: runner.clone(),
            },
            runner,
            skills: self.skills.clone(),
        }
    }

    fn run_doctor_sections(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
        tasks: Vec<TaskName>,
    ) -> Vec<DoctorSection> {
        thread::scope(|scope| {
            let handles = vec![
                (
                    "graph",
                    scope.spawn({
                        let repo = repo.clone();
                        let changed_files = changed_files.clone();
                        move || self.doctor_graph_section(repo, changed_files)
                    }),
                ),
                (
                    "affected",
                    scope.spawn({
                        let repo = repo.clone();
                        let base = base.clone();
                        let head = head.clone();
                        let changed_files = changed_files.clone();
                        move || self.doctor_affected_section(repo, base, head, changed_files, tasks)
                    }),
                ),
                (
                    "skills",
                    scope.spawn({
                        let repo = repo.clone();
                        move || self.doctor_skills_section(repo)
                    }),
                ),
                (
                    "hygiene",
                    scope.spawn({
                        let repo = repo.clone();
                        move || self.doctor_hygiene_section(repo)
                    }),
                ),
                (
                    "codegen",
                    scope.spawn({
                        let repo = repo.clone();
                        let base = base.clone();
                        let head = head.clone();
                        let changed_files = changed_files.clone();
                        move || self.doctor_codegen_section(repo, base, head, changed_files)
                    }),
                ),
                (
                    "inspect-size",
                    scope.spawn({
                        let repo = repo.clone();
                        let base = base.clone();
                        let head = head.clone();
                        let changed_files = changed_files.clone();
                        move || self.doctor_inspect_size_section(repo, base, head, changed_files)
                    }),
                ),
                (
                    "proto",
                    scope.spawn({
                        let repo = repo.clone();
                        let base = base.clone();
                        let head = head.clone();
                        let changed_files = changed_files.clone();
                        move || self.doctor_proto_section(repo, base, head, changed_files)
                    }),
                ),
                (
                    "iac",
                    scope.spawn({
                        move || self.doctor_iac_section(repo, base, head, changed_files)
                    }),
                ),
            ];

            handles
                .into_iter()
                .map(|(name, handle)| match handle.join() {
                    Ok(section) => section,
                    Err(_) => doctor_worker_panic_section(name),
                })
                .collect()
        })
    }

    fn doctor_graph_section(
        &self,
        repo: Option<PathBuf>,
        changed_files: Vec<RepoRelativePath>,
    ) -> DoctorSection {
        doctor_section(
            "graph",
            self.validate_graph(GraphValidateRequest {
                repo,
                changed_files,
                mode: ValidationMode::Metadata,
            })
            .map(|report| (report.diagnostics, "graph metadata validation".to_string())),
        )
    }

    fn doctor_affected_section(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
        tasks: Vec<TaskName>,
    ) -> DoctorSection {
        doctor_section(
            "affected",
            self.affected(AffectedRequest {
                repo,
                base,
                head,
                changed_files,
                tasks,
            })
            .map(|report| {
                (
                    report.diagnostics,
                    format!(
                        "{} affected projects, {} tasks",
                        report.directly_affected.len() + report.transitively_affected.len(),
                        report.tasks.len()
                    ),
                )
            }),
        )
    }

    fn doctor_skills_section(&self, repo: Option<PathBuf>) -> DoctorSection {
        doctor_section(
            "skills",
            self.skills()
                .check(SkillsFacadeRequest {
                    repo,
                    sync: false,
                    dry_run: false,
                })
                .map(|report| {
                    (
                        report.diagnostics,
                        format!("{} skill files drifted", report.diffs.len()),
                    )
                }),
        )
    }

    fn doctor_hygiene_section(&self, repo: Option<PathBuf>) -> DoctorSection {
        doctor_section(
            "hygiene",
            self.hygiene_check(HygieneCheckRequest { repo })
                .map(|report| {
                    let diagnostics = report
                        .diagnostics
                        .into_iter()
                        .filter(is_doctor_actionable_hygiene_diagnostic)
                        .collect::<Vec<_>>();
                    (diagnostics, "repository metadata leakage".to_string())
                }),
        )
    }

    fn doctor_codegen_section(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
    ) -> DoctorSection {
        doctor_section(
            "codegen",
            self.codegen_check(CodegenCheckRequest {
                repo,
                base,
                head,
                changed_files,
            })
            .map(|report| (report.diagnostics, "generated-code policy".to_string())),
        )
    }

    fn doctor_inspect_size_section(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
    ) -> DoctorSection {
        let has_change_scope = !changed_files.is_empty() || (base.is_some() && head.is_some());
        if !has_change_scope {
            return DoctorSection {
                name: "inspect-size".to_string(),
                status: DoctorStatus::Ok,
                summary: "skipped; pass --base/--head or --changed-file to inspect changed code \
                          size"
                    .to_string(),
                diagnostics: Vec::new(),
            };
        }
        doctor_section(
            "inspect-size",
            self.inspect_code_size(CodeSizeInspectionRequest {
                repo,
                scope: CodeSizeScope::Changed,
                base,
                head,
                changed_files,
                include_transitive: false,
                languages: Vec::new(),
                rules: Vec::new(),
                fail_on: InspectionFailOn::Never,
            })
            .map(|report| {
                let mut diagnostics = report.diagnostics;
                diagnostics.extend(report.findings.into_iter().map(|finding| {
                    Diagnostic::warning("inspect.code_size.finding", finding.message)
                        .with_path(finding.path.to_string())
                }));
                (
                    diagnostics,
                    format!("{} findings", report.summary.finding_count),
                )
            }),
        )
    }

    fn doctor_proto_section(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
    ) -> DoctorSection {
        doctor_section(
            "proto",
            self.proto()
                .check(ProtoFacadeRequest {
                    against: None,
                    execute: false,
                    repo,
                    operation: ProtoOperation::Check,
                    selector: None,
                    base,
                    head,
                    changed_files,
                })
                .map(|report| (report.diagnostics, "proto routing and policy".to_string())),
        )
    }

    fn doctor_iac_section(
        &self,
        repo: Option<PathBuf>,
        base: Option<String>,
        head: Option<String>,
        changed_files: Vec<RepoRelativePath>,
    ) -> DoctorSection {
        doctor_section(
            "iac",
            self.iac()
                .plan(IacFacadeRequest {
                    repo,
                    affected: true,
                    project: None,
                    env: Some("staging".to_string()),
                    core: false,
                    base,
                    head,
                    changed_files,
                    dry_run: true,
                })
                .map(|report| {
                    (
                        report.diagnostics,
                        format!("{} dry-run preview commands", report.commands.len()),
                    )
                }),
        )
    }

    /// Returns the skills facade.
    pub fn skills(&self) -> &SkillsFacade {
        &self.skills
    }
}

/// Proto command facade.
#[derive(Clone, Debug, Default)]
pub struct ProtoFacade {
    runner: RunnerService,
}

impl ProtoFacade {
    /// Runs a proto command family.
    pub fn run(&self, request: ProtoFacadeRequest) -> Result<ProtoFacadeReport, RepoctlError> {
        self.runner.proto(&request)
    }

    /// Checks proto toolchain and generated-code policy.
    pub fn check(
        &self,
        mut request: ProtoFacadeRequest,
    ) -> Result<ProtoFacadeReport, RepoctlError> {
        request.operation = ProtoOperation::Check;
        self.runner.proto(&request)
    }

    /// Looks up proto owners.
    pub fn owners(
        &self,
        mut request: ProtoFacadeRequest,
    ) -> Result<ProtoFacadeReport, RepoctlError> {
        request.operation = ProtoOperation::Owners;
        self.runner.proto(&request)
    }

    /// Looks up proto consumers.
    pub fn consumers(
        &self,
        mut request: ProtoFacadeRequest,
    ) -> Result<ProtoFacadeReport, RepoctlError> {
        request.operation = ProtoOperation::Consumers;
        self.runner.proto(&request)
    }
}

/// `IaC` command facade.
#[derive(Clone, Debug, Default)]
pub struct IacFacade {
    runner: RunnerService,
}

impl IacFacade {
    /// Plans `IaC` provider commands.
    pub fn plan(&self, request: IacFacadeRequest) -> Result<IacFacadeReport, RepoctlError> {
        self.runner.iac_plan(&request)
    }
}

/// Skills command facade.
#[derive(Clone, Debug, Default)]
pub struct SkillsFacade;

impl SkillsFacade {
    /// Placeholder for skills command families.
    pub fn check(&self, request: SkillsFacadeRequest) -> Result<SkillsFacadeReport, RepoctlError> {
        ScaffoldService::with_default_adapters().skills(&request)
    }

    /// Computes generated skill drift.
    pub fn diff(&self, request: SkillsFacadeRequest) -> Result<SkillsFacadeReport, RepoctlError> {
        ScaffoldService::with_default_adapters().skills(&request)
    }

    /// Synchronizes generated skills.
    pub fn sync(
        &self,
        mut request: SkillsFacadeRequest,
    ) -> Result<SkillsFacadeReport, RepoctlError> {
        request.sync = true;
        ScaffoldService::with_default_adapters().skills(&request)
    }
}

fn structural_discovery() -> DiscoveryService {
    DiscoveryService::new(
        Arc::new(DefaultRepoLocator),
        Arc::new(LocalRepoFileSystem),
        Arc::new(repoctl_core::YamlManifestParser),
        Arc::new(DefaultGraphBuilder::new(Vec::new())),
    )
}

fn default_doctor_tasks() -> Vec<TaskName> {
    ["check", "test", "build"]
        .into_iter()
        .filter_map(|task| TaskName::new(task).ok())
        .collect()
}

fn doctor_section(
    name: &str,
    result: Result<(Vec<Diagnostic>, String), RepoctlError>,
) -> DoctorSection {
    match result {
        Ok((diagnostics, summary)) => {
            let status = if diagnostics.is_empty() {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Diagnostics
            };
            DoctorSection {
                name: name.to_string(),
                status,
                summary,
                diagnostics,
            }
        }
        Err(error) => DoctorSection {
            name: name.to_string(),
            status: DoctorStatus::Blocked,
            summary: "section could not run".to_string(),
            diagnostics: diagnostics_from_error(error),
        },
    }
}

fn doctor_worker_panic_section(name: &str) -> DoctorSection {
    DoctorSection {
        name: name.to_string(),
        status: DoctorStatus::Blocked,
        summary: "section worker panicked".to_string(),
        diagnostics: vec![Diagnostic::error(
            "doctor.section.worker_panic",
            format!("doctor section `{name}` panicked while running"),
        )],
    }
}

fn is_doctor_actionable_hygiene_diagnostic(diagnostic: &Diagnostic) -> bool {
    if diagnostic.code.as_ref() != "adoption.hygiene.generated_artifact" {
        return true;
    }
    diagnostic.source.as_ref().is_some_and(|source| {
        source
            .path
            .rsplit('/')
            .next()
            .is_some_and(|name| matches!(name, ".git" | ".github"))
    })
}

fn diagnostics_from_error(error: RepoctlError) -> Vec<Diagnostic> {
    match error {
        RepoctlError::Diagnostic { diagnostic } => vec![*diagnostic],
        RepoctlError::Diagnostics { diagnostics } => diagnostics,
        RepoctlError::Io { path, source } => vec![
            Diagnostic::error(
                "doctor.section.io",
                format!("I/O error at {path}: {source}"),
            )
            .with_path(path.to_string()),
        ],
        RepoctlError::Environment(message) => {
            vec![Diagnostic::error("doctor.section.environment", message)]
        }
        RepoctlError::Internal(message) => {
            vec![Diagnostic::error("doctor.section.internal", message)]
        }
    }
}

const HYGIENE_GENERATED_DIRS: &[&str] = &[
    ".git",
    ".github",
    "node_modules",
    "target",
    "dist",
    ".next",
    ".turbo",
    ".cache",
];

fn locate_repo_path(start: Option<&Path>) -> Result<PathBuf, RepoctlError> {
    let mut current = match start {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|source| RepoctlError::Environment(format!("failed to read cwd: {source}")))?,
    };
    if current.is_file() {
        current.pop();
    }
    loop {
        if current.join("repo.yaml").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(RepoctlError::diagnostic(
                Diagnostic::error(
                    "repo.locate",
                    "could not find repo.yaml from the requested path",
                )
                .with_help("run this command from a repoctl-managed repository or pass --repo"),
            ));
        }
    }
}

fn write_utf8_file(
    root: &Path,
    path: &RepoRelativePath,
    content: &str,
) -> Result<(), RepoctlError> {
    let target = root.join(path.as_str());
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| RepoctlError::io(parent.display().to_string(), source))?;
    }
    fs::write(&target, content)
        .map_err(|source| RepoctlError::io(target.display().to_string(), source))
}

fn hygiene_report(root: &Path, cleanable: bool) -> Result<HygieneReport, RepoctlError> {
    let mut diagnostics = Vec::new();
    let mut operations = Vec::new();
    collect_hygiene(root, root, cleanable, &mut diagnostics, &mut operations)?;
    Ok(HygieneReport {
        diagnostics,
        operations,
    })
}

fn collect_hygiene(
    root: &Path,
    current: &Path,
    cleanable: bool,
    diagnostics: &mut Vec<Diagnostic>,
    operations: &mut Vec<FileOperation>,
) -> Result<(), RepoctlError> {
    for entry in fs::read_dir(current)
        .map_err(|source| RepoctlError::io(current.display().to_string(), source))?
    {
        let entry =
            entry.map_err(|source| RepoctlError::io(current.display().to_string(), source))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let rel = path.strip_prefix(root).map_err(|_| {
            RepoctlError::diagnostic(Diagnostic::error(
                "hygiene.path",
                "hygiene path escaped repo root",
            ))
        })?;
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        let nested_root = !rel_text.is_empty();
        if path.is_dir() && HYGIENE_GENERATED_DIRS.contains(&name) && nested_root {
            if name == ".git" && rel_text == ".git" {
                continue;
            }
            if name == ".github" && rel_text == ".github" {
                collect_hygiene(root, &path, cleanable, diagnostics, operations)?;
                continue;
            }
            diagnostics.push(
                Diagnostic::warning(
                    "adoption.hygiene.generated_artifact",
                    format!("generated or repository metadata directory `{rel_text}` is present"),
                )
                .with_path(&rel_text),
            );
            if cleanable && name != ".github" && name != ".git" {
                operations.push(FileOperation {
                    path: RepoRelativePath::new(rel_text.clone())
                        .map_err(RepoctlError::diagnostic)?,
                    operation: "remove-generated".to_string(),
                    content: None,
                });
            }
            continue;
        }
        if path.is_dir() {
            collect_hygiene(root, &path, cleanable, diagnostics, operations)?;
        }
    }
    Ok(())
}

fn append_code_size_pr_section(summary: &mut PrSummary, report: &CodeSizeInspectionReport) {
    if !report.findings.is_empty() {
        summary.markdown.push_str("\n## Code Size\n");
        for finding in &report.findings {
            let _ = writeln!(
                summary.markdown,
                "- `{}`: {}",
                finding.path, finding.message
            );
        }
    }
    if let Value::Object(object) = &mut summary.impact {
        object.insert(
            "codeSize".to_string(),
            serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({})),
        );
    }
}

#[cfg(test)]
// End-to-end operation fixtures keep their declarative input next to assertions.
mod tests {
    // Facade tests build temporary repositories synchronously before invoking sync services.
    #![allow(clippy::disallowed_methods, clippy::too_many_lines)]

    use std::{fs, path::Path};

    use super::{
        AdoptionApplyRequest, AdoptionCiMode, AdoptionOutputFormat, AdoptionPlanRequest,
        AiContextRequest, CodegenCheckRequest, DependencyRewriteMode, DiscoverRequest,
        DoctorRequest, DoctorStatus, GraphValidateRequest, HygieneCheckRequest, IacFacadeRequest,
        OpsPlanRequest, OpsReconcileRequest, OpsVerifyRequest, PrSummaryRequest, ProjectName,
        ProtoFacadeRequest, ProtoOperation, ProviderCapabilityRequest, RepoRelativePath, Repoctl,
        TaskName, TaskRunRequest, TemplateListRequest, TemplateRenderRequest, TemplateSource,
        ValidationMode,
    };

    #[test]
    fn test_should_discover_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_fixture(temp.path(), None);
        let facade = Repoctl::with_default_adapters().expect("facade");
        let snapshot = facade
            .discover(DiscoverRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect("snapshot");
        assert_eq!(snapshot.projects.len(), 1);
    }

    #[test]
    fn test_should_validate_graph_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_fixture(temp.path(), Some("apps.other"));
        let facade = Repoctl::with_default_adapters().expect("facade");
        let report = facade
            .validate_graph(GraphValidateRequest {
                repo: Some(temp.path().to_path_buf()),
                changed_files: Vec::new(),
                mode: ValidationMode::Structural,
            })
            .expect("validation report");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "policy.cross_app_dependency")
        );
    }

    #[test]
    fn test_should_aggregate_doctor_sections_without_reporting_root_git() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_fixture(temp.path(), None);
        fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        fs::write(
            temp.path().join("apps/catalog/src.rs"),
            "pub fn catalog() {}\n",
        )
        .expect("source");
        let facade = Repoctl::with_default_adapters().expect("facade");

        let report = facade.doctor(DoctorRequest {
            repo: Some(temp.path().to_path_buf()),
            base: None,
            head: None,
            changed_files: vec![RepoRelativePath::new("apps/catalog/src.rs").expect("path")],
            tasks: Vec::new(),
            agent: true,
        });

        assert!(report.command_succeeded);
        assert_eq!(report.status, DoctorStatus::Diagnostics);
        assert_eq!(
            report
                .sections
                .iter()
                .map(|section| section.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "graph",
                "affected",
                "skills",
                "hygiene",
                "codegen",
                "inspect-size",
                "proto",
                "iac",
            ]
        );
        assert!(
            report
                .sections
                .iter()
                .any(|section| section.name == "skills")
        );
        assert!(
            !report
                .sections
                .iter()
                .flat_map(|section| &section.diagnostics)
                .any(|diagnostic| diagnostic
                    .source
                    .as_ref()
                    .is_some_and(|source| source.path.as_ref() == ".git"))
        );
    }

    #[test]
    fn test_should_not_report_workspace_generated_dirs_in_doctor_hygiene() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_fixture(temp.path(), None);
        fs::create_dir_all(temp.path().join("apps/catalog/frontend/node_modules"))
            .expect("node_modules dir");
        fs::create_dir_all(temp.path().join("apps/catalog/frontend/dist")).expect("dist dir");
        let facade = Repoctl::with_default_adapters().expect("facade");

        let report = facade.doctor(DoctorRequest {
            repo: Some(temp.path().to_path_buf()),
            base: None,
            head: None,
            changed_files: Vec::new(),
            tasks: Vec::new(),
            agent: true,
        });

        let hygiene = report
            .sections
            .iter()
            .find(|section| section.name == "hygiene")
            .expect("hygiene section");
        assert_eq!(hygiene.diagnostics.len(), 0);
    }

    #[test]
    fn test_should_skip_doctor_code_size_without_change_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_fixture(temp.path(), None);
        let facade = Repoctl::with_default_adapters().expect("facade");

        let report = facade.doctor(DoctorRequest {
            repo: Some(temp.path().to_path_buf()),
            base: None,
            head: None,
            changed_files: Vec::new(),
            tasks: Vec::new(),
            agent: true,
        });

        let inspect_size = report
            .sections
            .iter()
            .find(|section| section.name == "inspect-size")
            .expect("inspect-size section");
        assert_eq!(inspect_size.status, DoctorStatus::Ok);
        assert_eq!(inspect_size.diagnostics.len(), 0);
        assert!(inspect_size.summary.contains("skipped"));
    }

    #[test]
    fn test_should_report_missing_repo_manifest_once_for_doctor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let facade = Repoctl::with_default_adapters().expect("facade");

        let report = facade.doctor(DoctorRequest {
            repo: Some(temp.path().to_path_buf()),
            base: None,
            head: None,
            changed_files: Vec::new(),
            tasks: Vec::new(),
            agent: true,
        });

        assert!(!report.command_succeeded);
        assert_eq!(report.status, DoctorStatus::Blocked);
        assert_eq!(report.sections.len(), 1);
        assert_eq!(report.sections[0].name, "repo");
        assert!(
            report.sections[0]
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "repo.locate")
        );
    }

    #[test]
    fn test_should_plan_apply_validate_and_check_hygiene_for_adoption() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        write_adoption_fixture(&source, &dest);
        let facade = Repoctl::with_default_adapters().expect("facade");

        let plan = facade
            .adopt_plan(AdoptionPlanRequest {
                source: source.clone(),
                dest: dest.clone(),
                exclude: vec!["synapse".to_string()],
                rewrite_deps: DependencyRewriteMode::Auto,
                ci: AdoptionCiMode::Update,
                verification: ValidationMode::Metadata,
                format: AdoptionOutputFormat::Json,
                ..AdoptionPlanRequest::default()
            })
            .expect("adoption plan");
        assert!(
            plan.sources
                .iter()
                .any(|source| source.name == "synapse" && source.skipped)
        );
        assert!(plan.sources.iter().any(|source| {
            source.name == "operon" && source.destination_path.as_str() == "frameworks/operon"
        }));
        assert!(
            plan.dependency_rewrites
                .iter()
                .any(|rewrite| rewrite.package == "operon")
        );

        let plan_path = temp.path().join("plan.json");
        fs::write(
            &plan_path,
            serde_json::to_vec_pretty(&plan).expect("serialize plan"),
        )
        .expect("write plan");
        facade
            .adopt_apply(AdoptionApplyRequest {
                plan: plan_path,
                refresh: false,
            })
            .expect("apply plan");

        assert!(dest.join("frameworks/operon/project.yaml").is_file());
        assert!(dest.join("apps/golgi/project.yaml").is_file());
        assert!(!dest.join("synapse").exists());
        assert!(!dest.join("apps/golgi/.git").exists());
        assert!(!dest.join("apps/golgi/node_modules").exists());
        let cargo = fs::read_to_string(dest.join("apps/golgi/Cargo.toml")).expect("read cargo");
        assert!(cargo.contains("path = \"../../frameworks/operon\""));

        let validation = facade
            .validate_graph(GraphValidateRequest {
                repo: Some(dest.clone()),
                changed_files: Vec::new(),
                mode: ValidationMode::Structural,
            })
            .expect("validate adopted dest");
        assert!(validation.is_success());
        let hygiene = facade
            .hygiene_check(HygieneCheckRequest { repo: Some(dest) })
            .expect("hygiene");
        assert_eq!(hygiene.diagnostics.len(), 0);
    }

    #[test]
    fn test_should_resolve_proto_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_phase789_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let report = facade
            .proto()
            .owners(ProtoFacadeRequest {
                against: None,
                execute: false,
                repo: Some(temp.path().to_path_buf()),
                operation: ProtoOperation::Owners,
                selector: Some("protos/acme/identity/v1/identity.proto".to_string()),
                base: None,
                head: None,
                changed_files: Vec::new(),
            })
            .expect("proto owners");
        assert!(
            report
                .owners
                .iter()
                .any(|owner| owner.as_str() == "foundations.identity")
        );
    }

    #[test]
    fn test_should_build_ai_context_and_pr_summary_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_phase789_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let context = facade
            .ai_context(AiContextRequest {
                changed_files: Vec::new(),
                include_docs: false,
                max_bytes: 65_536,
                repo: Some(temp.path().to_path_buf()),
                project: ProjectName::new("apps.catalog").expect("project"),
                audience: "ai".to_string(),
            })
            .expect("context");
        assert_eq!(context.payload["project"], "apps.catalog");
        let summary = facade
            .pr_summary(PrSummaryRequest {
                repo: Some(temp.path().to_path_buf()),
                base: None,
                head: None,
                changed_files: vec![
                    RepoRelativePath::new("apps/catalog/iac/stacks/prod.yaml").expect("path"),
                ],
            })
            .expect("summary");
        assert!(summary.markdown.contains("prod-iac"));
        assert_eq!(summary.impact["affected"]["riskFlags"][0], "prod-iac");
    }

    #[test]
    fn test_should_plan_iac_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_phase789_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let report = facade
            .iac()
            .plan(IacFacadeRequest {
                repo: Some(temp.path().to_path_buf()),
                affected: false,
                project: Some(ProjectName::new("apps.catalog").expect("project")),
                env: Some("dev".to_string()),
                core: false,
                base: None,
                head: None,
                changed_files: Vec::new(),
                dry_run: true,
            })
            .expect("iac plan");
        assert_eq!(report.commands[0].program, "pulumi");
        assert_eq!(report.commands[0].cwd.as_str(), "apps/catalog/iac");
    }

    #[test]
    fn test_should_plan_operations_session_review_surface() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_universe_ops_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let changed_files =
            vec![RepoRelativePath::new("frameworks/operon/operon-infra/index.ts").expect("path")];

        let affected = facade
            .affected(repoctl_core::AffectedRequest {
                repo: Some(temp.path().to_path_buf()),
                base: None,
                head: None,
                changed_files: changed_files.clone(),
                tasks: vec![TaskName::new("check").expect("task")],
            })
            .expect("affected");
        let unique_tasks = affected
            .tasks
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(affected.tasks.len(), unique_tasks.len());
        assert!(
            affected
                .tasks
                .iter()
                .any(|task| task == "apps.slides:infra:check")
        );

        let task_report = facade
            .run_task(TaskRunRequest {
                repo: Some(temp.path().to_path_buf()),
                tasks: vec![TaskName::new("check").expect("task")],
                affected: true,
                changed_files: changed_files.clone(),
                dry_run: true,
                ..TaskRunRequest::default()
            })
            .expect("task dry-run");
        assert!(task_report.commands.iter().any(|command| {
            command.project.as_ref().is_some_and(|project| {
                project.as_str() == "apps.slides" && command.task.as_ref().is_some()
            })
        }));

        let plan_path = temp.path().join("target/repoctl/ops-plan.json");
        let plan = facade
            .ops_plan(OpsPlanRequest {
                repo: Some(temp.path().to_path_buf()),
                changed_files: changed_files.clone(),
                environments: vec!["staging".to_string()],
                tasks: vec![TaskName::new("check").expect("task")],
                output: Some(plan_path.clone()),
                ..OpsPlanRequest::default()
            })
            .expect("ops plan");
        assert!(plan_path.is_file());
        assert!(plan.iac.windows(2).any(|window| window[0].workspace
            == "frameworks/operon/operon-infra"
            && window[1].workspace == "apps/slides/infra"));
        assert!(plan.dns.iter().any(
            |operation| operation.record == "slides.dev.int.iostream.app"
                && operation.expected_proxied == Some(false)
        ));
        assert!(
            plan.cdn
                .iter()
                .any(|check| check.provider == "aws-cloudfront")
        );
        assert!(
            plan.provider_capabilities
                .iter()
                .any(|report| report.status == "missing"
                    && report.field == "invokedViaFunctionUrl")
        );
        assert!(
            plan.probes
                .iter()
                .any(|probe| probe.name == "ligand-health")
        );
        assert!(
            plan.manual_reconciliation
                .iter()
                .any(|record| record.kind == "manual.lambda.add-permission")
        );

        let verify = facade
            .ops_verify(OpsVerifyRequest {
                repo: Some(temp.path().to_path_buf()),
                plan: plan_path.clone(),
            })
            .expect("ops verify");
        assert_ne!(verify.commands.len(), 0);
        assert_ne!(verify.skipped_mutating_commands.len(), 0);

        let reconcile = facade
            .ops_reconcile(OpsReconcileRequest {
                repo: Some(temp.path().to_path_buf()),
                plan: plan_path,
            })
            .expect("ops reconcile");
        assert!(
            reconcile
                .cleanup_commands
                .iter()
                .any(|command| command.program == "aws")
        );

        let capabilities = facade
            .provider_capabilities(ProviderCapabilityRequest {
                repo: Some(temp.path().to_path_buf()),
                workspace: Some("frameworks.operon:infra".to_string()),
                changed_files,
                ..ProviderCapabilityRequest::default()
            })
            .expect("provider capabilities");
        assert_eq!(capabilities[0].status, "missing");

        let summary = facade
            .pr_summary(PrSummaryRequest {
                repo: Some(temp.path().to_path_buf()),
                changed_files: vec![
                    RepoRelativePath::new("frameworks/operon/operon-infra/index.ts").expect("path"),
                ],
                ..PrSummaryRequest::default()
            })
            .expect("pr summary");
        assert!(summary.markdown.contains("Deploy Surface"));
        assert!(summary.markdown.contains("CloudFront"));
    }

    #[test]
    fn test_should_list_render_templates_and_check_codegen_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_phase789_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let templates = facade
            .template_list(TemplateListRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect("template list");
        assert!(
            templates
                .templates
                .iter()
                .any(|template| template.source == "builtin:app")
        );
        let plan = facade
            .template_render(TemplateRenderRequest {
                repo: Some(temp.path().to_path_buf()),
                source: TemplateSource::Builtin {
                    name: "app".to_string(),
                },
                inputs: serde_json::json!({ "name": "catalog-template" }),
                dry_run: true,
            })
            .expect("template render");
        assert_eq!(
            plan.operations[0].path.as_str(),
            "catalog-template/README.md"
        );
        let codegen = facade
            .codegen_check(CodegenCheckRequest {
                repo: Some(temp.path().to_path_buf()),
                base: None,
                head: None,
                changed_files: vec![
                    RepoRelativePath::new("apps/catalog/api/generated/client.rs").expect("path"),
                ],
            })
            .expect("codegen check");
        assert!(
            codegen
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "policy.generated_code_readonly")
        );
    }

    fn write_fixture(root: &Path, dependency: Option<&str>) {
        fs::write(
            root.join("repo.yaml"),
            r#"
schema: company.repo/v1
name: acme
layout: functional
defaults:
  owner: "@platform"
"#,
        )
        .expect("repo manifest");
        fs::create_dir_all(root.join("apps/catalog")).expect("catalog dir");
        fs::write(
            root.join("apps/catalog/project.yaml"),
            format!(
                r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
owners:
  - "@catalog"
{}
"#,
                dependency.map_or("depends_on: []".to_string(), |name| format!(
                    "depends_on:\n  - {name}"
                ),)
            ),
        )
        .expect("catalog manifest");
        if dependency.is_some() {
            fs::create_dir_all(root.join("apps/other")).expect("other dir");
            fs::write(
                root.join("apps/other/project.yaml"),
                r#"
schema: company.project/v1
name: apps.other
kind: app
path: apps/other
owners:
  - "@other"
"#,
            )
            .expect("other manifest");
        }
    }

    fn write_phase789_fixture(root: &Path) {
        fs::write(
            root.join("repo.yaml"),
            r#"
schema: company.repo/v1
name: acme
layout: functional
defaults:
  owner: "@platform"
protos:
  root: protos
  generated_code_policy: consumer-local
policies:
  prod_change:
    required_owners:
      - "@platform"
      - "@security"
"#,
        )
        .expect("repo manifest");
        fs::create_dir_all(root.join("protos")).expect("protos dir");
        fs::write(
            root.join("protos/project.yaml"),
            r#"
schema: company.project/v1
name: protos.shared
kind: proto-root
path: protos
owners:
  - "@platform"
"#,
        )
        .expect("proto project");
        fs::create_dir_all(root.join("apps/catalog/iac/stacks")).expect("catalog dir");
        fs::write(
            root.join("apps/catalog/project.yaml"),
            r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
owners:
  - "@catalog"
workspaces:
  - name: api
    language: rust
    root: api
    manifest: api/Cargo.toml
tasks:
  check:
    - workspace: api
      command: cargo check
protos:
  consumes:
    - protos/acme/identity/v1/**
iac:
  root: iac
  provider: pulumi
  stacks:
    - dev
    - prod
ai:
  editable:
    - api/**
    - iac/**
  do_not_edit:
    - "**/generated/**"
  docs:
    - README.md
"#,
        )
        .expect("catalog manifest");
        fs::create_dir_all(root.join("foundations/identity")).expect("identity dir");
        fs::write(
            root.join("foundations/identity/project.yaml"),
            r#"
schema: company.project/v1
name: foundations.identity
kind: foundation-service
path: foundations/identity
owners:
  - "@identity"
protos:
  owns:
    - protos/acme/identity/v1/**
"#,
        )
        .expect("identity manifest");
    }

    fn write_universe_ops_fixture(root: &Path) {
        fs::write(
            root.join("repo.yaml"),
            r#"
schema: company.repo/v1
name: universe
layout: functional
defaults:
  owner: "@platform"
iac:
  core_infra_root: core-infra
"#,
        )
        .expect("repo manifest");
        fs::create_dir_all(root.join("core-infra/nucleus")).expect("core dir");
        fs::write(
            root.join("core-infra/nucleus/Pulumi.yaml"),
            "name: nucleus\n",
        )
        .expect("core pulumi");

        fs::create_dir_all(root.join("frameworks/operon/operon-infra")).expect("operon infra");
        fs::write(
            root.join("frameworks/operon/operon-infra/package.json"),
            r#"{"dependencies":{"@pulumi/aws":"6.83.4"}}"#,
        )
        .expect("operon package");
        fs::write(
            root.join("frameworks/operon/operon-infra/index.ts"),
            "const permission = { invokedViaFunctionUrl: true };\n",
        )
        .expect("operon source");
        fs::write(
            root.join("frameworks/operon/project.yaml"),
            r#"
schema: company.project/v1
name: frameworks.operon
kind: framework
path: frameworks/operon
owners:
  - "@platform"
workspaces:
  - name: infra
    language: typescript
    toolchain: npm
    root: operon-infra
    manifest: operon-infra/package.json
tasks:
  check:
    - workspace: infra
      command: npm run check
iac:
  root: operon-infra
  provider: pulumi
  stacks:
    - staging
"#,
        )
        .expect("operon manifest");

        fs::create_dir_all(root.join("apps/slides/infra")).expect("slides infra");
        fs::write(root.join("apps/slides/infra/package.json"), "{}").expect("slides package");
        fs::write(
            root.join("apps/slides/project.yaml"),
            r#"
schema: company.project/v1
name: apps.slides
kind: app
path: apps/slides
owners:
  - "@slides"
depends_on:
  - frameworks.operon
workspaces:
  - name: infra
    language: typescript
    toolchain: npm
    root: infra
    manifest: infra/package.json
tasks:
  check:
    - workspace: infra
      command: npm run check
    - workspace: infra
      command: npm run check
iac:
  root: infra
  provider: pulumi
  stacks:
    - staging
dns:
  provider: cloudflare
  records:
    - name: slides.dev.int.iostream.app
      type: cname
      target:
        kind: cloudfront-distribution
        output: cdnDomainName
      proxied: false
      ttl: 300
cdn:
  provider: aws-cloudfront
  aliases:
    - slides.dev.int.iostream.app
  expected_response_headers:
    - "via: *CloudFront*"
ops:
  runtime_dependencies:
    - project: foundations.ligand
      endpoint: https://ligand.dev.int.iostream.app
      purpose: invitation verification
  probes:
    - name: slides-callback
      method: GET
      url: https://slides.dev.int.iostream.app/api/auth/google/callback?state=test&code=test
      expect:
        status: 401
        body_contains: missing cookie header
  manual_state:
    - kind: manual.lambda.add-permission
      resource: cellis-slides-staging-fn-url-invoke-public-manual
      status: pending-cleanup
      managed_equivalent: pulumi-nodejs:dynamic:Resource cellis-slides-staging-fn-url-invoke-public
      cleanup_command: aws lambda remove-permission --function-name cellis-slides-staging-fn --statement-id cellis-slides-staging-fn-url-invoke-public-manual
"#,
        )
        .expect("slides manifest");

        fs::create_dir_all(root.join("foundations/ligand/infra")).expect("ligand infra");
        fs::write(root.join("foundations/ligand/infra/package.json"), "{}")
            .expect("ligand package");
        fs::write(
            root.join("foundations/ligand/project.yaml"),
            r#"
schema: company.project/v1
name: foundations.ligand
kind: foundation-service
path: foundations/ligand
owners:
  - "@identity"
workspaces:
  - name: infra
    language: typescript
    root: infra
    manifest: infra/package.json
tasks:
  check:
    - workspace: infra
      command: npm run check
iac:
  root: infra
  provider: pulumi
  stacks:
    - staging
ops:
  probes:
    - name: ligand-health
      method: HEAD
      url: https://ligand.dev.int.iostream.app
      expect:
        status: 200
"#,
        )
        .expect("ligand manifest");
    }

    fn write_adoption_fixture(source: &Path, dest: &Path) {
        fs::create_dir_all(source).expect("source dir");
        fs::create_dir_all(dest.join("apps")).expect("dest apps");
        fs::create_dir_all(dest.join("frameworks")).expect("dest frameworks");
        fs::create_dir_all(dest.join("foundations")).expect("dest foundations");
        fs::create_dir_all(dest.join("core-infra")).expect("dest core");
        fs::create_dir_all(dest.join("tools")).expect("dest tools");
        fs::write(
            dest.join("repo.yaml"),
            r#"
schema: company.repo/v1
name: universe
layout: functional
defaults:
  owner: "@platform"
"#,
        )
        .expect("dest repo");

        fs::create_dir_all(source.join("operon/.git")).expect("operon git");
        fs::create_dir_all(source.join("operon/.github/workflows")).expect("operon github");
        fs::write(
            source.join("operon/README.md"),
            "# Operon\nReusable serverless framework and SDK.\n",
        )
        .expect("operon readme");
        fs::write(
            source.join("operon/Cargo.toml"),
            r#"[package]
name = "operon"
version = "0.1.0"
edition = "2024"
"#,
        )
        .expect("operon cargo");

        fs::create_dir_all(source.join("golgi/.git")).expect("golgi git");
        fs::create_dir_all(source.join("golgi/node_modules")).expect("golgi node_modules");
        fs::write(source.join("golgi/node_modules/generated.txt"), "generated").expect("generated");
        fs::write(source.join("golgi/README.md"), "# Golgi\nProduct app.\n").expect("golgi readme");
        fs::write(
            source.join("golgi/Cargo.toml"),
            r#"[package]
name = "golgi"
version = "0.1.0"
edition = "2024"

[dependencies]
operon = { version = "0.1.0", registry = "chromatin" }
"#,
        )
        .expect("golgi cargo");

        fs::create_dir_all(source.join("synapse")).expect("synapse");
        fs::write(
            source.join("synapse/README.md"),
            "# Synapse\nSkip this repo.\n",
        )
        .expect("synapse readme");
    }
}
