//! Validated domain model and facade request/response DTOs.

use std::{
    collections::BTreeMap,
    fmt::{Display, Formatter},
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use globset::Glob;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;

const NAME_MAX_BYTES: usize = 128;
const OWNER_MAX_BYTES: usize = 64;
const TASK_MAX_BYTES: usize = 64;
const SCHEMA_MAX_BYTES: usize = 96;
const PATH_MAX_BYTES: usize = 512;
const COMMAND_PART_MAX_BYTES: usize = 256;
const COMMAND_ARG_LIMIT: usize = 64;

/// Repository layout supported by repoctl.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoLayout {
    /// Functional top-level layout required for v0.2.
    Functional,
}

/// Name of a repo.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RepoName(String);

impl RepoName {
    /// Validates and creates a repo name.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_ascii_identifier("repo name", &value, NAME_MAX_BYTES, false)?;
        Ok(Self(value))
    }

    /// Returns the repo name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for RepoName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Globally unique project identifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectName(String);

impl ProjectName {
    /// Validates and creates a project name.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_ascii_identifier("project name", &value, NAME_MAX_BYTES, true)?;
        Ok(Self(value))
    }

    /// Returns the project name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProjectName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Workspace identifier unique within a project.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct WorkspaceName(String);

impl WorkspaceName {
    /// Validates and creates a workspace name.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_ascii_identifier("workspace name", &value, NAME_MAX_BYTES, false)?;
        Ok(Self(value))
    }

    /// Returns the workspace name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for WorkspaceName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Owner handle such as `@platform`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OwnerHandle(String);

impl OwnerHandle {
    /// Validates and creates an owner handle.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if !value.starts_with('@') {
            return Err(Diagnostic::error(
                "manifest.owner.invalid",
                "owner handle must start with @",
            ));
        }
        validate_ascii_identifier("owner handle", &value[1..], OWNER_MAX_BYTES - 1, false)?;
        Ok(Self(value))
    }

    /// Returns the owner handle as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for OwnerHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Task identifier unique within a project manifest.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TaskName(String);

impl TaskName {
    /// Validates and creates a task name.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_ascii_identifier("task name", &value, TASK_MAX_BYTES, false)?;
        Ok(Self(value))
    }

    /// Returns the task name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TaskName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Manifest schema identifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SchemaId(String);

impl SchemaId {
    /// Validates and creates a schema identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty() || value.len() > SCHEMA_MAX_BYTES {
            return Err(Diagnostic::error(
                "manifest.schema.invalid",
                "schema id must be non-empty and length-bounded",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'-' | b'_'))
        {
            return Err(Diagnostic::error(
                "manifest.schema.invalid",
                "schema id contains unsupported characters",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the schema identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SchemaId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Proto package identifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProtoPackageName(String);

impl ProtoPackageName {
    /// Validates and creates a proto package name.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_ascii_identifier("proto package", &value, NAME_MAX_BYTES, true)?;
        Ok(Self(value))
    }

    /// Returns the proto package name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProtoPackageName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Repo-relative path that cannot escape the repo root.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RepoRelativePath(String);

impl RepoRelativePath {
    /// Validates and normalizes a repo-relative path.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        normalize_relative_path(value.into(), false).map(Self)
    }

    /// Creates the root path `.`.
    pub fn root() -> Self {
        Self(".".to_string())
    }

    /// Returns the path as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Joins a project-relative path below this repo-relative path.
    pub fn join_project(&self, child: &ProjectRelativePath) -> Result<Self, Diagnostic> {
        if child.as_str() == "." {
            return Ok(self.clone());
        }
        let joined = if self.0 == "." {
            child.as_str().to_string()
        } else {
            format!("{}/{}", self.0, child.as_str())
        };
        Self::new(joined)
    }

    /// Returns true if this path begins with the given repo-relative prefix.
    pub fn starts_with(&self, prefix: &RepoRelativePath) -> bool {
        self.0 == prefix.0 || prefix.0 == "." || self.0.starts_with(&format!("{}/", prefix.0))
    }
}

impl Display for RepoRelativePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for RepoRelativePath {
    fn default() -> Self {
        Self::root()
    }
}

/// Project-relative path that cannot escape its project root.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectRelativePath(String);

impl ProjectRelativePath {
    /// Validates and normalizes a project-relative path.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        normalize_relative_path(value.into(), true).map(Self)
    }

    /// Creates the root path `.`.
    pub fn root() -> Self {
        Self(".".to_string())
    }

    /// Returns the path as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProjectRelativePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Glob pattern scoped to repo-relative or project-relative paths.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RepoGlob(String);

impl RepoGlob {
    /// Validates a glob pattern and rejects path traversal.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        validate_path_text(&value, true)?;
        Glob::new(&value).map_err(|source| {
            Diagnostic::error(
                "manifest.glob.invalid",
                format!("invalid glob pattern `{value}`: {source}"),
            )
        })?;
        Ok(Self(value))
    }

    /// Returns the glob as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for RepoGlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Absolute repository root.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRoot {
    /// Absolute UTF-8 path to the repository root.
    pub absolute: Utf8PathBuf,
}

impl RepoRoot {
    /// Creates a repository root from an absolute UTF-8 path.
    pub fn new(path: Utf8PathBuf) -> Result<Self, Diagnostic> {
        if !Utf8Path::new(path.as_str()).is_absolute() {
            return Err(Diagnostic::error(
                "repo.root.relative",
                "repo root must be an absolute path",
            ));
        }
        Ok(Self { absolute: path })
    }

    /// Joins a repo-relative path against the absolute root.
    pub fn join(&self, path: &RepoRelativePath) -> Utf8PathBuf {
        if path.as_str() == "." {
            self.absolute.clone()
        } else {
            self.absolute.join(path.as_str())
        }
    }
}

/// Project kind.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectKind {
    /// Product or service application boundary.
    App,
    /// Shared capability extracted behind public facades.
    Framework,
    /// Company-level foundation service.
    FoundationService,
    /// Central proto source ownership project.
    ProtoRoot,
    /// Shared infrastructure baseline.
    CoreInfra,
    /// Independently owned component under the core infrastructure lane.
    CoreInfraComponent,
    /// Developer, agent, or repository automation tooling.
    Tool,
}

/// Workspace execution toolchain.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Toolchain {
    /// Cargo toolchain.
    Cargo,
    /// npm toolchain.
    Npm,
    /// pnpm toolchain.
    Pnpm,
    /// Yarn toolchain.
    Yarn,
    /// Bun toolchain.
    Bun,
    /// uv toolchain.
    Uv,
    /// Custom toolchain name.
    Custom(String),
}

