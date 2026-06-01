//! Infrastructure and capability-service ports.

use std::collections::BTreeMap;

use crate::{
    RepoManifest, TemplateFile,
    diagnostic::{Diagnostic, RepoctlError},
    domain::{
        EdgeKind, GraphEdge, ProcessCommand, ProcessOutput, ProjectManifest, RepoGraph,
        RepoRelativePath, RepoRoot, RepoSnapshot, TemplateManifest, WorkspaceLanguage,
        WorkspaceSpec,
    },
    manifest::ManifestSource,
};

/// Locates a repository root from a starting path.
pub trait RepoLocator: Send + Sync {
    /// Finds the repository root.
    fn locate(&self, start: Option<&std::path::Path>) -> Result<RepoRoot, RepoctlError>;
}

/// Fixed repo locator useful for tests.
#[derive(Clone, Debug)]
pub struct FixedRepoLocator {
    /// Root returned for every locate request.
    pub root: RepoRoot,
}

impl RepoLocator for FixedRepoLocator {
    fn locate(&self, _start: Option<&std::path::Path>) -> Result<RepoRoot, RepoctlError> {
        Ok(self.root.clone())
    }
}

/// Filesystem operations used by repoctl services.
pub trait RepoFileSystem: Send + Sync {
    /// Reads a repo-relative file.
    fn read_file(&self, root: &RepoRoot, path: &RepoRelativePath) -> Result<Vec<u8>, RepoctlError>;

    /// Walks repo-relative files according to the request.
    fn walk(
        &self,
        root: &RepoRoot,
        request: &WalkRequest,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError>;
}

/// In-memory repo filesystem useful for service tests.
#[derive(Clone, Debug, Default)]
pub struct InMemoryRepoFileSystem {
    files: BTreeMap<RepoRelativePath, Vec<u8>>,
}

impl InMemoryRepoFileSystem {
    /// Creates an empty in-memory filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a repo-relative file.
    pub fn insert(&mut self, path: RepoRelativePath, bytes: impl Into<Vec<u8>>) -> Option<Vec<u8>> {
        self.files.insert(path, bytes.into())
    }
}

impl RepoFileSystem for InMemoryRepoFileSystem {
    fn read_file(
        &self,
        _root: &RepoRoot,
        path: &RepoRelativePath,
    ) -> Result<Vec<u8>, RepoctlError> {
        self.files.get(path).cloned().ok_or_else(|| {
            RepoctlError::diagnostic(
                Diagnostic::error("repo.fs.not_found", format!("file `{path}` was not found"))
                    .with_path(path.as_str()),
            )
        })
    }

    fn walk(
        &self,
        _root: &RepoRoot,
        request: &WalkRequest,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        let mut files = self
            .files
            .keys()
            .filter(|path| request.roots.iter().any(|root| path.starts_with(root)))
            .take(request.max_files.saturating_add(1))
            .cloned()
            .collect::<Vec<_>>();
        if files.len() > request.max_files {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "repo.walk.too_many_files",
                format!("repository walk exceeded {} files", request.max_files),
            )));
        }
        files.sort();
        Ok(files)
    }
}

/// Manifest parser boundary.
pub trait ManifestParser: Send + Sync {
    /// Parses a repo manifest.
    fn parse_repo(&self, source: ManifestSource) -> Result<RepoManifest, RepoctlError>;

    /// Parses a project manifest.
    fn parse_project(&self, source: ManifestSource) -> Result<ProjectManifest, RepoctlError>;

    /// Parses a template manifest.
    fn parse_template(&self, source: ManifestSource) -> Result<TemplateManifest, RepoctlError>;
}

/// Request for a bounded repository walk.
#[derive(Clone, Debug)]
pub struct WalkRequest {
    /// Root prefixes to walk.
    pub roots: Vec<RepoRelativePath>,
    /// Maximum files to return.
    pub max_files: usize,
}

impl Default for WalkRequest {
    fn default() -> Self {
        Self {
            roots: vec![RepoRelativePath::root()],
            max_files: 20_000,
        }
    }
}

/// Input required to build a repository graph.
#[derive(Clone, Debug)]
pub struct GraphBuildInput {
    /// Repository root.
    pub root: RepoRoot,
    /// Repo-level manifest.
    pub repo_manifest: RepoManifest,
    /// Project manifests.
    pub projects: Vec<ProjectManifest>,
}

