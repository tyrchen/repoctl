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

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use camino::{Utf8Path, Utf8PathBuf};
pub use repoctl_core::*;
use repoctl_engine::{
    DefaultGraphBuilder, DefaultRepoLocator, DiscoveryService, LocalRepoFileSystem, RepoctlEngine,
};
use repoctl_inspect::InspectorService;
use repoctl_runner::RunnerService;
use repoctl_scaffold::ScaffoldService;
use serde_json::Value;
use toml_edit::{DocumentMut, InlineTable, Item, Value as TomlValue, value};

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
        let content = render_github_actions_workflow(&snapshot)?;
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

        let sections = vec![
            self.doctor_graph_section(repo.clone(), changed_files.clone()),
            self.doctor_affected_section(
                repo.clone(),
                base.clone(),
                head.clone(),
                changed_files.clone(),
                tasks,
            ),
            self.doctor_skills_section(repo.clone()),
            self.doctor_hygiene_section(repo.clone()),
            self.doctor_codegen_section(
                repo.clone(),
                base.clone(),
                head.clone(),
                changed_files.clone(),
            ),
            self.doctor_inspect_size_section(
                repo.clone(),
                base.clone(),
                head.clone(),
                changed_files.clone(),
            ),
            self.doctor_proto_section(
                repo.clone(),
                base.clone(),
                head.clone(),
                changed_files.clone(),
            ),
            self.doctor_iac_section(repo, base, head, changed_files),
        ];

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

const ADOPTION_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".github",
    "target",
    "node_modules",
    "dist",
    ".next",
    ".turbo",
    ".cache",
];

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

fn utf8_absolute(path: &Path) -> Result<Utf8PathBuf, RepoctlError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| RepoctlError::Environment(format!("failed to read cwd: {source}")))?
            .join(path)
    };
    Utf8PathBuf::from_path_buf(absolute).map_err(|path| {
        RepoctlError::diagnostic(Diagnostic::error(
            "path.non_utf8",
            format!("path is not valid UTF-8: {}", path.display()),
        ))
    })
}

fn read_adoption_plan(path: &Path) -> Result<AdoptionPlan, RepoctlError> {
    let bytes =
        fs::read(path).map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    serde_json::from_slice(&bytes).map_err(|source| {
        RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.plan.json",
                format!("failed to parse plan JSON: {source}"),
            )
            .with_path(path.display().to_string()),
        )
    })
}

fn plan_adoption(
    request: &AdoptionPlanRequest,
    repoctl: &Repoctl,
) -> Result<AdoptionPlan, RepoctlError> {
    let source_root = utf8_absolute(&request.source)?;
    let dest_root = utf8_absolute(&request.dest)?;
    if !dest_root.join("repo.yaml").is_file() {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "adoption.dest.uninitialized",
            "destination must be an initialized repoctl monorepo with repo.yaml",
        )));
    }

    let mut source_plan = plan_adoption_sources(request, &source_root, &dest_root)?;

    let dependency_rewrites = if request.rewrite_deps == DependencyRewriteMode::Off {
        Vec::new()
    } else {
        let package_index = build_adopted_package_index(&source_plan.sources)?;
        plan_dependency_rewrites(&source_plan.sources, &package_index)?
    };
    if request.rewrite_deps == DependencyRewriteMode::Auto {
        apply_rewrites_to_operations(&mut source_plan.operations, &dependency_rewrites)?;
    }
    let ci_operations = if request.ci == AdoptionCiMode::Off {
        Vec::new()
    } else {
        let workflow = repoctl.ci_workflow(CiWorkflowRequest {
            repo: Some(dest_root.as_std_path().to_path_buf()),
            provider: CiProvider::GitHubActions,
            write: false,
        })?;
        workflow.operations
    };
    let verification = adoption_verification_plan(
        &dest_root,
        request.verification.clone(),
        &source_plan.sources,
    );

    Ok(AdoptionPlan {
        source_root,
        dest_root,
        sources: source_plan.sources,
        operations: source_plan.operations,
        dependency_rewrites,
        manifest_syntheses: source_plan.manifest_syntheses,
        ci_operations,
        verification,
        diagnostics: source_plan.diagnostics,
    })
}

#[derive(Debug, Default)]
struct AdoptionSourcePlan {
    sources: Vec<AdoptedSource>,
    operations: Vec<AdoptionFileOperation>,
    manifest_syntheses: Vec<ProjectManifestSynthesis>,
    diagnostics: Vec<Diagnostic>,
}