impl Toolchain {
    /// Parses a manifest toolchain value.
    pub fn parse(value: &str) -> Result<Self, Diagnostic> {
        if value.is_empty() || value.len() > NAME_MAX_BYTES {
            return Err(Diagnostic::error(
                "manifest.workspace.toolchain",
                "toolchain must be non-empty and length-bounded",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(Diagnostic::error(
                "manifest.workspace.toolchain",
                "toolchain contains unsupported characters",
            ));
        }
        Ok(match value {
            "cargo" => Self::Cargo,
            "npm" => Self::Npm,
            "pnpm" => Self::Pnpm,
            "yarn" => Self::Yarn,
            "bun" => Self::Bun,
            "uv" => Self::Uv,
            custom => Self::Custom(custom.to_string()),
        })
    }

    /// Returns the manifest representation.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
            Self::Uv => "uv",
            Self::Custom(value) => value.as_str(),
        }
    }
}

/// Workspace language.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceLanguage {
    /// Rust workspace.
    Rust,
    /// TypeScript or JavaScript workspace.
    #[serde(rename = "typescript")]
    TypeScript,
    /// Python workspace.
    Python,
    /// Protocol buffer workspace.
    Proto,
    /// Infrastructure workspace.
    Iac,
}

/// Project visibility.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    /// Internal project visibility.
    #[default]
    Internal,
    /// Public project visibility.
    Public,
}

/// `IaC` provider.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IacProvider {
    /// Pulumi provider.
    Pulumi,
    /// Terraform provider.
    Terraform,
    /// `OpenTofu` provider.
    #[serde(rename = "opentofu")]
    OpenTofu,
}

/// Generated-code placement policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeneratedCodePolicy {
    /// Generated code lives under each consumer project.
    #[default]
    ConsumerLocal,
}

/// Validated repo-level manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoManifest {
    /// Manifest schema id.
    pub schema: SchemaId,
    /// Repository name.
    pub name: RepoName,
    /// Required repository layout.
    pub layout: RepoLayout,
    /// Default owner used when project manifests omit owners.
    pub default_owner: Option<OwnerHandle>,
    /// Proto source root.
    pub protos_root: RepoRelativePath,
    /// Core infrastructure root.
    pub core_infra_root: RepoRelativePath,
    /// Agent skills root.
    pub agent_skills_root: RepoRelativePath,
    /// Claude skills root.
    pub claude_skills_root: RepoRelativePath,
    /// Context output root.
    pub context_output: RepoRelativePath,
    /// Generated-code policy.
    pub generated_code_policy: GeneratedCodePolicy,
    /// Global policy settings.
    pub policies: RepoPolicySet,
}

/// Repo-level policy configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPolicySet {
    /// Cross-app dependency policy mode.
    pub cross_app_dependency: PolicyMode,
    /// Framework internal dependency policy mode.
    pub framework_internal_dependency: PolicyMode,
    /// Generated-code direct-edit policy mode.
    pub generated_code_direct_edit: PolicyMode,
    /// Required owners for production changes.
    pub prod_change_required_owners: Vec<OwnerHandle>,
}

/// Policy enforcement mode.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyMode {
    /// Deny the matched condition.
    #[default]
    Deny,
    /// Warn on the matched condition.
    Warn,
    /// Allow the matched condition.
    Allow,
}

/// Validated workspace specification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpec {
    /// Workspace id.
    pub name: WorkspaceName,
    /// Workspace language.
    pub language: WorkspaceLanguage,
    /// Optional execution toolchain such as `npm`, `bun`, or `uv`.
    pub toolchain: Option<Toolchain>,
    /// Workspace root relative to the project root.
    pub root: ProjectRelativePath,
    /// Workspace manifest path relative to the project root.
    pub manifest: ProjectRelativePath,
    /// Optional lockfile path relative to the project root.
    pub lockfile: Option<ProjectRelativePath>,
    /// Optional repo-level target directory.
    pub target_dir: Option<RepoRelativePath>,
    /// Optional repo-level cache directory.
    pub cache_dir: Option<RepoRelativePath>,
}

/// Task command declared for a workspace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCommand {
    /// Workspace that should execute the command.
    pub workspace: WorkspaceName,
    /// Command in argv form.
    pub command: CommandSpec,
    /// Commands that must run before this command.
    pub depends_on: Vec<TaskDependency>,
}

/// Task prerequisite edge.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDependency {
    /// Project containing the prerequisite task.
    pub project: ProjectName,
    /// Workspace containing the prerequisite task.
    pub workspace: WorkspaceName,
    /// Task to run first.
    pub task: TaskName,
}

/// Task run plan command in argv form.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    /// Executable program.
    pub program: String,
    /// Command arguments.
    pub args: Vec<String>,
}

