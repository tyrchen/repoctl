#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
// Facade methods intentionally own request DTOs so frontends can build-and-call without lifetimes.
#![allow(clippy::needless_pass_by_value)]

//! Public repoctl facade consumed by CLI and other frontends.

pub use repoctl_core::*;
use repoctl_engine::RepoctlEngine;
use repoctl_runner::RunnerService;
use repoctl_scaffold::ScaffoldService;

/// Typed repoctl facade.
#[derive(Clone, Debug)]
pub struct Repoctl {
    engine: RepoctlEngine,
    scaffold: ScaffoldService,
    runner: RunnerService,
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
        let snapshot = self.discover(DiscoverRequest { repo: request.repo })?;
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

    /// Builds a PR summary.
    pub fn pr_summary(&self, request: PrSummaryRequest) -> Result<PrSummary, RepoctlError> {
        self.runner.pr_summary(&request)
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

    /// Synchronizes generated skills.
    pub fn sync(
        &self,
        mut request: SkillsFacadeRequest,
    ) -> Result<SkillsFacadeReport, RepoctlError> {
        request.sync = true;
        ScaffoldService::with_default_adapters().skills(&request)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{
        AiContextRequest, CodegenCheckRequest, DiscoverRequest, GraphValidateRequest,
        IacFacadeRequest, PrSummaryRequest, ProjectName, ProtoFacadeRequest, ProtoOperation,
        RepoRelativePath, Repoctl, TemplateListRequest, TemplateRenderRequest, TemplateSource,
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
}