fn plan_adoption_sources(
    request: &AdoptionPlanRequest,
    source_root: &Utf8Path,
    dest_root: &Utf8Path,
) -> Result<AdoptionSourcePlan, RepoctlError> {
    let candidates = source_repositories(source_root)?;
    let mut plan = AdoptionSourcePlan::default();
    for source_path in candidates {
        append_adoption_source(request, dest_root, source_path, &mut plan)?;
    }
    Ok(plan)
}

fn append_adoption_source(
    request: &AdoptionPlanRequest,
    dest_root: &Utf8Path,
    source_path: Utf8PathBuf,
    plan: &mut AdoptionSourcePlan,
) -> Result<(), RepoctlError> {
    let name = source_name(&source_path)?;
    let include_selected = request.include.is_empty() || request.include.contains(&name);
    let skipped = !include_selected || request.exclude.contains(&name);
    let inventory = inventory_source(&source_path, &name)?;
    let placement = infer_placement(
        &name,
        &inventory,
        request.map.get(&name).cloned(),
        request.kind.get(&name).cloned(),
    );
    if skipped {
        plan.sources.push(adopted_source(
            name,
            source_path,
            placement,
            inventory,
            true,
        ));
        return Ok(());
    }
    if placement.confidence < 0.75 && !placement.override_applied {
        plan.diagnostics.push(
            Diagnostic::error(
                "adoption.placement.low_confidence",
                format!(
                    "source `{name}` placement confidence {:.2} requires --map",
                    placement.confidence
                ),
            )
            .with_help(format!(
                "pass --map {name}=<lane>/{name} and optionally --kind {name}=<kind>"
            )),
        );
    }
    let owner = request
        .owner
        .get(&name)
        .cloned()
        .or_else(|| OwnerHandle::new(format!("@{name}")).ok());
    let manifest = synthesize_project_manifest(&name, &placement, &inventory, owner.as_ref())?;
    plan.operations.extend(copy_operations_for_source(
        &source_path,
        &placement.destination,
        dest_root,
    )?);
    let manifest_path = placement
        .destination
        .join_project(&ProjectRelativePath::new("project.yaml").map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    plan.operations.push(AdoptionFileOperation {
        id: format!("adoption.manifest.{name}"),
        operation: "write-file".to_string(),
        source_path: None,
        destination_path: manifest_path.clone(),
        content: Some(manifest.clone()),
        checksum: None,
    });
    plan.manifest_syntheses.push(ProjectManifestSynthesis {
        source: name.clone(),
        project: project_name_for_destination(&placement.destination, &placement.kind)?,
        manifest_path,
        content: manifest,
    });
    plan.sources.push(adopted_source(
        name,
        source_path,
        placement,
        inventory,
        false,
    ));
    Ok(())
}

fn adopted_source(
    name: String,
    source_path: Utf8PathBuf,
    placement: PlacementDecision,
    inventory: SourceInventory,
    skipped: bool,
) -> AdoptedSource {
    AdoptedSource {
        name,
        source_path,
        destination_path: placement.destination,
        inferred_kind: placement.kind,
        confidence: placement.confidence,
        reasons: placement.reasons,
        inventory,
        skipped,
        override_applied: placement.override_applied,
    }
}

fn apply_adoption_plan(plan: &AdoptionPlan) -> Result<(), RepoctlError> {
    let mut diagnostics = plan.diagnostics.clone();
    diagnostics.extend(plan.sources.iter().filter_map(|source| {
        if source.skipped || source.override_applied || source.confidence >= 0.75 {
            None
        } else {
            Some(
                Diagnostic::error(
                    "adoption.placement.low_confidence",
                    format!(
                        "source `{}` placement confidence {:.2} requires a reviewed override",
                        source.name, source.confidence
                    ),
                )
                .with_path(source.destination_path.as_str()),
            )
        }
    }));
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(RepoctlError::Diagnostics { diagnostics });
    }
    for operation in &plan.operations {
        apply_adoption_operation(plan, operation)?;
    }
    for operation in &plan.ci_operations {
        if let Some(content) = &operation.content {
            write_utf8_file(plan.dest_root.as_std_path(), &operation.path, content)?;
        }
    }
    Ok(())
}