impl CommandSpec {
    /// Parses a PRD-compatible command string into argv form.
    pub fn parse(value: &str) -> Result<Self, Diagnostic> {
        if value.trim().is_empty() || value.len() > COMMAND_PART_MAX_BYTES * COMMAND_ARG_LIMIT {
            return Err(Diagnostic::error(
                "manifest.command.invalid",
                "command must be non-empty and length-bounded",
            ));
        }
        reject_shell_syntax(value)?;
        let parts = split_command(value)?;
        if parts.is_empty() {
            return Err(Diagnostic::error(
                "manifest.command.invalid",
                "command must include a program",
            ));
        }
        if parts.len() > COMMAND_ARG_LIMIT {
            return Err(Diagnostic::error(
                "manifest.command.too_many_args",
                "command has too many arguments",
            ));
        }
        for part in &parts {
            if part.len() > COMMAND_PART_MAX_BYTES {
                return Err(Diagnostic::error(
                    "manifest.command.part_too_long",
                    "command part exceeds byte limit",
                ));
            }
        }
        let mut parts = parts.into_iter();
        let program = parts.next().ok_or_else(|| {
            Diagnostic::error("manifest.command.invalid", "command must include a program")
        })?;
        Ok(Self {
            program,
            args: parts.collect(),
        })
    }
}

/// Dependency target surface requested by a manifest or discovered adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencySurface {
    /// No specific surface was requested.
    Unspecified,
    /// Public facade of a framework.
    FrameworkFacade,
    /// Internal area of a framework.
    FrameworkInternal,
    /// Public client of a foundation service.
    FoundationPublicClient,
    /// Internal area of a foundation service.
    FoundationInternal,
    /// Public module of core infrastructure.
    CoreInfraPublicModule,
    /// Internal module of core infrastructure.
    CoreInfraInternalModule,
}

/// Dependency target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyTarget {
    /// Dependency points to a project.
    Project(ProjectName),
    /// Dependency points to a proto package.
    ProtoPackage(ProtoPackageName),
}

/// Validated dependency declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDependency {
    /// Raw dependency id as written in the manifest.
    pub id: String,
    /// Resolved target.
    pub target: DependencyTarget,
    /// Requested dependency surface.
    pub surface: DependencySurface,
}

impl ProjectDependency {
    /// Parses a dependency declaration.
    pub fn parse(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.starts_with("protos.") {
            return Ok(Self {
                target: DependencyTarget::ProtoPackage(ProtoPackageName::new(value.clone())?),
                id: value,
                surface: DependencySurface::Unspecified,
            });
        }
        let (target, surface) = if let Some(stripped) = value.strip_suffix(".client") {
            (
                ProjectName::new(stripped.to_string())?,
                DependencySurface::FoundationPublicClient,
            )
        } else if let Some(stripped) = value.strip_suffix(".internal") {
            let surface = if stripped.starts_with("frameworks.") {
                DependencySurface::FrameworkInternal
            } else if stripped.starts_with("foundations.") {
                DependencySurface::FoundationInternal
            } else {
                DependencySurface::Unspecified
            };
            (ProjectName::new(stripped.to_string())?, surface)
        } else if let Some(stripped) = value.strip_suffix(".facade") {
            (
                ProjectName::new(stripped.to_string())?,
                DependencySurface::FrameworkFacade,
            )
        } else {
            (
                ProjectName::new(value.clone())?,
                DependencySurface::Unspecified,
            )
        };
        Ok(Self {
            id: value,
            target: DependencyTarget::Project(target),
            surface,
        })
    }
}

/// Project proto ownership and consumption specification.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProtoSpec {
    /// Proto glob patterns owned by the project.
    pub owns: Vec<RepoGlob>,
    /// Proto glob patterns consumed by the project.
    pub consumes: Vec<RepoGlob>,
}

/// Project `IaC` specification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IacSpec {
    /// `IaC` root relative to the project root.
    pub root: ProjectRelativePath,
    /// `IaC` provider.
    pub provider: IacProvider,
    /// Declared stack names.
    pub stacks: Vec<String>,
}

/// Project deployment specification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploySpec {
    /// Deploy root relative to the project root.
    pub root: ProjectRelativePath,
    /// Environment names.
    pub environments: Vec<String>,
}

/// Project AI edit policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAiSpec {
    /// Editable path globs.
    pub editable: Vec<RepoGlob>,
    /// Do-not-edit path globs.
    pub do_not_edit: Vec<RepoGlob>,
    /// Documentation paths.
    pub docs: Vec<ProjectRelativePath>,
}

/// Public and internal project areas used for boundary classification.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAreas {
    /// Public facade paths keyed by language.
    pub public_facades: BTreeMap<String, Vec<ProjectRelativePath>>,
    /// Public client paths keyed by language.
    pub public_clients: BTreeMap<String, Vec<ProjectRelativePath>>,
    /// Internal paths keyed by language.
    pub internal: BTreeMap<String, Vec<ProjectRelativePath>>,
    /// Public core-infra modules.
    pub public_modules: Vec<ProjectRelativePath>,
    /// Internal core-infra modules.
    pub internal_modules: Vec<ProjectRelativePath>,
}

/// Validated project manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    /// Manifest schema id.
    pub schema: SchemaId,
    /// Project id.
    pub name: ProjectName,
    /// Project kind.
    pub kind: ProjectKind,
    /// Project root path in the repository.
    pub path: RepoRelativePath,
    /// Project owners.
    pub owners: Vec<OwnerHandle>,
    /// Project visibility.
    pub visibility: Visibility,
    /// Workspaces declared by the project.
    pub workspaces: Vec<WorkspaceSpec>,
    /// Project dependencies declared in the manifest.
    pub depends_on: Vec<ProjectDependency>,
    /// Task definitions keyed by task name.
    pub tasks: BTreeMap<TaskName, Vec<TaskCommand>>,
    /// Optional `IaC` specification.
    pub iac: Option<IacSpec>,
    /// Optional deployment specification.
    pub deploy: Option<DeploySpec>,
    /// Optional proto specification.
    pub protos: ProjectProtoSpec,
    /// Optional AI policy specification.
    pub ai: ProjectAiSpec,
    /// Public and internal areas.
    pub areas: ProjectAreas,
    /// Project-local policy flags.
    pub policies: BTreeMap<String, serde_json::Value>,
    /// Source manifest path.
    pub source: RepoRelativePath,
}