/// Builds a repository graph from validated manifests and adapter-discovered edges.
pub trait GraphBuilder: Send + Sync {
    /// Builds the graph.
    fn build(&self, input: GraphBuildInput) -> Result<RepoGraph, RepoctlError>;
}

/// Static graph builder useful for tests.
#[derive(Clone, Debug, Default)]
pub struct StaticGraphBuilder {
    /// Graph returned for every build request.
    pub graph: RepoGraph,
}

impl GraphBuilder for StaticGraphBuilder {
    fn build(&self, _input: GraphBuildInput) -> Result<RepoGraph, RepoctlError> {
        Ok(self.graph.clone())
    }
}

/// Input passed to workspace inspectors.
#[derive(Clone, Debug)]
pub struct WorkspaceInspectionInput<'a> {
    /// Repository root.
    pub root: &'a RepoRoot,
    /// All discovered projects.
    pub projects: &'a [ProjectManifest],
    /// Project owning the workspace.
    pub project: &'a ProjectManifest,
    /// Workspace to inspect.
    pub workspace: &'a WorkspaceSpec,
}

/// Edge discovered by a language or toolchain adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredEdge {
    /// Source project.
    pub from_project: String,
    /// Source workspace.
    pub from_workspace: String,
    /// Target project.
    pub to_project: String,
    /// Edge kind.
    pub kind: EdgeKind,
    /// Evidence path or package name.
    pub evidence: Option<String>,
}

/// Inspects one workspace type for adapter-discovered dependencies.
pub trait WorkspaceInspector: Send + Sync {
    /// Language handled by the inspector.
    fn language(&self) -> WorkspaceLanguage;

    /// Discovers graph edges from the workspace.
    fn inspect(
        &self,
        input: &WorkspaceInspectionInput<'_>,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError>;
}

/// Context passed to policy rules.
#[derive(Clone, Debug)]
pub struct PolicyContext<'a> {
    /// Repository snapshot.
    pub snapshot: &'a RepoSnapshot,
    /// Changed files used by path-based rules.
    pub changed_files: &'a [RepoRelativePath],
}

/// Boundary or hygiene policy rule.
pub trait PolicyRule: Send + Sync {
    /// Stable rule name.
    fn name(&self) -> &'static str;

    /// Evaluates the rule.
    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError>;
}

/// Runs a process in argv form.
pub trait ProcessRunner: Send + Sync {
    /// Runs a command and captures output.
    fn run(&self, command: &ProcessCommand) -> Result<ProcessOutput, RepoctlError>;
}

/// Template render request.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    /// Template manifest.
    pub template: TemplateManifest,
    /// Template file mapping.
    pub file: TemplateFile,
    /// JSON render context.
    pub context: serde_json::Value,
}

/// Rendered template content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedTemplate {
    /// Rendered bytes.
    pub bytes: Vec<u8>,
}

/// Template engine boundary.
pub trait TemplateEngine: Send + Sync {
    /// Renders one template file.
    fn render(&self, request: &RenderRequest) -> Result<RenderedTemplate, RepoctlError>;
}

/// In-memory process runner useful for tests.
#[derive(Clone, Debug, Default)]
pub struct FakeProcessRunner {
    /// Output returned for every command.
    pub output: ProcessOutput,
}

impl ProcessRunner for FakeProcessRunner {
    fn run(&self, _command: &ProcessCommand) -> Result<ProcessOutput, RepoctlError> {
        Ok(self.output.clone())
    }
}

/// Deterministic fake template engine useful for tests.
#[derive(Clone, Debug, Default)]
pub struct FakeTemplateEngine;

impl TemplateEngine for FakeTemplateEngine {
    fn render(&self, request: &RenderRequest) -> Result<RenderedTemplate, RepoctlError> {
        Ok(RenderedTemplate {
            bytes: request.file.target.as_bytes().to_vec(),
        })
    }
}

/// Converts a discovered edge into a graph edge.
pub fn discovered_to_graph_edge(edge: &DiscoveredEdge) -> GraphEdge {
    GraphEdge {
        from: format!("project:{}", edge.from_project),
        to: format!("project:{}", edge.to_project),
        kind: edge.kind.clone(),
        evidence: edge.evidence.clone(),
    }
}
