//! Repository discovery service.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use repoctl_core::{
    Diagnostic, DiscoverRequest, GraphBuildInput, GraphBuilder, ManifestParser, ManifestSource,
    ProjectManifest, ProjectRelativePath, RepoFileSystem, RepoLocator, RepoRelativePath,
    RepoSnapshot, RepoctlError, WalkRequest, YamlManifestParser, validate_project_convention,
};

use crate::{DefaultGraphBuilder, DefaultRepoLocator, LocalRepoFileSystem};

/// Discovers repo manifests and builds snapshots.
#[derive(Clone)]
pub struct DiscoveryService {
    locator: Arc<dyn RepoLocator>,
    filesystem: Arc<dyn RepoFileSystem>,
    parser: Arc<dyn ManifestParser>,
    graph_builder: Arc<dyn GraphBuilder>,
}

impl std::fmt::Debug for DiscoveryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryService").finish_non_exhaustive()
    }
}

impl Default for DiscoveryService {
    fn default() -> Self {
        Self {
            locator: Arc::new(DefaultRepoLocator),
            filesystem: Arc::new(LocalRepoFileSystem),
            parser: Arc::new(YamlManifestParser),
            graph_builder: Arc::new(DefaultGraphBuilder::default()),
        }
    }
}

impl DiscoveryService {
    /// Creates a discovery service from explicit adapters.
    pub fn new(
        locator: Arc<dyn RepoLocator>,
        filesystem: Arc<dyn RepoFileSystem>,
        parser: Arc<dyn ManifestParser>,
        graph_builder: Arc<dyn GraphBuilder>,
    ) -> Self {
        Self {
            locator,
            filesystem,
            parser,
            graph_builder,
        }
    }

    /// Discovers a repository and returns a validated snapshot.
    pub fn discover(&self, request: &DiscoverRequest) -> Result<RepoSnapshot, RepoctlError> {
        let root = self.locator.locate(request.repo.as_deref())?;
        let repo_path = RepoRelativePath::new("repo.yaml").map_err(RepoctlError::diagnostic)?;
        let repo_bytes = self.filesystem.read_file(&root, &repo_path)?;
        let repo_manifest = self
            .parser
            .parse_repo(ManifestSource::new("repo.yaml", repo_bytes))?;
        let project_paths = self.discover_project_manifest_paths(&root)?;
        let mut diagnostics = Vec::new();
        let mut projects = Vec::new();
        for path in project_paths {
            match self.filesystem.read_file(&root, &path).and_then(|bytes| {
                self.parser
                    .parse_project(ManifestSource::new(path.as_str(), bytes))
            }) {
                Ok(project) => projects.push(project),
                Err(error) => diagnostics.extend(error.diagnostics()),
            }
        }
        Self::apply_repo_defaults(&repo_manifest, &mut projects, &mut diagnostics);
        diagnostics.extend(validate_project_names(&projects));
        diagnostics.extend(validate_manifest_locations(&projects));
        diagnostics.extend(validate_project_dependencies(&projects));
        for project in &projects {
            diagnostics.extend(validate_project_convention(project));
        }
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == repoctl_core::Severity::Error)
        {
            return Err(RepoctlError::Diagnostics { diagnostics });
        }
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        let graph = self.graph_builder.build(GraphBuildInput {
            root: root.clone(),
            repo_manifest: repo_manifest.clone(),
            projects: projects.clone(),
        })?;
        Ok(RepoSnapshot::new(root, repo_manifest, projects, graph))
    }

    fn discover_project_manifest_paths(
        &self,
        root: &repoctl_core::RepoRoot,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        let files = self.filesystem.walk(root, &WalkRequest::default())?;
        let mut project_paths = files
            .into_iter()
            .filter(|path| path.as_str().ends_with("project.yaml"))
            .collect::<Vec<_>>();
        project_paths.sort();
        Ok(project_paths)
    }

    fn apply_repo_defaults(
        repo_manifest: &repoctl_core::RepoManifest,
        projects: &mut [ProjectManifest],
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        for project in projects {
            if project.owners.is_empty() {
                if let Some(owner) = &repo_manifest.default_owner {
                    project.owners.push(owner.clone());
                } else {
                    diagnostics.push(
                        Diagnostic::error(
                            "manifest.project.owner_missing",
                            format!("project `{}` must declare at least one owner", project.name),
                        )
                        .with_path(project.source.as_str())
                        .with_project(project.name.as_str()),
                    );
                }
            }
        }
    }
}

fn validate_project_names(projects: &[ProjectManifest]) -> Vec<Diagnostic> {
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for project in projects {
        if !seen.insert(project.name.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "manifest.project.duplicate_name",
                    format!("duplicate project name `{}`", project.name),
                )
                .with_path(project.source.as_str())
                .with_project(project.name.as_str()),
            );
        }
    }
    diagnostics
}

fn validate_manifest_locations(projects: &[ProjectManifest]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let project_manifest = ProjectRelativePath::new("project.yaml");
    let Ok(project_manifest) = project_manifest else {
        return diagnostics;
    };
    for project in projects {
        let expected = project.path.join_project(&project_manifest);
        match expected {
            Ok(expected) if expected == project.source => {}
            Ok(expected) => diagnostics.push(
                Diagnostic::error(
                    "manifest.project.path_mismatch",
                    format!(
                        "project `{}` declares path `{}` but manifest is at `{}`",
                        project.name, project.path, project.source
                    ),
                )
                .with_path(project.source.as_str())
                .with_project(project.name.as_str())
                .with_help(format!(
                    "move the manifest to `{expected}` or update `path`"
                )),
            ),
            Err(diagnostic) => diagnostics.push(diagnostic.with_path(project.source.as_str())),
        }
    }
    diagnostics
}

fn validate_project_dependencies(projects: &[ProjectManifest]) -> Vec<Diagnostic> {
    let by_name = projects
        .iter()
        .map(|project| (project.name.clone(), project))
        .collect::<BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();
    for project in projects {
        for dependency in &project.depends_on {
            let repoctl_core::DependencyTarget::Project(target) = &dependency.target else {
                continue;
            };
            if !by_name.contains_key(target) {
                diagnostics.push(
                    Diagnostic::error(
                        "manifest.dependency.unknown_project",
                        format!(
                            "project `{}` depends on unknown project `{target}`",
                            project.name
                        ),
                    )
                    .with_path(project.source.as_str())
                    .with_project(project.name.as_str()),
                );
            }
        }
    }
    diagnostics
}
