#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]

//! Discovery, graph construction, and policy evaluation for repoctl.

use std::sync::Arc;

use repoctl_core::RepoSnapshot;

mod adapters;
mod diagrams;
mod discovery;
mod graph;
mod local_inspectors;
mod policy;

pub use adapters::{DefaultRepoLocator, LocalRepoFileSystem, RustWorkspaceInspector};
pub use diagrams::{affected_mermaid, graph_mermaid};
pub use discovery::DiscoveryService;
pub use graph::DefaultGraphBuilder;
pub use local_inspectors::{
    LocalRustWorkspaceInspector, NodeWorkspaceInspector, PythonWorkspaceInspector,
};
pub use policy::{
    CrossAppDependencyRule, FoundationPublicClientOnlyRule, FrameworkFacadeOnlyRule,
    GeneratedCodeReadonlyRule, HighRiskIacRule, PolicyEngine, ProjectKindDependencyRule,
};

/// Default engine wiring for phase 0-3 repoctl behavior.
#[derive(Clone, Debug, Default)]
pub struct RepoctlEngine {
    discovery: DiscoveryService,
    policies: PolicyEngine,
}

impl RepoctlEngine {
    /// Creates an engine with local filesystem, YAML parsing, graph, and policy adapters.
    pub fn with_default_adapters() -> Self {
        Self::default()
    }

    /// Shares a command-scoped immutable snapshot across capability services.
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use repoctl_core::DiscoverRequest;
    /// use repoctl_engine::{DiscoveryService, RepoctlEngine};
    /// # fn main() -> Result<(), repoctl_core::RepoctlError> {
    /// let snapshot = DiscoveryService::default().discover(&DiscoverRequest::default())?;
    /// let service = RepoctlEngine::from_snapshot(Arc::new(snapshot));
    /// # let _ = service;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_snapshot(snapshot: Arc<RepoSnapshot>) -> Self {
        Self {
            discovery: DiscoveryService::from_snapshot(snapshot),
            policies: PolicyEngine::default(),
        }
    }

    /// Returns the discovery service.
    pub fn discovery(&self) -> &DiscoveryService {
        &self.discovery
    }

    /// Returns the policy engine.
    pub fn policies(&self) -> &PolicyEngine {
        &self.policies
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures build temporary repositories synchronously before invoking sync services.
    #![allow(clippy::disallowed_methods)]

    use std::{fs, path::Path};

    use repoctl_core::{DiscoverRequest, RepoRelativePath};

    use super::{DiscoveryService, PolicyEngine};

    #[test]
    fn test_should_discover_empty_repo() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(temp.path());
        let snapshot = DiscoveryService::default()
            .discover(&DiscoverRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect("snapshot");
        assert_eq!(snapshot.projects.len(), 0);
        assert_eq!(snapshot.graph.nodes.len(), 0);
    }

    #[test]
    fn test_should_build_graph_with_declared_edges() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(temp.path());
        write_project(
            temp.path(),
            "apps/catalog",
            r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
owners:
  - "@catalog"
depends_on:
  - frameworks.runtime
  - foundations.identity.client
"#,
        );
        write_project(
            temp.path(),
            "frameworks/runtime",
            r#"
schema: company.project/v1
name: frameworks.runtime
kind: framework
path: frameworks/runtime
owners:
  - "@platform"
"#,
        );
        write_project(
            temp.path(),
            "foundations/identity",
            r#"
schema: company.project/v1
name: foundations.identity
kind: foundation-service
path: foundations/identity
owners:
  - "@identity"
"#,
        );
        let snapshot = DiscoveryService::default()
            .discover(&DiscoverRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect("snapshot");
        assert_eq!(snapshot.projects.len(), 3);
        assert!(
            snapshot
                .graph
                .edges
                .iter()
                .any(|edge| edge.kind == repoctl_core::EdgeKind::UsesFrameworkFacade)
        );
        assert!(
            snapshot
                .graph
                .edges
                .iter()
                .any(|edge| edge.kind == repoctl_core::EdgeKind::UsesFoundationClient)
        );
    }

    #[test]
    fn test_should_reject_project_path_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(temp.path());
        write_project(
            temp.path(),
            "apps/catalog",
            r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/wrong
owners:
  - "@catalog"
"#,
        );
        let error = DiscoveryService::default()
            .discover(&DiscoverRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect_err("path mismatch");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "manifest.project.path_mismatch")
        );
    }

    #[test]
    fn test_should_report_policy_violations_and_risk() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(temp.path());
        write_project(
            temp.path(),
            "apps/catalog",
            r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
owners:
  - "@catalog"
depends_on:
  - apps.other
  - frameworks.runtime.internal
ai:
  do_not_edit:
    - "**/generated/**"
"#,
        );
        write_project(
            temp.path(),
            "apps/other",
            r#"
schema: company.project/v1
name: apps.other
kind: app
path: apps/other
owners:
  - "@other"
"#,
        );
        write_project(
            temp.path(),
            "frameworks/runtime",
            r#"
schema: company.project/v1
name: frameworks.runtime
kind: framework
path: frameworks/runtime
owners:
  - "@platform"
"#,
        );
        let snapshot = DiscoveryService::default()
            .discover(&DiscoverRequest {
                repo: Some(temp.path().to_path_buf()),
            })
            .expect("snapshot");
        let changed_files = vec![
            RepoRelativePath::new("apps/catalog/api/generated/client.rs").expect("path"),
            RepoRelativePath::new("apps/catalog/iac/stacks/prod.yaml").expect("path"),
        ];
        let diagnostics = PolicyEngine::default()
            .evaluate(&snapshot, &changed_files)
            .expect("policies");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "policy.cross_app_dependency")
        );
        assert!(
            diagnostics.iter().any(
                |diagnostic| diagnostic.code.as_ref() == "policy.framework_internal_dependency"
            )
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "policy.generated_code_readonly")
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_ref() == "policy.high_risk_iac")
        );
    }

    #[test]
    fn test_should_pin_one_snapshot_and_refresh_the_next_invocation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_repo_manifest(temp.path());
        let request = DiscoverRequest {
            repo: Some(temp.path().to_path_buf()),
        };
        let initial = DiscoveryService::default().discover(&request)?;
        let pinned = DiscoveryService::from_snapshot(std::sync::Arc::new(initial));
        write_project(
            temp.path(),
            "apps/new",
            "schema: company.project/v1\nname: apps.new\nkind: app\npath: apps/new\n",
        );
        assert_eq!(pinned.discover(&request)?.projects.len(), 0);
        assert_eq!(
            DiscoveryService::default()
                .discover(&request)?
                .projects
                .len(),
            1
        );
        Ok(())
    }

    fn write_repo_manifest(root: &Path) {
        fs::write(
            root.join("repo.yaml"),
            r#"
schema: company.repo/v1
name: acme
layout: functional
defaults:
  owner: "@platform"
policies:
  prod_change:
    required_owners:
      - "@platform"
      - "@security"
"#,
        )
        .expect("repo manifest");
    }

    fn write_project(root: &Path, relative: &str, contents: &str) {
        let dir = root.join(relative);
        fs::create_dir_all(&dir).expect("project dir");
        fs::write(dir.join("project.yaml"), contents).expect("project manifest");
    }
}