fn apply_adoption_operation(
    plan: &AdoptionPlan,
    operation: &AdoptionFileOperation,
) -> Result<(), RepoctlError> {
    if operation.operation == "reject-symlink" {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.copy.symlink_rejected",
                format!(
                    "source symlink for `{}` is not allowed",
                    operation.destination_path
                ),
            )
            .with_path(operation.destination_path.as_str()),
        ));
    }
    let destination = plan.dest_root.join(operation.destination_path.as_str());
    if let Some(content) = &operation.content {
        write_utf8_file(
            plan.dest_root.as_std_path(),
            &operation.destination_path,
            content,
        )?;
        return Ok(());
    }
    let Some(source) = &operation.source_path else {
        return Ok(());
    };
    if destination.exists() {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.copy.conflict",
                format!(
                    "destination `{}` already exists",
                    operation.destination_path
                ),
            )
            .with_path(operation.destination_path.as_str()),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| RepoctlError::io(parent.as_str().to_string(), source))?;
    }
    fs::copy(source, &destination)
        .map_err(|source_error| RepoctlError::io(destination.as_str().to_string(), source_error))?;
    let metadata = fs::metadata(source)
        .map_err(|source_error| RepoctlError::io(source.as_str().to_string(), source_error))?;
    fs::set_permissions(&destination, metadata.permissions())
        .map_err(|source_error| RepoctlError::io(destination.as_str().to_string(), source_error))?;
    Ok(())
}

fn source_repositories(source_root: &Utf8Path) -> Result<Vec<Utf8PathBuf>, RepoctlError> {
    if source_root.join("Cargo.toml").is_file()
        || source_root.join("package.json").is_file()
        || source_root.join("project.yaml").is_file()
        || source_root.join("Pulumi.yaml").is_file()
    {
        return Ok(vec![source_root.to_path_buf()]);
    }
    let mut repos = Vec::new();
    let entries = fs::read_dir(source_root)
        .map_err(|source| RepoctlError::io(source_root.to_path_buf(), source))?;
    for entry in entries {
        let entry = entry.map_err(|source| RepoctlError::io(source_root.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        if path.is_dir()
            && (path.join("Cargo.toml").is_file()
                || path.join("package.json").is_file()
                || path.join("project.yaml").is_file()
                || path.join("Pulumi.yaml").is_file()
                || path.join("README.md").is_file())
        {
            repos.push(path);
        }
    }
    repos.sort();
    Ok(repos)
}

fn source_name(path: &Utf8Path) -> Result<String, RepoctlError> {
    path.file_name().map(ToString::to_string).ok_or_else(|| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.source.name",
            "source path must have a directory name",
        ))
    })
}

fn inventory_source(source_path: &Utf8Path, name: &str) -> Result<SourceInventory, RepoctlError> {
    let mut manifests = Vec::new();
    let mut top_level_dirs = Vec::new();
    let mut dependencies = BTreeSet::new();
    let mut generated = Vec::new();
    let mut tools = BTreeSet::new();
    let has_vcs = source_path.join(".git").exists();
    let readme_summary = read_readme_summary(source_path)?;

    for entry in fs::read_dir(source_path)
        .map_err(|source| RepoctlError::io(source_path.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(source_path.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        let Some(file_name) = path.file_name() else {
            continue;
        };
        if path.is_dir() {
            top_level_dirs.push(file_name.to_string());
        }
    }

    collect_inventory_files(
        source_path,
        source_path,
        &mut manifests,
        &mut dependencies,
        &mut generated,
        &mut tools,
    )?;
    top_level_dirs.sort();
    manifests.sort();
    generated.sort();
    Ok(SourceInventory {
        name: name.to_string(),
        has_vcs,
        readme_summary,
        manifests,
        top_level_dirs,
        dependency_references: dependencies.into_iter().collect(),
        generated_artifacts: generated,
        required_tools: tools.into_iter().collect(),
    })
}

fn collect_inventory_files(
    root: &Utf8Path,
    current: &Utf8Path,
    manifests: &mut Vec<String>,
    dependencies: &mut BTreeSet<String>,
    generated: &mut Vec<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    for entry in
        fs::read_dir(current).map_err(|source| RepoctlError::io(current.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(current.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        let Some(name) = path.file_name() else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .map_or_else(|_| name.to_string(), |value| value.as_str().to_string());
        if path.is_dir() {
            if ADOPTION_EXCLUDED_DIRS.contains(&name) {
                generated.push(rel);
                continue;
            }
            collect_inventory_files(root, &path, manifests, dependencies, generated, tools)?;
            continue;
        }
        if is_primary_manifest(name)
            || rel.ends_with("/package.json")
            || rel.ends_with("/Cargo.toml")
        {
            manifests.push(rel.clone());
        }
        if name == "Cargo.toml" {
            collect_cargo_dependencies(&path, dependencies, tools)?;
        } else if name == "package.json" {
            collect_package_json_dependencies(&path, dependencies, tools)?;
        } else if name == "build.rs" {
            let content = fs::read_to_string(&path)
                .map_err(|source| RepoctlError::io(path.clone(), source))?;
            if content.contains("prost_build") || content.contains("protoc") {
                tools.insert("protoc".to_string());
            }
        }
    }
    Ok(())
}

fn read_readme_summary(source_path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let path = source_path.join("README.md");
    if !path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|source| RepoctlError::io(path, source))?;
    Ok(content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_string()))
}

fn is_primary_manifest(name: &str) -> bool {
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain.toml"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "Pulumi.yaml"
            | "operon.yaml"
            | "Makefile"
            | "project.yaml"
            | "project.yml"
    )
}

fn collect_cargo_dependencies(
    path: &Utf8Path,
    dependencies: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let Ok(document) = content.parse::<DocumentMut>() else {
        return Ok(());
    };
    for table_name in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "workspace.dependencies",
    ] {
        collect_toml_dependency_table(&document, table_name, dependencies);
    }
    if content.contains("prost-build") {
        tools.insert("protoc".to_string());
    }
    Ok(())
}