impl ProjectManifest {
    /// Returns the project node id.
    pub fn node_id(&self) -> String {
        format!("project:{}", self.name)
    }

    /// Returns true when the project owns the given repo-relative path.
    pub fn contains_path(&self, path: &RepoRelativePath) -> bool {
        path.starts_with(&self.path)
    }
}

/// Template manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateManifest {
    /// Manifest schema id.
    pub schema: SchemaId,
    /// Template name.
    pub name: String,
    /// Template kind.
    pub kind: String,
    /// Template engine.
    pub engine: String,
    /// Template inputs.
    pub inputs: Vec<TemplateInput>,
    /// Template file mappings.
    pub files: Vec<TemplateFile>,
    /// Post-render validation commands.
    pub post_render_validate: Vec<CommandSpec>,
}

/// Template source selector.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TemplateSource {
    /// Built-in template by stable name.
    Builtin {
        /// Built-in template name.
        name: String,
    },
    /// Repository-local template root.
    Local {
        /// Repo-relative template root.
        root: RepoRelativePath,
    },
}

/// Resolved template source and manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTemplateSource {
    /// Source root used to resolve template files.
    pub root: RepoRelativePath,
    /// Parsed template manifest.
    pub manifest: TemplateManifest,
}

/// Template input declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInput {
    /// Input name.
    pub name: String,
    /// Input type.
    pub input_type: String,
    /// Whether the input is required.
    pub required: bool,
    /// Optional JSON default value.
    pub default: Option<serde_json::Value>,
}

/// Template file declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFile {
    /// Template source path.
    pub source: ProjectRelativePath,
    /// Render target expression.
    pub target: String,
    /// Render mode.
    pub mode: String,
    /// Optional condition expression.
    pub when: Option<String>,
}

/// Graph node kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GraphNodeKind {
    /// Project node.
    Project,
    /// Workspace node.
    Workspace,
    /// Proto package or glob node.
    ProtoPackage,
    /// Infrastructure target node.
    IacTarget,
    /// Template node.
    Template,
}

/// Graph node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    /// Stable graph node id.
    pub id: String,
    /// Node kind.
    pub kind: GraphNodeKind,
    /// Human-readable label.
    pub label: String,
    /// Owning project when applicable.
    pub project: Option<ProjectName>,
    /// Owning workspace when applicable.
    pub workspace: Option<WorkspaceName>,
}

/// Graph edge kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// Project dependency edge.
    DependsOnProject,
    /// Project contains workspace edge.
    ContainsWorkspace,
    /// Project consumes proto edge.
    ConsumesProto,
    /// Project owns proto edge.
    OwnsProto,
    /// Project uses framework facade edge.
    UsesFrameworkFacade,
    /// Project uses framework internal edge.
    UsesFrameworkInternal,
    /// Project uses foundation public client edge.
    UsesFoundationClient,
    /// Project uses foundation internal edge.
    UsesFoundationInternal,
    /// Project uses public core infrastructure module edge.
    UsesCoreInfraModule,
    /// Project uses internal core infrastructure module edge.
    UsesCoreInfraInternalModule,
    /// Project owns `IaC` target edge.
    OwnsIac,
    /// Project runs task edge.
    RunsTask,
}

/// Graph edge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Edge kind.
    pub kind: EdgeKind,
    /// Evidence path or manifest value.
    pub evidence: Option<String>,
}

/// Repository graph.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoGraph {
    /// Graph nodes.
    pub nodes: Vec<GraphNode>,
    /// Graph edges.
    pub edges: Vec<GraphEdge>,
}

impl RepoGraph {
    /// Adds a graph node if it is not already present.
    pub fn add_node(&mut self, node: GraphNode) {
        if !self.nodes.iter().any(|existing| existing.id == node.id) {
            self.nodes.push(node);
        }
    }

    /// Adds a graph edge if it is not already present.
    pub fn add_edge(&mut self, edge: GraphEdge) {
        if !self.edges.iter().any(|existing| existing == &edge) {
            self.edges.push(edge);
        }
    }
}

/// Snapshot of the discovered repository.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSnapshot {
    /// Absolute repository root.
    pub root: RepoRoot,
    /// Repo-level manifest.
    pub repo_manifest: RepoManifest,
    /// Discovered project manifests.
    pub projects: Vec<ProjectManifest>,
    /// Built repository graph.
    pub graph: RepoGraph,
    /// Generated-code policy.
    pub generated_policy: GeneratedCodePolicy,
    /// Unix timestamp in seconds when discovery completed.
    pub discovered_at_unix: u64,
}

impl RepoSnapshot {
    /// Creates a repository snapshot.
    pub fn new(
        root: RepoRoot,
        repo_manifest: RepoManifest,
        projects: Vec<ProjectManifest>,
        graph: RepoGraph,
    ) -> Self {
        let discovered_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let generated_policy = repo_manifest.generated_code_policy.clone();
        Self {
            root,
            repo_manifest,
            projects,
            graph,
            generated_policy,
            discovered_at_unix,
        }
    }

    /// Finds a project by name.
    pub fn project(&self, name: &ProjectName) -> Option<&ProjectManifest> {
        self.projects.iter().find(|project| &project.name == name)
    }
}

/// Request to discover a repository.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
}

