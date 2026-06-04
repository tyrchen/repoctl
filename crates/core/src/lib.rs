#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
// The public API intentionally mirrors the vocabulary in the v0.2 specs.
#![allow(clippy::module_name_repetitions)]
// Constructors and accessors stay lightweight; callers can decide when `#[must_use]` is useful.
#![allow(clippy::must_use_candidate)]
// Every fallible public function is documented at the type/module level to avoid repetitive noise.
#![allow(clippy::missing_errors_doc)]

//! Core domain, manifest, diagnostic, graph, and port types for repoctl.
//!
//! This crate is intentionally infrastructure-neutral. Concrete filesystem,
//! Cargo, process, and template adapters live outside of it.

pub mod diagnostic;
pub mod domain;
pub mod manifest;
pub mod ports;

pub use diagnostic::{
    Diagnostic, DiagnosticSource, RepoctlError, Severity, SourceSpan, ValidationReport,
};
pub use domain::{
    AdoptedSource, AdoptionApplyRequest, AdoptionCiMode, AdoptionFileOperation,
    AdoptionOutputFormat, AdoptionPlan, AdoptionPlanRequest, AdoptionVerifyRequest, AffectedReason,
    AffectedReport, AffectedRequest, AiContext, AiContextRequest, BoundaryLintReport,
    BoundaryLintRequest, CiFallback, CiMatrixReport, CiMatrixRequest, CiProvider, CiWorkflowReport,
    CiWorkflowRequest, CodegenCheckReport, CodegenCheckRequest, CommandSpec, DependencyRewrite,
    DependencyRewriteMode, DependencySurface, DependencyTarget, DeploySpec, DiscoverRequest,
    EdgeKind, ExplainReport, ExplainRequest, FileOperation, GeneratedCodePolicy, GraphEdge,
    GraphNode, GraphNodeKind, GraphPrintReport, GraphPrintRequest, GraphValidateRequest,
    HygieneCheckRequest, HygieneCleanRequest, HygieneReport, IacFacadeReport, IacFacadeRequest,
    IacProvider, IacSpec, InitPlan, InitProfile, InitRequest, NewProjectRequest, OwnerHandle,
    PolicyMode, PrSummary, PrSummaryRequest, ProcessCommand, ProcessOutput, ProjectAiSpec,
    ProjectAreas, ProjectDependency, ProjectKind, ProjectManifest, ProjectManifestSynthesis,
    ProjectName, ProjectProtoSpec, ProjectRelativePath, ProtoFacadeReport, ProtoFacadeRequest,
    ProtoOperation, ProtoPackageName, RenderPlan, RepoGlob, RepoGraph, RepoLayout, RepoManifest,
    RepoName, RepoPolicySet, RepoRelativePath, RepoRoot, RepoSnapshot, ResolvedTemplateSource,
    SchemaId, SkillsFacadeReport, SkillsFacadeRequest, SourceInventory, TaskCommand,
    TaskCommandOutput, TaskDependency, TaskName, TaskRunPlan, TaskRunReport, TaskRunRequest,
    TemplateFile, TemplateInput, TemplateListReport, TemplateListRequest, TemplateManifest,
    TemplateRenderRequest, TemplateSource, TemplateSummary, ToolPrerequisite, Toolchain,
    ValidationMode, VerificationPlan, Visibility, WorkspaceLanguage, WorkspaceName, WorkspaceSpec,
    utf8_path_buf, validate_project_convention,
};
pub use manifest::{ManifestSource, YamlManifestParser};
pub use ports::{
    DiscoveredEdge, FixedRepoLocator, GraphBuildInput, GraphBuilder, InMemoryRepoFileSystem,
    ManifestParser, PolicyContext, PolicyRule, ProcessRunner, RenderRequest, RenderedTemplate,
    RepoFileSystem, RepoLocator, StaticGraphBuilder, TemplateEngine, TemplateSourceResolver,
    ToolchainAdapter, ToolchainEnvironmentInput, WalkRequest, WorkspaceInspectionInput,
    WorkspaceInspector, discovered_to_graph_edge,
};