fn collect_toml_dependency_table(
    document: &DocumentMut,
    table_name: &str,
    dependencies: &mut BTreeSet<String>,
) {
    let Some(table) =
        dotted_toml_item(document.as_item(), table_name).and_then(Item::as_table_like)
    else {
        return;
    };
    for (name, item) in table.iter() {
        if name.starts_with("@cellis") || name == "operon" || item.to_string().contains("chromatin")
        {
            dependencies.insert(name.to_string());
        }
    }
}

fn dotted_toml_item<'a>(item: &'a Item, path: &str) -> Option<&'a Item> {
    let mut current = item;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn collect_package_json_dependencies(
    path: &Utf8Path,
    dependencies: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Ok(());
    };
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(object) = value.get(section).and_then(Value::as_object) {
            for key in object.keys() {
                if key.starts_with("@cellis/") || key == "operon" {
                    dependencies.insert(key.clone());
                }
            }
        }
    }
    if let Some(scripts) = value.get("scripts").and_then(Value::as_object) {
        for command in scripts.values().filter_map(Value::as_str) {
            if command.contains("pulumi") {
                tools.insert("pulumi".to_string());
            }
            if command.contains("buf") {
                tools.insert("buf".to_string());
            }
        }
    }
    tools.insert("node".to_string());
    Ok(())
}

#[derive(Clone, Debug)]
struct PlacementDecision {
    destination: RepoRelativePath,
    kind: ProjectKind,
    confidence: f32,
    reasons: Vec<String>,
    override_applied: bool,
}