/// Request to validate a graph and boundary policies.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphValidateRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Changed files used by path-based policy rules.
    pub changed_files: Vec<RepoRelativePath>,
    /// Validation depth.
    pub mode: ValidationMode,
}

/// Graph validation mode.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationMode {
    /// Structural manifest and policy checks only.
    #[default]
    Structural,
    /// Structural checks plus offline metadata checks when available.
    Metadata,
    /// Metadata checks plus full task-planning/environment checks.
    Full,
}

/// Request to print a graph.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPrintRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
}

/// Report returned for graph printing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPrintReport {
    /// Repository snapshot.
    pub snapshot: RepoSnapshot,
}

/// Request to explain a selector.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Project name or graph node id.
    pub selector: String,
}

/// Explanation report for a graph selector.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainReport {
    /// Requested selector.
    pub selector: String,
    /// Matching graph nodes.
    pub nodes: Vec<GraphNode>,
    /// Edges touching the matching nodes.
    pub edges: Vec<GraphEdge>,
    /// Diagnostics produced while explaining.
    pub diagnostics: Vec<Diagnostic>,
}

/// Request to lint boundaries.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryLintRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Changed files used by path-based policy rules.
    pub changed_files: Vec<RepoRelativePath>,
}

/// Boundary lint report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryLintReport {
    /// Boundary diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Init profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitProfile {
    /// Startup profile.
    Startup,
    /// Enterprise profile.
    Enterprise,
}

/// Request for repository initialization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitRequest {
    /// Target repo root.
    pub repo_root: PathBuf,
    /// Repository name.
    pub name: RepoName,
    /// Initialization profile.
    pub profile: InitProfile,
    /// Repository layout.
    pub layout: RepoLayout,
    /// Whether to only plan writes.
    pub dry_run: bool,
}

/// Planned file operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperation {
    /// Target path.
    pub path: RepoRelativePath,
    /// Operation kind.
    pub operation: String,
    /// UTF-8 content to write for file operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Repository initialization plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitPlan {
    /// Planned operations.
    pub operations: Vec<FileOperation>,
    /// Warnings.
    pub warnings: Vec<Diagnostic>,
    /// Recommended next steps.
    pub next_steps: Vec<String>,
}

/// Request to create a new project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProjectRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Project kind.
    pub kind: ProjectKind,
    /// Project path.
    pub path: RepoRelativePath,
    /// Requested stack entries such as `rust-api`, `bun-web`, and `uv-jobs`.
    pub stack: Vec<String>,
    /// Requested languages for framework/foundation generation.
    pub languages: Vec<String>,
    /// Requested public clients for foundation generation.
    pub clients: Vec<String>,
    /// Whether framework generation should include facade/internal areas.
    pub facade: bool,
    /// Optional `IaC` provider.
    pub iac: Option<IacProvider>,
    /// Optional proto package.
    pub proto: Option<ProtoPackageName>,
    /// Optional owner handle.
    pub owner: Option<OwnerHandle>,
    /// Whether to only plan writes.
    pub dry_run: bool,
}

/// Render plan returned by project and template operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPlan {
    /// Planned file operations.
    pub operations: Vec<FileOperation>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Template listing request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
}

/// Template summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummary {
    /// Template source label.
    pub source: String,
    /// Template name.
    pub name: String,
    /// Template kind.
    pub kind: String,
}

/// Template listing report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListReport {
    /// Available templates.
    pub templates: Vec<TemplateSummary>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Template render request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRenderRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Source to render.
    pub source: TemplateSource,
    /// JSON input values.
    pub inputs: serde_json::Value,
    /// Whether to only plan writes.
    pub dry_run: bool,
}

/// Request for affected analysis.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Optional base git ref.
    pub base: Option<String>,
    /// Optional head git ref.
    pub head: Option<String>,
    /// Changed files.
    pub changed_files: Vec<RepoRelativePath>,
    /// Requested task names for affected reports.
    pub tasks: Vec<TaskName>,
}

/// Affected analysis report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedReport {
    /// Directly affected project ids.
    pub directly_affected: Vec<ProjectName>,
    /// Transitively affected project ids.
    pub transitively_affected: Vec<ProjectName>,
    /// Affected workspaces in `project:workspace` form.
    pub workspaces: Vec<String>,
    /// Affected task ids in `project:workspace:task` form.
    pub tasks: Vec<String>,
    /// Risk flags emitted by affected analysis.
    pub risk_flags: Vec<String>,
    /// Explainable reasons.
    pub reasons: Vec<AffectedReason>,
    /// Suggested reviewers.
    pub suggested_reviewers: Vec<OwnerHandle>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reason attached to an affected result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedReason {
    /// Changed or propagated source.
    pub source: String,
    /// Affected target.
    pub target: String,
    /// Human-readable explanation.
    pub reason: String,
}

/// Request to run repo tasks.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Requested task names.
    pub tasks: Vec<TaskName>,
    /// Optional project filter.
    pub projects: Vec<ProjectName>,
    /// Optional workspace filter in `project:workspace` form.
    pub workspaces: Vec<String>,
    /// Run only affected tasks.
    pub affected: bool,
    /// Changed files used when `affected` is true.
    pub changed_files: Vec<RepoRelativePath>,
    /// Optional base git ref used when `affected` is true.
    pub base: Option<String>,
    /// Optional head git ref used when `affected` is true.
    pub head: Option<String>,
    /// Maximum task concurrency.
    pub concurrency: Option<NonZeroU32>,
    /// Plan tasks without executing them.
    pub dry_run: bool,
}

/// Task run plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunPlan {
    /// Planned process commands.
    pub commands: Vec<ProcessCommand>,
    /// Maximum concurrency.
    pub concurrency: NonZeroUsize,
}

