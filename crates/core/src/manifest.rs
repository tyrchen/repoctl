//! YAML manifest parsing and raw-to-domain validation.

use std::collections::{BTreeMap, BTreeSet};

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::{
    diagnostic::{Diagnostic, RepoctlError},
    domain::{
        CommandSpec, DeploySpec, GeneratedCodePolicy, IacProvider, IacSpec, OwnerHandle,
        PolicyMode, ProjectAiSpec, ProjectAreas, ProjectDependency, ProjectKind, ProjectManifest,
        ProjectName, ProjectProtoSpec, ProjectRelativePath, RepoGlob, RepoLayout, RepoManifest,
        RepoName, RepoPolicySet, RepoRelativePath, SchemaId, TaskCommand, TaskName, TemplateFile,
        TemplateInput, TemplateManifest, Visibility, WorkspaceLanguage, WorkspaceName,
        WorkspaceSpec,
    },
    ports::ManifestParser,
};

const MANIFEST_MAX_BYTES: usize = 256 * 1024;
const OWNER_LIMIT: usize = 32;
const WORKSPACE_LIMIT: usize = 64;
const TASK_LIMIT: usize = 128;
const TASK_COMMAND_LIMIT: usize = 64;
const DEPENDENCY_LIMIT: usize = 256;
const PATTERN_LIMIT: usize = 256;
const TEMPLATE_INPUT_LIMIT: usize = 64;
const TEMPLATE_FILE_LIMIT: usize = 512;

/// Source bytes for one manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestSource {
    /// Source path used in diagnostics.
    pub path: String,
    /// Raw manifest bytes.
    pub bytes: Vec<u8>,
}

impl ManifestSource {
    /// Creates a manifest source from bytes.
    pub fn new(path: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            bytes: bytes.into(),
        }
    }
}

/// YAML-backed manifest parser using `serde_norway`.
#[derive(Clone, Debug, Default)]
pub struct YamlManifestParser;

impl YamlManifestParser {
    /// Parses a repo manifest from bytes.
    pub fn parse_repo_bytes(
        &self,
        path: impl Into<String>,
        bytes: &[u8],
    ) -> Result<RepoManifest, RepoctlError> {
        self.parse_repo(ManifestSource::new(path, bytes.to_vec()))
    }

    /// Parses a project manifest from bytes.
    pub fn parse_project_bytes(
        &self,
        path: impl Into<String>,
        bytes: &[u8],
    ) -> Result<ProjectManifest, RepoctlError> {
        self.parse_project(ManifestSource::new(path, bytes.to_vec()))
    }

    /// Parses a template manifest from bytes.
    pub fn parse_template_bytes(
        &self,
        path: impl Into<String>,
        bytes: &[u8],
    ) -> Result<TemplateManifest, RepoctlError> {
        self.parse_template(ManifestSource::new(path, bytes.to_vec()))
    }

    /// Returns the generated JSON Schema for repo manifests.
    pub fn repo_schema() -> serde_json::Value {
        serde_json::to_value(schema_for!(RawRepoManifest)).unwrap_or_else(|_| serde_json::json!({}))
    }

    /// Returns the generated JSON Schema for project manifests.
    pub fn project_schema() -> serde_json::Value {
        serde_json::to_value(schema_for!(RawProjectManifest))
            .unwrap_or_else(|_| serde_json::json!({}))
    }

    /// Returns the generated JSON Schema for template manifests.
    pub fn template_schema() -> serde_json::Value {
        serde_json::to_value(schema_for!(RawTemplateManifest))
            .unwrap_or_else(|_| serde_json::json!({}))
    }
}

impl ManifestParser for YamlManifestParser {
    fn parse_repo(&self, source: ManifestSource) -> Result<RepoManifest, RepoctlError> {
        ensure_size(&source)?;
        let raw: RawRepoManifest = parse_yaml(&source)?;
        raw.into_domain().map_err(|diagnostic| {
            RepoctlError::diagnostic(diagnostic.with_path(source.path.clone()))
        })
    }

