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

/// Typed repoctl facade.
#[derive(Clone, Debug)]
pub struct Repoctl {
    engine: RepoctlEngine,
    proto: ProtoFacade,
    iac: IacFacade,
    skills: SkillsFacade,
}

impl Repoctl {
    /// Creates a facade with default local adapters.
    pub fn with_default_adapters() -> Result<Self, RepoctlError> {
        Ok(Self {
            engine: RepoctlEngine::with_default_adapters(),
            proto: ProtoFacade,
            iac: IacFacade,
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
    pub fn init(&self, _request: InitRequest) -> Result<InitPlan, RepoctlError> {
        unsupported("init is implemented in phase 4")
    }

    /// Plans a new project.
    pub fn new_project(&self, _request: NewProjectRequest) -> Result<RenderPlan, RepoctlError> {
        unsupported("new project commands are implemented in phase 5")
    }

    /// Computes affected projects.
    pub fn affected(&self, _request: AffectedRequest) -> Result<AffectedReport, RepoctlError> {
        unsupported("affected analysis is implemented in phase 6")
    }

    /// Runs a repo task.
    pub fn run_task(&self, _request: TaskRunRequest) -> Result<TaskRunReport, RepoctlError> {
        unsupported("task execution is implemented in phase 6")
    }

    /// Builds a CI matrix.
    pub fn ci_matrix(&self, _request: CiMatrixRequest) -> Result<CiMatrixReport, RepoctlError> {
        unsupported("CI matrix generation is implemented in phase 6")
    }

    /// Builds a PR summary.
    pub fn pr_summary(&self, _request: PrSummaryRequest) -> Result<PrSummary, RepoctlError> {
        unsupported("PR summaries are implemented in phase 8")
    }

    /// Builds AI context.
    pub fn ai_context(&self, _request: AiContextRequest) -> Result<AiContext, RepoctlError> {
        unsupported("AI context is implemented in phase 8")
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
pub struct ProtoFacade;

impl ProtoFacade {
    /// Placeholder for proto command families.
    pub fn check(&self, _request: ProtoFacadeRequest) -> Result<ProtoFacadeReport, RepoctlError> {
        unsupported("proto commands are implemented in phase 7")
    }
}

/// `IaC` command facade.
#[derive(Clone, Debug, Default)]
pub struct IacFacade;

impl IacFacade {
    /// Placeholder for `IaC` command families.
    pub fn plan(&self, _request: IacFacadeRequest) -> Result<IacFacadeReport, RepoctlError> {
        unsupported("IaC plan is implemented in phase 9")
    }
}

/// Skills command facade.
#[derive(Clone, Debug, Default)]
pub struct SkillsFacade;

impl SkillsFacade {
    /// Placeholder for skills command families.
    pub fn check(&self, _request: SkillsFacadeRequest) -> Result<SkillsFacadeReport, RepoctlError> {
        unsupported("skills commands are implemented in phase 4")
    }
}

fn unsupported<T>(message: &'static str) -> Result<T, RepoctlError> {
    Err(RepoctlError::diagnostic(Diagnostic::error(
        "repoctl.unsupported_phase",
        message,
    )))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{DiscoverRequest, GraphValidateRequest, Repoctl};

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
}