/// Task run report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunReport {
    /// Planned task commands.
    pub commands: Vec<ProcessCommand>,
    /// Command outputs.
    pub outputs: Vec<TaskCommandOutput>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Output from one task command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCommandOutput {
    /// Project id.
    pub project: ProjectName,
    /// Workspace id.
    pub workspace: WorkspaceName,
    /// Task id.
    pub task: TaskName,
    /// Process output.
    pub output: ProcessOutput,
}

/// Process command passed to a runner.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessCommand {
    /// Project id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectName>,
    /// Workspace id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceName>,
    /// Task id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskName>,
    /// Working directory.
    pub cwd: RepoRelativePath,
    /// Absolute working directory for local execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absolute_cwd: Option<PathBuf>,
    /// Program.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Environment overlay.
    pub env: BTreeMap<String, String>,
}

/// Process output from a runner.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutput {
    /// Exit status code.
    pub status: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Request for CI matrix generation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiMatrixRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Requested task names.
    pub tasks: Vec<TaskName>,
    /// Changed files used for affected CI matrix generation.
    pub changed_files: Vec<RepoRelativePath>,
    /// Optional base git ref.
    pub base: Option<String>,
    /// Optional head git ref.
    pub head: Option<String>,
    /// Behavior when affected-file detection cannot select entries.
    pub fallback: CiFallback,
}

/// CI fallback behavior.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CiFallback {
    /// Include all projects with requested tasks.
    All,
    /// Include no projects.
    #[default]
    None,
    /// Fail when no changed-file signal is available.
    Error,
}

/// CI matrix report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiMatrixReport {
    /// JSON matrix entries.
    pub entries: Vec<serde_json::Value>,
    /// GitHub Actions-safe matrix object.
    pub github_actions: serde_json::Value,
}

/// Supported CI workflow provider.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CiProvider {
    /// GitHub Actions.
    #[default]
    GitHubActions,
}

/// Request to render a CI workflow.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiWorkflowRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Workflow provider.
    pub provider: CiProvider,
    /// Write the generated workflow file.
    pub write: bool,
}

/// Rendered CI workflow.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiWorkflowReport {
    /// Workflow file path.
    pub path: RepoRelativePath,
    /// Workflow content.
    pub content: String,
    /// Planned or applied operations.
    pub operations: Vec<FileOperation>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Hygiene request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneCheckRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
}

/// Hygiene clean request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneCleanRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Plan without deleting files.
    pub dry_run: bool,
}

/// Hygiene report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneReport {
    /// Hygiene diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Cleanable generated-artifact operations.
    pub operations: Vec<FileOperation>,
}

/// Dependency rewrite mode for adoption planning.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyRewriteMode {
    /// Rewrite supported dependencies automatically.
    #[default]
    Auto,
    /// Do not inspect or rewrite dependencies.
    Off,
    /// Report rewrite candidates without changing copied files.
    ReportOnly,
}

/// CI behavior for adoption planning.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdoptionCiMode {
    /// Update generated CI files.
    #[default]
    Update,
    /// Do not generate CI changes.
    Off,
    /// Report CI changes without applying them.
    ReportOnly,
}

/// Adoption output format.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdoptionOutputFormat {
    /// Human-readable report.
    #[default]
    Human,
    /// JSON report.
    Json,
    /// GitHub Actions format.
    GitHubActions,
}

/// Request to create an adoption plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionPlanRequest {
    /// Directory containing source repositories or a single source repository.
    pub source: PathBuf,
    /// Destination initialized monorepo.
    pub dest: PathBuf,
    /// Source names to include.
    pub include: Vec<String>,
    /// Source names to exclude.
    pub exclude: Vec<String>,
    /// Placement overrides keyed by source name.
    pub map: BTreeMap<String, RepoRelativePath>,
    /// Project-kind overrides keyed by source name.
    pub kind: BTreeMap<String, ProjectKind>,
    /// Owner overrides keyed by source name.
    pub owner: BTreeMap<String, OwnerHandle>,
    /// Dependency rewrite mode.
    pub rewrite_deps: DependencyRewriteMode,
    /// CI generation behavior.
    pub ci: AdoptionCiMode,
    /// Verification mode.
    pub verification: ValidationMode,
    /// Requested output format.
    pub format: AdoptionOutputFormat,
}

/// Request to apply a reviewed adoption plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionApplyRequest {
    /// Plan JSON file.
    pub plan: PathBuf,
    /// Recompute inference before applying.
    pub refresh: bool,
}

/// Request to verify a reviewed adoption plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionVerifyRequest {
    /// Plan JSON file.
    pub plan: PathBuf,
}

/// Adoption plan.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionPlan {
    /// Source root.
    pub source_root: Utf8PathBuf,
    /// Destination repo root.
    pub dest_root: Utf8PathBuf,
    /// Source repository decisions.
    pub sources: Vec<AdoptedSource>,
    /// Copy operations.
    pub operations: Vec<AdoptionFileOperation>,
    /// Dependency rewrite operations.
    pub dependency_rewrites: Vec<DependencyRewrite>,
    /// Synthesized manifests.
    pub manifest_syntheses: Vec<ProjectManifestSynthesis>,
    /// CI file operations.
    pub ci_operations: Vec<FileOperation>,
    /// Verification plan.
    pub verification: VerificationPlan,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Source repository selected for adoption.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptedSource {
    /// Source repository name.
    pub name: String,
    /// Source path.
    pub source_path: Utf8PathBuf,
    /// Destination path.
    pub destination_path: RepoRelativePath,
    /// Inferred kind.
    pub inferred_kind: ProjectKind,
    /// Confidence score from 0.0 to 1.0.
    pub confidence: f32,
    /// Inference reasons.
    pub reasons: Vec<String>,
    /// Inventory snapshot.
    pub inventory: SourceInventory,
    /// Whether this source is skipped.
    pub skipped: bool,
    /// Whether placement was explicitly overridden.
    pub override_applied: bool,
}