    fn parse_project(&self, source: ManifestSource) -> Result<ProjectManifest, RepoctlError> {
        ensure_size(&source)?;
        let raw: RawProjectManifest = parse_yaml(&source)?;
        let source_path = RepoRelativePath::new(source.path.clone()).map_err(|diagnostic| {
            RepoctlError::diagnostic(diagnostic.with_path(source.path.clone()))
        })?;
        raw.into_domain(source_path).map_err(|diagnostic| {
            RepoctlError::diagnostic(diagnostic.with_path(source.path.clone()))
        })
    }

    fn parse_template(&self, source: ManifestSource) -> Result<TemplateManifest, RepoctlError> {
        ensure_size(&source)?;
        let raw: RawTemplateManifest = parse_yaml(&source)?;
        raw.into_domain().map_err(|diagnostic| {
            RepoctlError::diagnostic(diagnostic.with_path(source.path.clone()))
        })
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoManifest {
    schema: String,
    name: String,
    layout: String,
    #[serde(default)]
    defaults: RawRepoDefaults,
    #[serde(default)]
    #[serde(rename = "languages")]
    _languages: serde_json::Value,
    #[serde(default)]
    protos: RawRepoProtos,
    #[serde(default)]
    #[serde(rename = "ci")]
    _ci: serde_json::Value,
    #[serde(default)]
    iac: RawRepoIac,
    #[serde(default)]
    #[serde(rename = "templates")]
    _templates: serde_json::Value,
    #[serde(default)]
    ai: RawRepoAi,
    #[serde(default)]
    policies: RawRepoPolicies,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoDefaults {
    owner: Option<String>,
    #[serde(default)]
    #[serde(rename = "visibility")]
    _visibility: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoProtos {
    #[serde(default = "default_protos_root")]
    root: String,
    #[serde(default)]
    #[serde(rename = "toolchain")]
    _toolchain: Option<String>,
    #[serde(default = "default_generated_policy")]
    generated_code_policy: String,
}

impl Default for RawRepoProtos {
    fn default() -> Self {
        Self {
            root: default_protos_root(),
            _toolchain: None,
            generated_code_policy: default_generated_policy(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoIac {
    #[serde(default = "default_core_infra_root")]
    core_infra_root: String,
    #[serde(default)]
    #[serde(rename = "app_iac_mode")]
    _app_iac_mode: Option<String>,
    #[serde(default)]
    #[serde(rename = "supported")]
    _supported: Vec<String>,
}

impl Default for RawRepoIac {
    fn default() -> Self {
        Self {
            core_infra_root: default_core_infra_root(),
            _app_iac_mode: None,
            _supported: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoAi {
    #[serde(default = "default_agent_skills_root")]
    agent_skills_root: String,
    #[serde(default = "default_claude_skills_root")]
    claude_skills_root: String,
    #[serde(default = "default_context_output")]
    context_output: String,
}

impl Default for RawRepoAi {
    fn default() -> Self {
        Self {
            agent_skills_root: default_agent_skills_root(),
            claude_skills_root: default_claude_skills_root(),
            context_output: default_context_output(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawRepoPolicies {
    #[serde(default)]
    cross_app_dependency: Option<RawModePolicy>,
    #[serde(default)]
    framework_internal_dependency: Option<RawModePolicy>,
    #[serde(default)]
    generated_code: Option<RawGeneratedPolicy>,
    #[serde(default)]
    prod_change: Option<RawProdChangePolicy>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawModePolicy {
    mode: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawGeneratedPolicy {
    direct_edit: String,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawProdChangePolicy {
    #[serde(default)]
    required_owners: Vec<String>,
}

impl RawRepoManifest {
    fn into_domain(self) -> Result<RepoManifest, Diagnostic> {
        let schema = SchemaId::new(self.schema)?;
        if schema.as_str() != "company.repo/v1" {
            return Err(Diagnostic::error(
                "manifest.schema.unsupported",
                "repo manifest schema must be company.repo/v1",
            ));
        }
        let layout = parse_repo_layout(&self.layout)?;
        let default_owner = self.defaults.owner.map(OwnerHandle::new).transpose()?;
        let policies = RepoPolicySet {
            cross_app_dependency: self
                .policies
                .cross_app_dependency
                .map_or(Ok(PolicyMode::Deny), |policy| {
                    parse_policy_mode(&policy.mode)
                })?,
            framework_internal_dependency: self
                .policies
                .framework_internal_dependency
                .map_or(Ok(PolicyMode::Deny), |policy| {
                    parse_policy_mode(&policy.mode)
                })?,
            generated_code_direct_edit: self
                .policies
                .generated_code
                .map_or(Ok(PolicyMode::Deny), |policy| {
                    parse_policy_mode(&policy.direct_edit)
                })?,
            prod_change_required_owners: bounded_vec(
                self.policies
                    .prod_change
                    .map_or_else(Vec::new, |policy| policy.required_owners),
                OWNER_LIMIT,
                "manifest.owner.too_many",
            )?
            .into_iter()
            .map(OwnerHandle::new)
            .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(RepoManifest {
            schema,
            name: RepoName::new(self.name)?,
            layout,
            default_owner,
            protos_root: RepoRelativePath::new(self.protos.root)?,
            core_infra_root: RepoRelativePath::new(self.iac.core_infra_root)?,
            agent_skills_root: RepoRelativePath::new(self.ai.agent_skills_root)?,
            claude_skills_root: RepoRelativePath::new(self.ai.claude_skills_root)?,
            context_output: RepoRelativePath::new(self.ai.context_output)?,
            generated_code_policy: parse_generated_policy(&self.protos.generated_code_policy)?,
            policies,
        })
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawProjectManifest {
    schema: String,
    name: String,
    kind: String,
    path: String,
    #[serde(default)]
    owners: Vec<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    workspaces: Vec<RawWorkspaceSpec>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    tasks: BTreeMap<String, Vec<RawTaskCommand>>,
    #[serde(default)]
    iac: Option<RawIacSpec>,
    #[serde(default)]
    deploy: Option<RawDeploySpec>,
    #[serde(default)]
    protos: RawProjectProtoSpec,
    #[serde(default)]
    ai: RawProjectAiSpec,
    #[serde(default)]
    public_facades: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    public_clients: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    internal: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    modules: RawModulesSpec,
    #[serde(default)]
    policies: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawWorkspaceSpec {
    name: String,
    language: String,
    #[serde(default)]
    toolchain: Option<String>,
    root: String,
    manifest: String,
    #[serde(default)]
    lockfile: Option<String>,
    #[serde(default)]
    target_dir: Option<String>,
    #[serde(default)]
    cache_dir: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTaskCommand {
    workspace: String,
    command: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawIacSpec {
    root: String,
    provider: String,
    #[serde(default)]
    stacks: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawDeploySpec {
    root: String,
    #[serde(default)]
    environments: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawProjectProtoSpec {
    #[serde(default)]
    owns: Vec<String>,
    #[serde(default)]
    consumes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawProjectAiSpec {
    #[serde(default)]
    editable: Vec<String>,
    #[serde(default)]
    do_not_edit: Vec<String>,
    #[serde(default)]
    docs: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawModulesSpec {
    #[serde(default)]
    public: Vec<String>,
    #[serde(default)]
    internal: Vec<String>,
}

impl RawProjectManifest {
    fn into_domain(self, source: RepoRelativePath) -> Result<ProjectManifest, Diagnostic> {
        let schema = SchemaId::new(self.schema)?;
        if schema.as_str() != "company.project/v1" {
            return Err(Diagnostic::error(
                "manifest.schema.unsupported",
                "project manifest schema must be company.project/v1",
            ));
        }
        let workspaces = bounded_vec(
            self.workspaces,
            WORKSPACE_LIMIT,
            "manifest.workspace.too_many",
        )?
        .into_iter()
        .map(RawWorkspaceSpec::into_domain)
        .collect::<Result<Vec<_>, _>>()?;
        validate_unique_workspace_names(&workspaces)?;
        let workspace_names = workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect::<BTreeSet<_>>();
        let tasks = bounded_map(self.tasks, TASK_LIMIT, "manifest.task.too_many")?
            .into_iter()
            .map(|(name, commands)| {
                let name = TaskName::new(name)?;
                let commands = bounded_vec(
                    commands,
                    TASK_COMMAND_LIMIT,
                    "manifest.task_command.too_many",
                )?
                .into_iter()
                .map(RawTaskCommand::into_domain)
                .collect::<Result<Vec<_>, _>>()?;
                for command in &commands {
                    if !workspace_names.contains(&command.workspace) {
                        return Err(Diagnostic::error(
                            "manifest.task.unknown_workspace",
                            format!(
                                "task `{name}` references unknown workspace `{}`",
                                command.workspace
                            ),
                        ));
                    }
                }
                Ok((name, commands))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(ProjectManifest {
            schema,
            name: ProjectName::new(self.name)?,
            kind: parse_project_kind(&self.kind)?,
            path: RepoRelativePath::new(self.path)?,
            owners: bounded_vec(self.owners, OWNER_LIMIT, "manifest.owner.too_many")?
                .into_iter()
                .map(OwnerHandle::new)
                .collect::<Result<Vec<_>, _>>()?,
            visibility: self
                .visibility
                .as_deref()
                .map_or(Ok(Visibility::Internal), parse_visibility)?,
            workspaces,
            depends_on: bounded_vec(
                self.depends_on,
                DEPENDENCY_LIMIT,
                "manifest.dependency.too_many",
            )?
            .into_iter()
            .map(ProjectDependency::parse)
            .collect::<Result<Vec<_>, _>>()?,
            tasks,
            iac: self.iac.map(RawIacSpec::into_domain).transpose()?,
            deploy: self.deploy.map(RawDeploySpec::into_domain).transpose()?,
            protos: self.protos.into_domain()?,
            ai: self.ai.into_domain()?,
            areas: ProjectAreas {
                public_facades: parse_area_map(self.public_facades)?,
                public_clients: parse_area_map(self.public_clients)?,
                internal: parse_area_map(self.internal)?,
                public_modules: bounded_vec(
                    self.modules.public,
                    PATTERN_LIMIT,
                    "manifest.module.too_many",
                )?
                .into_iter()
                .map(ProjectRelativePath::new)
                .collect::<Result<Vec<_>, _>>()?,
                internal_modules: bounded_vec(
                    self.modules.internal,
                    PATTERN_LIMIT,
                    "manifest.module.too_many",
                )?
                .into_iter()
                .map(ProjectRelativePath::new)
                .collect::<Result<Vec<_>, _>>()?,
            },
            policies: self.policies,
            source,
        })
    }
}

impl RawWorkspaceSpec {
    fn into_domain(self) -> Result<WorkspaceSpec, Diagnostic> {
        let root = ProjectRelativePath::new(self.root)?;
        let manifest = ProjectRelativePath::new(self.manifest)?;
        if root.as_str() != "."
            && !manifest
                .as_str()
                .starts_with(&format!("{}/", root.as_str()))
        {
            return Err(Diagnostic::error(
                "manifest.workspace.manifest_outside_root",
                "workspace manifest must live under the workspace root",
            ));
        }
        Ok(WorkspaceSpec {
            name: WorkspaceName::new(self.name)?,
            language: parse_workspace_language(&self.language)?,
            toolchain: self.toolchain,
            root,
            manifest,
            lockfile: self.lockfile.map(ProjectRelativePath::new).transpose()?,
            target_dir: self
                .target_dir
                .as_deref()
                .map(repo_path_from_template)
                .transpose()?,
            cache_dir: self
                .cache_dir
                .as_deref()
                .map(repo_path_from_template)
                .transpose()?,
        })
    }
}

impl RawTaskCommand {
    fn into_domain(self) -> Result<TaskCommand, Diagnostic> {
        Ok(TaskCommand {
            workspace: WorkspaceName::new(self.workspace)?,
            command: CommandSpec::parse(&self.command)?,
        })
    }
}

impl RawIacSpec {
    fn into_domain(self) -> Result<IacSpec, Diagnostic> {
        Ok(IacSpec {
            root: ProjectRelativePath::new(self.root)?,
            provider: parse_iac_provider(&self.provider)?,
            stacks: self.stacks,
        })
    }
}

impl RawDeploySpec {
    fn into_domain(self) -> Result<DeploySpec, Diagnostic> {
        Ok(DeploySpec {
            root: ProjectRelativePath::new(self.root)?,
            environments: self.environments,
        })
    }
}

impl RawProjectProtoSpec {
    fn into_domain(self) -> Result<ProjectProtoSpec, Diagnostic> {
        Ok(ProjectProtoSpec {
            owns: bounded_vec(self.owns, PATTERN_LIMIT, "manifest.proto.too_many")?
                .into_iter()
                .map(RepoGlob::new)
                .collect::<Result<Vec<_>, _>>()?,
            consumes: bounded_vec(self.consumes, PATTERN_LIMIT, "manifest.proto.too_many")?
                .into_iter()
                .map(RepoGlob::new)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl RawProjectAiSpec {
    fn into_domain(self) -> Result<ProjectAiSpec, Diagnostic> {
        Ok(ProjectAiSpec {
            editable: bounded_vec(self.editable, PATTERN_LIMIT, "manifest.ai.too_many")?
                .into_iter()
                .map(RepoGlob::new)
                .collect::<Result<Vec<_>, _>>()?,
            do_not_edit: bounded_vec(self.do_not_edit, PATTERN_LIMIT, "manifest.ai.too_many")?
                .into_iter()
                .map(RepoGlob::new)
                .collect::<Result<Vec<_>, _>>()?,
            docs: bounded_vec(self.docs, PATTERN_LIMIT, "manifest.ai.too_many")?
                .into_iter()
                .map(ProjectRelativePath::new)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTemplateManifest {
    schema: String,
    name: String,
    kind: String,
    engine: String,
    #[serde(default)]
    inputs: Vec<RawTemplateInput>,
    #[serde(default)]
    files: Vec<RawTemplateFile>,
    #[serde(default)]
    post_render: RawTemplatePostRender,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTemplateInput {
    name: String,
    #[serde(rename = "type")]
    input_type: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTemplateFile {
    source: String,
    target: String,
    #[serde(default = "default_template_mode")]
    mode: String,
    #[serde(default)]
    when: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct RawTemplatePostRender {
    #[serde(default)]
    validate: Vec<String>,
}

impl RawTemplateManifest {
    fn into_domain(self) -> Result<TemplateManifest, Diagnostic> {
        let schema = SchemaId::new(self.schema)?;
        if schema.as_str() != "repoctl.template/v1" {
            return Err(Diagnostic::error(
                "manifest.schema.unsupported",
                "template manifest schema must be repoctl.template/v1",
            ));
        }
        Ok(TemplateManifest {
            schema,
            name: self.name,
            kind: self.kind,
            engine: self.engine,
            inputs: bounded_vec(
                self.inputs,
                TEMPLATE_INPUT_LIMIT,
                "manifest.template.input_too_many",
            )?
            .into_iter()
            .map(RawTemplateInput::into_domain)
            .collect::<Result<Vec<_>, _>>()?,
            files: bounded_vec(
                self.files,
                TEMPLATE_FILE_LIMIT,
                "manifest.template.file_too_many",
            )?
            .into_iter()
            .map(RawTemplateFile::into_domain)
            .collect::<Result<Vec<_>, _>>()?,
            post_render_validate: self
                .post_render
                .validate
                .into_iter()
                .map(|command| {
                    let spec = CommandSpec::parse(&command)?;
                    if spec.program != "repoctl" {
                        return Err(Diagnostic::error(
                            "manifest.template.post_render",
                            "post-render validate commands must call repoctl",
                        ));
                    }
                    Ok(spec)
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl RawTemplateInput {
    fn into_domain(self) -> Result<TemplateInput, Diagnostic> {
        validate_template_identifier(&self.name)?;
        Ok(TemplateInput {
            name: self.name,
            input_type: self.input_type,
            required: self.required,
            default: self.default,
        })
    }
}

impl RawTemplateFile {
    fn into_domain(self) -> Result<TemplateFile, Diagnostic> {
        Ok(TemplateFile {
            source: ProjectRelativePath::new(self.source)?,
            target: self.target,
            mode: self.mode,
            when: self.when,
        })
    }
}

fn ensure_size(source: &ManifestSource) -> Result<(), RepoctlError> {
    if source.bytes.len() > MANIFEST_MAX_BYTES {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "manifest.too_large",
                format!("manifest exceeds {MANIFEST_MAX_BYTES} byte limit"),
            )
            .with_path(source.path.clone()),
        ));
    }
    Ok(())
}

fn parse_yaml<T>(source: &ManifestSource) -> Result<T, RepoctlError>
where
    T: for<'de> Deserialize<'de>,
{
    let text = std::str::from_utf8(&source.bytes).map_err(|error| {
        RepoctlError::diagnostic(
            Diagnostic::error("manifest.utf8", format!("manifest is not UTF-8: {error}"))
                .with_path(source.path.clone()),
        )
    })?;
    serde_norway::from_str(text).map_err(|error| {
        RepoctlError::diagnostic(
            Diagnostic::error("manifest.yaml", format!("failed to parse YAML: {error}"))
                .with_path(source.path.clone()),
        )
    })
}

fn parse_repo_layout(value: &str) -> Result<RepoLayout, Diagnostic> {
    match value {
        "functional" => Ok(RepoLayout::Functional),
        _ => Err(Diagnostic::error(
            "manifest.repo.layout",
            "layout must be functional for v0.2",
        )),
    }
}

fn parse_policy_mode(value: &str) -> Result<PolicyMode, Diagnostic> {
    match value {
        "deny" => Ok(PolicyMode::Deny),
        "warn" => Ok(PolicyMode::Warn),
        "allow" => Ok(PolicyMode::Allow),
        _ => Err(Diagnostic::error(
            "manifest.policy.mode",
            format!("unsupported policy mode `{value}`"),
        )),
    }
}

fn parse_generated_policy(value: &str) -> Result<GeneratedCodePolicy, Diagnostic> {
    match value {
        "consumer-local" => Ok(GeneratedCodePolicy::ConsumerLocal),
        _ => Err(Diagnostic::error(
            "manifest.repo.generated_policy",
            "generated_code_policy must be consumer-local for v0.2",
        )),
    }
}

fn parse_project_kind(value: &str) -> Result<ProjectKind, Diagnostic> {
    match value {
        "app" => Ok(ProjectKind::App),
        "framework" => Ok(ProjectKind::Framework),
        "foundation-service" => Ok(ProjectKind::FoundationService),
        "proto-root" => Ok(ProjectKind::ProtoRoot),
        "core-infra" => Ok(ProjectKind::CoreInfra),
        _ => Err(Diagnostic::error(
            "manifest.project.kind",
            format!("unsupported project kind `{value}`"),
        )),
    }
}

fn parse_visibility(value: &str) -> Result<Visibility, Diagnostic> {
    match value {
        "internal" => Ok(Visibility::Internal),
        "public" => Ok(Visibility::Public),
        _ => Err(Diagnostic::error(
            "manifest.project.visibility",
            format!("unsupported visibility `{value}`"),
        )),
    }
}

fn parse_workspace_language(value: &str) -> Result<WorkspaceLanguage, Diagnostic> {
    match value {
        "rust" => Ok(WorkspaceLanguage::Rust),
        "typescript" => Ok(WorkspaceLanguage::TypeScript),
        "python" => Ok(WorkspaceLanguage::Python),
        "proto" => Ok(WorkspaceLanguage::Proto),
        "iac" => Ok(WorkspaceLanguage::Iac),
        _ => Err(Diagnostic::error(
            "manifest.workspace.language",
            format!("unsupported workspace language `{value}`"),
        )),
    }
}

fn parse_iac_provider(value: &str) -> Result<IacProvider, Diagnostic> {
    match value {
        "pulumi" => Ok(IacProvider::Pulumi),
        "terraform" => Ok(IacProvider::Terraform),
        "opentofu" => Ok(IacProvider::OpenTofu),
        _ => Err(Diagnostic::error(
            "manifest.iac.provider",
            format!("unsupported IaC provider `{value}`"),
        )),
    }
}

fn repo_path_from_template(value: &str) -> Result<RepoRelativePath, Diagnostic> {
    let stripped = value
        .strip_prefix("{repo_root}/")
        .map_or(value, |path| path);
    RepoRelativePath::new(stripped.to_string())
}

fn parse_area_map(
    raw: BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<ProjectRelativePath>>, Diagnostic> {
    raw.into_iter()
        .map(|(language, paths)| {
            let paths = bounded_vec(paths, PATTERN_LIMIT, "manifest.area.too_many")?
                .into_iter()
                .map(ProjectRelativePath::new)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((language, paths))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
}

fn validate_unique_workspace_names(workspaces: &[WorkspaceSpec]) -> Result<(), Diagnostic> {
    let mut seen = BTreeSet::new();
    for workspace in workspaces {
        if !seen.insert(workspace.name.clone()) {
            return Err(Diagnostic::error(
                "manifest.workspace.duplicate_name",
                format!("duplicate workspace name `{}`", workspace.name),
            ));
        }
    }
    Ok(())
}

fn validate_template_identifier(value: &str) -> Result<(), Diagnostic> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(Diagnostic::error(
            "manifest.template.input_name",
            "template input names must use lowercase snake_case",
        ));
    }
    Ok(())
}

fn bounded_vec<T>(values: Vec<T>, limit: usize, code: &str) -> Result<Vec<T>, Diagnostic> {
    if values.len() > limit {
        Err(Diagnostic::error(
            code,
            format!("collection exceeds limit of {limit}"),
        ))
    } else {
        Ok(values)
    }
}

fn bounded_map<K, V>(
    values: BTreeMap<K, V>,
    limit: usize,
    code: &str,
) -> Result<BTreeMap<K, V>, Diagnostic> {
    if values.len() > limit {
        Err(Diagnostic::error(
            code,
            format!("map exceeds limit of {limit}"),
        ))
    } else {
        Ok(values)
    }
}

fn default_protos_root() -> String {
    "protos".to_string()
}

fn default_generated_policy() -> String {
    "consumer-local".to_string()
}

fn default_core_infra_root() -> String {
    "core-infra".to_string()
}

fn default_agent_skills_root() -> String {
    ".agents/skills".to_string()
}

fn default_claude_skills_root() -> String {
    ".claude/skills".to_string()
}

fn default_context_output() -> String {
    "target/repoctl/context-packs".to_string()
}

fn default_template_mode() -> String {
    "managed".to_string()
}

#[cfg(test)]
mod tests {
    use super::YamlManifestParser;
    use crate::domain::{DependencyTarget, ProjectKind};

    const REPO: &str = r#"
schema: company.repo/v1
name: acme
layout: functional
defaults:
  owner: "@platform"
protos:
  root: protos
  generated_code_policy: consumer-local
"#;

    const PROJECT: &str = r#"
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
depends_on:
  - frameworks.service-runtime
  - foundations.identity.client
tasks:
  check:
    - workspace: api
      command: cargo check --workspace
"#;

    const TEMPLATE: &str = r#"
schema: repoctl.template/v1
name: app-fullstack
kind: app
engine: minijinja
inputs:
  - name: app_name
    type: string
    required: true
files:
  - source: files/project.yaml.j2
    target: "{{ app.path }}/project.yaml"
post_render:
  validate:
    - repoctl graph validate
"#;

    #[test]
    fn test_should_parse_valid_repo_manifest() {
        let manifest = YamlManifestParser
            .parse_repo_bytes("repo.yaml", REPO.as_bytes())
            .expect("repo parses");
        assert_eq!(manifest.name.as_str(), "acme");
    }

    #[test]
    fn test_should_parse_valid_project_manifest() {
        let manifest = YamlManifestParser
            .parse_project_bytes("apps/catalog/project.yaml", PROJECT.as_bytes())
            .expect("project parses");
        assert_eq!(manifest.kind, ProjectKind::App);
        assert_eq!(manifest.tasks.len(), 1);
        match &manifest.depends_on[1].target {
            DependencyTarget::Project(name) => assert_eq!(name.as_str(), "foundations.identity"),
            DependencyTarget::ProtoPackage(_) => panic!("expected project dependency"),
        }
    }

    #[test]
    fn test_should_reject_unknown_project_schema() {
        let yaml = PROJECT.replace("company.project/v1", "company.project/v9");
        let error = YamlManifestParser
            .parse_project_bytes("apps/catalog/project.yaml", yaml.as_bytes())
            .expect_err("schema is rejected");
        assert_eq!(
            error.diagnostics()[0].code.as_ref(),
            "manifest.schema.unsupported"
        );
    }

    #[test]
    fn test_should_parse_valid_template_manifest() {
        let manifest = YamlManifestParser
            .parse_template_bytes("templates/app/template.yaml", TEMPLATE.as_bytes())
            .expect("template parses");
        assert_eq!(manifest.files.len(), 1);
    }
}