fn infer_placement(
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
        "{} {} {} {}",
        name,
        inventory.readme_summary.clone().unwrap_or_default(),
        inventory.top_level_dirs.join(" "),
        inventory.dependency_references.join(" ")
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
    } else if text.contains("nucleus")
        || text.contains("membrane")
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
    } else if text.contains("framework")
        || text.contains("sdk")
        || text.contains("runtime")
        || name == "operon"
    {
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
        || matches!(name, "atp" | "chromatin" | "ligand")
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

fn synthesize_project_manifest(
    name: &str,
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
    let tasks = synthesize_tasks(inventory, &workspaces);
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
    let _ = name;
    Ok(output)
}

fn project_kind_manifest_name(kind: &ProjectKind) -> &'static str {
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

fn project_name_for_destination(
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

fn synthesize_tasks(inventory: &SourceInventory, workspaces: &[String]) -> String {
    let mut tasks = String::new();
    if workspaces
        .iter()
        .any(|workspace| workspace.contains("language: rust"))
    {
        tasks.push_str("  check:\n    - workspace: service\n      command: cargo check\n");
        tasks.push_str("  test:\n    - workspace: service\n      command: cargo test\n");
        tasks.push_str("  build:\n    - workspace: service\n      command: cargo build\n");
    }
    if workspaces
        .iter()
        .any(|workspace| workspace.contains("language: typescript"))
    {
        let workspace = if inventory
            .manifests
            .iter()
            .any(|path| path.starts_with("infra/package.json"))
        {
            "infra"
        } else {
            "frontend"
        };
        let _ = write!(
            tasks,
            "  check:\n    - workspace: {workspace}\n      command: npx tsc --noEmit\n"
        );
        let _ = write!(
            tasks,
            "  build:\n    - workspace: {workspace}\n      command: npm run build\n"
        );
    }
    tasks
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

fn copy_operations_for_source(
    source_path: &Utf8Path,
    destination: &RepoRelativePath,
    dest_root: &Utf8Path,
) -> Result<Vec<AdoptionFileOperation>, RepoctlError> {
    let mut operations = Vec::new();
    collect_copy_operations(
        source_path,
        source_path,
        destination,
        dest_root,
        &mut operations,
    )?;
    Ok(operations)
}

fn collect_copy_operations(
    root: &Utf8Path,
    current: &Utf8Path,
    destination: &RepoRelativePath,
    dest_root: &Utf8Path,
    operations: &mut Vec<AdoptionFileOperation>,
) -> Result<(), RepoctlError> {
    for entry in
        fs::read_dir(current).map_err(|source| RepoctlError::io(current.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(current.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        let Some(name) = path.file_name() else {
            continue;
        };
        if matches!(name, "project.yaml" | "project.yml") {
            continue;
        }
        if path.is_dir() && ADOPTION_EXCLUDED_DIRS.contains(&name) {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            RepoctlError::diagnostic(Diagnostic::error(
                "adoption.copy.path",
                "source path escaped source root",
            ))
        })?;
        let relative_project_path = ProjectRelativePath::new(relative.as_str().to_string())
            .map_err(RepoctlError::diagnostic)?;
        let destination_path = destination
            .join_project(&relative_project_path)
            .map_err(RepoctlError::diagnostic)?;
        if path.is_dir() {
            collect_copy_operations(root, &path, destination, dest_root, operations)?;
        } else if path.is_symlink() {
            operations.push(AdoptionFileOperation {
                id: format!("adoption.copy.reject_symlink.{destination_path}"),
                operation: "reject-symlink".to_string(),
                source_path: Some(path),
                destination_path,
                content: None,
                checksum: None,
            });
        } else {
            let target = dest_root.join(destination_path.as_str());
            if target.exists() {
                return Err(RepoctlError::diagnostic(
                    Diagnostic::error(
                        "adoption.copy.conflict",
                        format!("destination `{destination_path}` already exists"),
                    )
                    .with_path(destination_path.as_str()),
                ));
            }
            operations.push(AdoptionFileOperation {
                id: format!("adoption.copy.{destination_path}"),
                operation: "copy-file".to_string(),
                source_path: Some(path),
                destination_path,
                content: None,
                checksum: None,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct PackageIndex {
    rust: BTreeMap<String, RepoRelativePath>,
    node: BTreeMap<String, RepoRelativePath>,
}

fn build_adopted_package_index(sources: &[AdoptedSource]) -> Result<PackageIndex, RepoctlError> {
    let mut index = PackageIndex::default();
    for source in sources.iter().filter(|source| !source.skipped) {
        collect_package_index(
            &source.source_path,
            &source.source_path,
            &source.destination_path,
            &mut index,
        )?;
    }
    Ok(index)
}

fn collect_package_index(
    root: &Utf8Path,
    current: &Utf8Path,
    destination: &RepoRelativePath,
    index: &mut PackageIndex,
) -> Result<(), RepoctlError> {
    for entry in
        fs::read_dir(current).map_err(|source| RepoctlError::io(current.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(current.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        let Some(name) = path.file_name() else {
            continue;
        };
        if path.is_dir() {
            if ADOPTION_EXCLUDED_DIRS.contains(&name) {
                continue;
            }
            collect_package_index(root, &path, destination, index)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            RepoctlError::diagnostic(Diagnostic::error(
                "adoption.package.path",
                "package path escaped source root",
            ))
        })?;
        let package_root = relative
            .parent()
            .filter(|path| !path.as_str().is_empty())
            .unwrap_or_else(|| Utf8Path::new("."));
        let package_root = ProjectRelativePath::new(package_root.as_str().to_string())
            .map_err(RepoctlError::diagnostic)?;
        let dest_package_root = destination
            .join_project(&package_root)
            .map_err(RepoctlError::diagnostic)?;
        if name == "Cargo.toml" {
            if let Some(package) = cargo_package_name(&path)? {
                index.rust.insert(package, dest_package_root);
            }
        } else if name == "package.json"
            && let Some(package) = node_package_name(&path)?
        {
            index.node.insert(package, dest_package_root);
        }
    }
    Ok(())
}

fn cargo_package_name(path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let Ok(document) = content.parse::<DocumentMut>() else {
        return Ok(None);
    };
    Ok(document
        .get("package")
        .and_then(|item| item.get("name"))
        .and_then(Item::as_str)
        .map(ToString::to_string))
}

fn node_package_name(path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Ok(None);
    };
    Ok(value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

fn plan_dependency_rewrites(
    sources: &[AdoptedSource],
    index: &PackageIndex,
) -> Result<Vec<DependencyRewrite>, RepoctlError> {
    let mut rewrites = Vec::new();
    for source in sources.iter().filter(|source| !source.skipped) {
        for manifest in &source.inventory.manifests {
            let file = source
                .destination_path
                .join_project(
                    &ProjectRelativePath::new(manifest.clone())
                        .map_err(RepoctlError::diagnostic)?,
                )
                .map_err(RepoctlError::diagnostic)?;
            if manifest.ends_with("Cargo.toml") {
                for package in &source.inventory.dependency_references {
                    if let Some(target) = index.rust.get(package) {
                        rewrites.push(DependencyRewrite {
                            file: file.clone(),
                            package: package.clone(),
                            from: "registry/git".to_string(),
                            to: relative_dependency_path(&file, target),
                            surface: DependencySurface::FrameworkFacade,
                            owner_project: project_name_for_destination(
                                &source.destination_path,
                                &source.inferred_kind,
                            )?,
                        });
                    }
                }
            } else if manifest.ends_with("package.json") {
                for package in &source.inventory.dependency_references {
                    if let Some(target) = index.node.get(package) {
                        rewrites.push(DependencyRewrite {
                            file: file.clone(),
                            package: package.clone(),
                            from: "registry/git".to_string(),
                            to: format!("file:{}", relative_dependency_path(&file, target)),
                            surface: DependencySurface::FrameworkFacade,
                            owner_project: project_name_for_destination(
                                &source.destination_path,
                                &source.inferred_kind,
                            )?,
                        });
                    }
                }
            }
        }
    }
    rewrites.sort_by(|left, right| (&left.file, &left.package).cmp(&(&right.file, &right.package)));
    rewrites.dedup_by(|left, right| left.file == right.file && left.package == right.package);
    Ok(rewrites)
}

fn apply_rewrites_to_operations(
    operations: &mut [AdoptionFileOperation],
    rewrites: &[DependencyRewrite],
) -> Result<(), RepoctlError> {
    let mut by_file: BTreeMap<RepoRelativePath, Vec<&DependencyRewrite>> = BTreeMap::new();
    for rewrite in rewrites {
        by_file
            .entry(rewrite.file.clone())
            .or_default()
            .push(rewrite);
    }
    for (file, file_rewrites) in by_file {
        let Some(operation) = operations
            .iter_mut()
            .find(|operation| operation.destination_path == file)
        else {
            continue;
        };
        let Some(source_path) = &operation.source_path else {
            continue;
        };
        let content = fs::read_to_string(source_path)
            .map_err(|source| RepoctlError::io(source_path.clone(), source))?;
        let rewritten = if file.as_str().ends_with("Cargo.toml") {
            rewrite_cargo_manifest(&content, &file_rewrites)?
        } else if file.as_str().ends_with("package.json") {
            rewrite_package_json(&content, &file_rewrites)?
        } else {
            content
        };
        operation.content = Some(rewritten);
    }
    Ok(())
}

fn rewrite_cargo_manifest(
    content: &str,
    rewrites: &[&DependencyRewrite],
) -> Result<String, RepoctlError> {
    let mut document = content.parse::<DocumentMut>().map_err(|source| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.dependency.rewrite",
            format!("failed to parse Cargo manifest: {source}"),
        ))
    })?;
    for table_name in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "workspace.dependencies",
    ] {
        rewrite_cargo_table(&mut document, table_name, rewrites);
    }
    Ok(document.to_string())
}

fn rewrite_cargo_table(
    document: &mut DocumentMut,
    table_name: &str,
    rewrites: &[&DependencyRewrite],
) {
    let Some(table) =
        dotted_toml_item_mut(document.as_item_mut(), table_name).and_then(Item::as_table_like_mut)
    else {
        return;
    };
    for rewrite in rewrites {
        if table.contains_key(&rewrite.package) {
            table.insert(&rewrite.package, cargo_path_dependency(&rewrite.to));
        }
    }
}

fn cargo_path_dependency(path: &str) -> Item {
    let mut table = InlineTable::default();
    let Ok(path_value) = value(path.to_string()).into_value() else {
        return value(path.to_string());
    };
    table.insert("path", path_value);
    Item::Value(TomlValue::InlineTable(table))
}

fn dotted_toml_item_mut<'a>(item: &'a mut Item, path: &str) -> Option<&'a mut Item> {
    let mut current = item;
    for part in path.split('.') {
        current = current.get_mut(part)?;
    }
    Some(current)
}

fn rewrite_package_json(
    content: &str,
    rewrites: &[&DependencyRewrite],
) -> Result<String, RepoctlError> {
    let mut value = serde_json::from_str::<Value>(content).map_err(|source| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.dependency.rewrite",
            format!("failed to parse package.json: {source}"),
        ))
    })?;
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(object) = value.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        for rewrite in rewrites {
            if object.contains_key(&rewrite.package) {
                object.insert(rewrite.package.clone(), Value::String(rewrite.to.clone()));
            }
        }
    }
    serde_json::to_string_pretty(&value)
        .map(|mut output| {
            output.push('\n');
            output
        })
        .map_err(|source| {
            RepoctlError::Internal(format!("failed to render rewritten package.json: {source}"))
        })
}

fn relative_dependency_path(from_file: &RepoRelativePath, target: &RepoRelativePath) -> String {
    let from_parent = from_file
        .as_str()
        .rsplit_once('/')
        .map_or(".", |(parent, _)| parent);
    let from_parts = from_parent
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    let target_parts = target
        .as_str()
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    let common = from_parts
        .iter()
        .zip(target_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_string());
    }
    parts.extend(
        target_parts[common..]
            .iter()
            .map(|part| (*part).to_string()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn adoption_verification_plan(
    dest_root: &Utf8Path,
    mode: ValidationMode,
    sources: &[AdoptedSource],
) -> VerificationPlan {
    let mut prerequisites = BTreeMap::new();
    for source in sources.iter().filter(|source| !source.skipped) {
        for tool in &source.inventory.required_tools {
            prerequisites.insert(
                tool.clone(),
                ToolPrerequisite {
                    tool: tool.clone(),
                    reason: format!("required by adopted source `{}`", source.name),
                },
            );
        }
    }
    let repo = RepoRelativePath::root();
    let mut commands = vec![
        process_command(
            dest_root,
            &repo,
            "repoctl",
            ["graph", "validate", "--mode", "structural"],
        ),
        process_command(dest_root, &repo, "repoctl", ["hygiene", "check"]),
    ];
    if matches!(mode, ValidationMode::Metadata | ValidationMode::Full) {
        commands.push(process_command(
            dest_root,
            &repo,
            "repoctl",
            ["graph", "validate", "--mode", "metadata"],
        ));
    }
    if mode == ValidationMode::Full {
        for source in sources.iter().filter(|source| !source.skipped) {
            if source
                .inventory
                .manifests
                .iter()
                .any(|path| path.ends_with("Cargo.toml"))
            {
                commands.push(process_command(
                    dest_root,
                    &source.destination_path,
                    "cargo",
                    ["check", "--workspace", "--all-features"],
                ));
            }
            if source
                .inventory
                .manifests
                .iter()
                .any(|path| path.ends_with("package.json"))
            {
                commands.push(process_command(
                    dest_root,
                    &source.destination_path,
                    "npm",
                    ["run", "build"],
                ));
            }
        }
    }
    VerificationPlan {
        prerequisites: prerequisites.into_values().collect(),
        commands,
    }
}

fn process_command<const N: usize>(
    root: &Utf8Path,
    cwd: &RepoRelativePath,
    program: &str,
    args: [&str; N],
) -> ProcessCommand {
    ProcessCommand {
        cwd: cwd.clone(),
        absolute_cwd: Some(root.join(cwd.as_str()).as_std_path().to_path_buf()),
        program: program.to_string(),
        args: args.into_iter().map(ToString::to_string).collect(),
        ..ProcessCommand::default()
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

fn render_github_actions_workflow(snapshot: &RepoSnapshot) -> Result<String, RepoctlError> {
    let jobs = WorkflowJobs::from_snapshot(snapshot);
    let mut workflow = String::from(
        r#"# Generated by repoctl. Refresh with: repoctl ci workflow --provider github-actions --write
name: repoctl

on:
  pull_request:
  push:
    branches:
      - main
      - master

jobs:
  graph:
    name: graph
    runs-on: ubuntu-latest
    outputs:
      matrix: ${{ steps.matrix.outputs.matrix }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install repoctl
        run: cargo install --path apps/repoctl-cli --locked
      - name: Validate graph
        run: repoctl graph validate --mode structural
      - name: Inspect code size
        run: repoctl inspect size --scope changed --base ${{ github.event.pull_request.base.sha || github.event.before }} --head ${{ github.sha }} --fail-on error
      - id: matrix
        name: Compute matrix
        run: |
          matrix=$(repoctl ci matrix --tasks check,test,build --fallback all --format github-actions)
          echo "matrix=${matrix}" >> "$GITHUB_OUTPUT"
"#,
    );
    append_rust_workflow_job(&mut workflow, jobs.has_rust);
    append_node_workflow_job(&mut workflow, jobs.has_node);
    append_iac_workflow_job(&mut workflow, jobs.has_iac);
    append_required_workflow_job(&mut workflow, jobs)?;
    Ok(workflow)
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

#[derive(Clone, Copy, Debug)]
struct WorkflowJobs {
    has_rust: bool,
    has_node: bool,
    has_iac: bool,
}

impl WorkflowJobs {
    fn from_snapshot(snapshot: &RepoSnapshot) -> Self {
        let has_rust = snapshot.projects.iter().any(|project| {
            project
                .workspaces
                .iter()
                .any(|workspace| workspace.language == WorkspaceLanguage::Rust)
        });
        let has_node = snapshot.projects.iter().any(|project| {
            project
                .workspaces
                .iter()
                .any(|workspace| workspace.language == WorkspaceLanguage::TypeScript)
        });
        let has_iac = snapshot
            .projects
            .iter()
            .any(|project| project.iac.is_some());
        Self {
            has_rust,
            has_node,
            has_iac,
        }
    }
}

fn append_rust_workflow_job(workflow: &mut String, enabled: bool) {
    if enabled {
        workflow.push_str(
            r"
  rust:
    name: rust
    runs-on: ubuntu-latest
    needs: graph
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install protoc
        run: sudo apt-get update && sudo apt-get install -y protobuf-compiler
      - name: Run Rust checks
        run: repoctl run check --dry-run --format human
",
        );
    }
}

fn append_node_workflow_job(workflow: &mut String, enabled: bool) {
    if enabled {
        workflow.push_str(
            r#"
  node:
    name: node
    runs-on: ubuntu-latest
    needs: graph
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - name: Run Node checks
        run: repoctl run check --dry-run --format human
"#,
        );
    }
}

fn append_iac_workflow_job(workflow: &mut String, enabled: bool) {
    if enabled {
        workflow.push_str(
            r"
  iac:
    name: iac
    runs-on: ubuntu-latest
    needs: graph
    steps:
      - uses: actions/checkout@v4
      - name: Type-check IaC plans
        run: repoctl iac plan --affected --dry-run
",
        );
    }
}

fn append_required_workflow_job(
    workflow: &mut String,
    jobs: WorkflowJobs,
) -> Result<(), RepoctlError> {
    let needs = [
        "graph",
        if jobs.has_rust { "rust" } else { "" },
        if jobs.has_node { "node" } else { "" },
        if jobs.has_iac { "iac" } else { "" },
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    writeln!(
        workflow,
        r#"
  required:
    name: repoctl required
    runs-on: ubuntu-latest
    needs: [{needs}]
    if: always()
    steps:
      - name: Check required jobs
        run: |
          if [ '${{{{ contains(toJson(needs), '"result":"failure"') }}}}' = 'true' ]; then
            exit 1
          fi
"#
    )
    .map_err(|source| RepoctlError::Internal(format!("failed to render workflow: {source}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    // Facade tests build temporary repositories synchronously before invoking sync services.
    #![allow(clippy::disallowed_methods)]

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
        assert!(hygiene.diagnostics.is_empty());
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
        assert!(inspect_size.diagnostics.is_empty());
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
        assert!(hygiene.diagnostics.is_empty());
    }

    #[test]
    fn test_should_resolve_proto_through_facade() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_phase789_fixture(temp.path());
        let facade = Repoctl::with_default_adapters().expect("facade");
        let report = facade
            .proto()
            .owners(ProtoFacadeRequest {
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
        assert!(!verify.commands.is_empty());
        assert!(!verify.skipped_mutating_commands.is_empty());

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