/// Source inventory collected before planning.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInventory {
    /// Repository name.
    pub name: String,
    /// Whether a VCS marker exists.
    pub has_vcs: bool,
    /// README first heading or summary.
    pub readme_summary: Option<String>,
    /// Primary manifest paths.
    pub manifests: Vec<String>,
    /// Top-level directories.
    pub top_level_dirs: Vec<String>,
    /// Dependency package names referenced by source manifests.
    pub dependency_references: Vec<String>,
    /// Generated artifact paths found in the source.
    pub generated_artifacts: Vec<String>,
    /// Required local tools inferred from manifests and build scripts.
    pub required_tools: Vec<String>,
}

/// Adoption file operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionFileOperation {
    /// Operation id.
    pub id: String,
    /// Operation kind.
    pub operation: String,
    /// Optional source file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<Utf8PathBuf>,
    /// Destination path relative to the destination repo root.
    pub destination_path: RepoRelativePath,
    /// Optional UTF-8 replacement content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// File checksum when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

/// Dependency rewrite record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyRewrite {
    /// File being rewritten.
    pub file: RepoRelativePath,
    /// Package or crate dependency name.
    pub package: String,
    /// Original dependency value.
    pub from: String,
    /// Replacement dependency value.
    pub to: String,
    /// Dependency surface.
    pub surface: DependencySurface,
    /// Owning project.
    pub owner_project: ProjectName,
}

/// Synthesized project manifest record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifestSynthesis {
    /// Source repository name.
    pub source: String,
    /// Project name.
    pub project: ProjectName,
    /// Manifest path.
    pub manifest_path: RepoRelativePath,
    /// Synthesized YAML.
    pub content: String,
}

/// Verification plan generated for adoption.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationPlan {
    /// Tool prerequisites.
    pub prerequisites: Vec<ToolPrerequisite>,
    /// Ordered commands.
    pub commands: Vec<ProcessCommand>,
}

/// Required local tool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPrerequisite {
    /// Tool name.
    pub tool: String,
    /// Why it is needed.
    pub reason: String,
}

/// Request for PR summary.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummaryRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Optional base git ref.
    pub base: Option<String>,
    /// Optional head git ref.
    pub head: Option<String>,
    /// Explicit changed files.
    pub changed_files: Vec<RepoRelativePath>,
}

/// PR summary report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    /// Markdown body.
    pub markdown: String,
    /// Machine-readable impact payload.
    pub impact: serde_json::Value,
}

/// Request for AI context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContextRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Project to build context for.
    pub project: ProjectName,
    /// Context audience.
    pub audience: String,
}

/// AI context report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    /// JSON context payload.
    pub payload: serde_json::Value,
}

/// Generated-code check request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodegenCheckRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Optional base git ref.
    pub base: Option<String>,
    /// Optional head git ref.
    pub head: Option<String>,
    /// Explicit changed files.
    pub changed_files: Vec<RepoRelativePath>,
}

/// Generated-code check report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodegenCheckReport {
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Proto facade request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoFacadeRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Proto operation.
    pub operation: ProtoOperation,
    /// Proto path or package selector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Optional base git ref for changed-file checks.
    pub base: Option<String>,
    /// Optional head git ref for changed-file checks.
    pub head: Option<String>,
    /// Explicit changed files for generated-code policy checks.
    pub changed_files: Vec<RepoRelativePath>,
}

/// Proto operation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtoOperation {
    /// Check proto toolchain and generated-code policy.
    #[default]
    Check,
    /// Find owners for a proto path.
    Owners,
    /// Find consumers for a proto path or package.
    Consumers,
}

/// Proto facade report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoFacadeReport {
    /// Matching owner projects.
    pub owners: Vec<ProjectName>,
    /// Matching consumer projects.
    pub consumers: Vec<ProjectName>,
    /// Commands planned or executed by proto toolchains.
    pub commands: Vec<ProcessCommand>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// `IaC` facade request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IacFacadeRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Plan only affected `IaC` targets.
    pub affected: bool,
    /// Explicit project selector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectName>,
    /// Environment or stack name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Plan core infrastructure.
    pub core: bool,
    /// Optional base git ref for affected selection.
    pub base: Option<String>,
    /// Optional head git ref for affected selection.
    pub head: Option<String>,
    /// Explicit changed files for affected selection and risk classification.
    pub changed_files: Vec<RepoRelativePath>,
    /// Plan without executing provider commands.
    pub dry_run: bool,
}

/// `IaC` facade report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IacFacadeReport {
    /// Provider plan commands.
    pub commands: Vec<ProcessCommand>,
    /// Risk flags.
    pub risk_flags: Vec<String>,
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Skills facade request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsFacadeRequest {
    /// Optional starting path or explicit repo root.
    pub repo: Option<PathBuf>,
    /// Whether to write missing or stale skills.
    pub sync: bool,
    /// Whether to only plan writes.
    pub dry_run: bool,
}

/// Skills facade report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsFacadeReport {
    /// Diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Validates project path conventions for the supported kinds.
pub fn validate_project_convention(project: &ProjectManifest) -> Vec<Diagnostic> {
    let expected_prefix = match project.kind {
        ProjectKind::App => "apps/",
        ProjectKind::Framework => "frameworks/",
        ProjectKind::FoundationService => "foundations/",
        ProjectKind::ProtoRoot => "protos",
        ProjectKind::CoreInfra => "core-infra",
        ProjectKind::CoreInfraComponent => "core-infra/",
        ProjectKind::Tool => "tools/",
    };
    let valid = match project.kind {
        ProjectKind::ProtoRoot | ProjectKind::CoreInfra => project.path.as_str() == expected_prefix,
        ProjectKind::App
        | ProjectKind::Framework
        | ProjectKind::FoundationService
        | ProjectKind::CoreInfraComponent
        | ProjectKind::Tool => project.path.as_str().starts_with(expected_prefix),
    };
    if valid {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                "manifest.project.path_convention",
                format!(
                    "project `{}` of kind {:?} must live under `{expected_prefix}`",
                    project.name, project.kind
                ),
            )
            .with_path(project.source.as_str()),
        ]
    }
}

/// Converts a repo path to a UTF-8 path.
pub fn utf8_path_buf(path: PathBuf) -> Result<Utf8PathBuf, Diagnostic> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| {
        Diagnostic::error(
            "path.non_utf8",
            format!("path is not valid UTF-8: {}", path.display()),
        )
    })
}

fn validate_ascii_identifier(
    label: &str,
    value: &str,
    max_bytes: usize,
    allow_dot: bool,
) -> Result<(), Diagnostic> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(Diagnostic::error(
            "manifest.identifier.invalid",
            format!("{label} must be non-empty and length-bounded"),
        ));
    }
    let mut previous_dot = false;
    for byte in value.bytes() {
        let is_dot = byte == b'.';
        let allowed = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'-' | b'_')
            || (allow_dot && is_dot);
        if !allowed {
            return Err(Diagnostic::error(
                "manifest.identifier.invalid",
                format!("{label} contains unsupported characters"),
            ));
        }
        if allow_dot && is_dot && previous_dot {
            return Err(Diagnostic::error(
                "manifest.identifier.invalid",
                format!("{label} cannot contain consecutive dots"),
            ));
        }
        previous_dot = is_dot;
    }
    if allow_dot && (value.starts_with('.') || value.ends_with('.')) {
        return Err(Diagnostic::error(
            "manifest.identifier.invalid",
            format!("{label} cannot start or end with a dot"),
        ));
    }
    Ok(())
}

fn normalize_relative_path(value: String, allow_root: bool) -> Result<String, Diagnostic> {
    validate_path_text(&value, false)?;
    if value == "." {
        return if allow_root {
            Ok(value)
        } else {
            Err(Diagnostic::error(
                "manifest.path.invalid",
                "repo-relative path cannot be the root path",
            ))
        };
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(Diagnostic::error(
                "manifest.path.traversal",
                "relative path cannot contain ..",
            ));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return if allow_root {
            Ok(".".to_string())
        } else {
            Err(Diagnostic::error(
                "manifest.path.invalid",
                "relative path must not be empty",
            ))
        };
    }
    Ok(parts.join("/"))
}

fn validate_path_text(value: &str, allow_glob: bool) -> Result<(), Diagnostic> {
    if value.is_empty() || value.len() > PATH_MAX_BYTES {
        return Err(Diagnostic::error(
            "manifest.path.invalid",
            "path must be non-empty and length-bounded",
        ));
    }
    if value.contains('\0') || value.contains('\\') {
        return Err(Diagnostic::error(
            "manifest.path.invalid",
            "path must not contain NUL bytes or platform separators",
        ));
    }
    if value.starts_with('/') {
        return Err(Diagnostic::error(
            "manifest.path.absolute",
            "path must be relative",
        ));
    }
    if value
        .split('/')
        .any(|part| part == ".." || (!allow_glob && part.contains('*')))
    {
        return Err(Diagnostic::error(
            "manifest.path.traversal",
            "path must not contain traversal or unsupported glob segments",
        ));
    }
    Ok(())
}

fn reject_shell_syntax(value: &str) -> Result<(), Diagnostic> {
    const REJECTED: [&str; 13] = [
        "&&", "||", "|", ";", ">", "<", "$(", "`", "\n", "\r", "*", "?", "[",
    ];
    if let Some(token) = REJECTED.iter().find(|token| value.contains(**token)) {
        return Err(Diagnostic::error(
            "manifest.command.shell_syntax",
            format!("command uses shell-only syntax `{token}`"),
        ));
    }
    Ok(())
}

fn split_command(value: &str) -> Result<Vec<String>, Diagnostic> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match quote {
            Some(active) if character == active => quote = None,
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            Some(_) | None => current.push(character),
        }
    }
    if escaped || quote.is_some() {
        return Err(Diagnostic::error(
            "manifest.command.invalid",
            "command contains an unfinished escape or quote",
        ));
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::{
        CommandSpec, OwnerHandle, ProjectDependency, ProjectRelativePath, RepoRelativePath,
    };

    #[test]
    fn test_should_parse_argv_command() {
        let command = CommandSpec::parse("cargo check --workspace").expect("command parses");
        assert_eq!(command.program, "cargo");
        assert_eq!(command.args, ["check", "--workspace"]);
    }

    #[test]
    fn test_should_reject_shell_syntax_in_command() {
        let error = CommandSpec::parse("cargo check && rm -rf target").expect_err("rejects shell");
        assert_eq!(error.code.as_ref(), "manifest.command.shell_syntax");
    }

    #[test]
    fn test_should_reject_invalid_owner() {
        let error = OwnerHandle::new("platform").expect_err("owner requires @");
        assert_eq!(error.code.as_ref(), "manifest.owner.invalid");
    }

    #[test]
    fn test_should_reject_path_traversal() {
        let error = RepoRelativePath::new("../outside").expect_err("rejects traversal");
        assert_eq!(error.code.as_ref(), "manifest.path.traversal");
    }

    #[test]
    fn test_should_allow_project_root_path() {
        let path = ProjectRelativePath::new(".").expect("root is valid");
        assert_eq!(path.as_str(), ".");
    }

    #[test]
    fn test_should_parse_foundation_client_dependency() {
        let dependency =
            ProjectDependency::parse("foundations.identity.client").expect("dependency parses");
        assert_eq!(dependency.id, "foundations.identity.client");
    }
}
