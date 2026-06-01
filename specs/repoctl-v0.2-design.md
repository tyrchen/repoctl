# repoctl v0.2 Design Spec

Status: Draft

Source PRD: [01-initial-spec.md](01-initial-spec.md)

## 1. Design Thesis

`repoctl` is a monorepo control plane. Its central abstraction is not a language workspace, package manager, CI workflow, template, or deploy target. Its central abstraction is a validated project graph that humans, CI, and AI agents can share.

The v0.2 design follows five rules:

1. Top-level repo layout is functional: `apps`, `frameworks`, `foundations`, `core-infra`, `protos`, `templates`, and `tools`.
2. Language workspaces belong to functional projects, never to the generated repo root.
3. The CLI is intentionally thin. It parses flags, calls a facade crate, renders typed results, and maps diagnostics to exit codes.
4. Core behavior lives behind stable traits in a small number of capability crates.
5. All repo operations are plan-first: discover, validate, compute impact, then execute or render.

The current `repoctl` source repository may keep its existing Rust workspace while the tool is being built. That is a bootstrap detail for this tool's own codebase. A repo initialized by `repoctl init` must not get a root `Cargo.toml`, root `package.json`, root `pnpm-workspace.yaml`, or root `pyproject.toml`.

## 2. Scope

In scope for v0.2:

- `repoctl init` for functional monorepo layout.
- `repoctl new app`, `new framework`, and `new foundation`.
- `repo.yaml`, `project.yaml`, `protos/project.yaml`, and `template.yaml` parsing and validation.
- Project, workspace, proto, IaC, facade, and reverse dependency graph construction.
- Boundary linting and generated-code direct-edit detection.
- Affected analysis and GitHub Actions matrix generation.
- Generic task runner for repo-defined `check`, `test`, `build`, and codegen tasks.
- AI context, PR summary, and skill synchronization.
- Proto ownership and consumer queries.
- IaC plan routing and risk flags, without apply.

Out of scope for v0.2:

- Docker-specific build orchestration.
- Automatic IaC apply.
- Arbitrary template hooks or shell scripts from templates.
- Dynamic service runtime, deployment engine, or language-specific package manager replacement.
- Full remote Git template support unless the trust model is implemented with pinned refs and checksums.

## 3. System Architecture

The CLI app is a shell around a facade crate. The CLI must not parse manifests, walk repos, build graphs, evaluate policies, compute affected projects, render templates, or spawn task commands directly. It delegates those operations to the facade.

```text
+--------------------------+
| repoctl CLI app          |
| apps/repoctl-cli         |
|                          |
| - clap args              |
| - terminal/json output   |
| - exit codes             |
| - tracing setup          |
+-------------+------------+
              |
              | calls typed facade API
              v
+-------------+------------+
| repoctl facade crate     |
| crates/repoctl           |
|                          |
| - public API             |
| - request/response DTOs  |
| - default service wiring |
| - re-exported domain     |
+-------------+------------+
              |
              | orchestrates capability services
              v
+-------------+------------------------------------------------+
| capability crates                                            |
|                                                              |
| repoctl-engine     repoctl-scaffold     repoctl-runner       |
|                                                              |
| engine: discovery, graph, policy, affected, ci, pr, ai       |
| scaffold: init, new, template, skills                        |
| runner: task execution, proto checks, iac plan, codegen      |
+-------------+------------------------------------------------+
              |
              | uses domain model and adapter traits
              v
+-------------+------------------------------------------------+
| domain crates                                                |
|                                                              |
| repoctl-core       repoctl-manifest   repoctl-workspace      |
| repoctl-graph-model repoctl-diagnostics repoctl-policy-model |
+-------------+------------------------------------------------+
              |
              | implemented by infrastructure adapters
              v
+-------------+------------------------------------------------+
| adapter crates                                               |
|                                                              |
| fs/git/process/cargo/bun/uv/buf/iac/minijinja/schema         |
+--------------------------------------------------------------+
```

The dependency direction is one-way:

```text
cli -> facade -> capability crates -> domain traits -> adapters
```

The CLI can depend on `clap`, terminal formatting, tracing initialization, and the facade crate. It should not depend on `repoctl-engine`, `repoctl-scaffold`, `repoctl-runner`, domain crates, or concrete adapters directly. This keeps the CLI replaceable by other frontends, such as a GitHub Action, an editor extension, or a future daemon.

## 4. Crate Boundaries

Target crate layout:

```text
apps/
  repoctl-cli/
    Cargo.toml
    src/main.rs

crates/
  repoctl/                 # facade crate consumed by CLI and other frontends
  repoctl-core/            # domain primitives and common request/response types
  repoctl-diagnostics/     # diagnostic model and reporters
  repoctl-manifest/        # repo/project/template/proto manifest domain
  repoctl-workspace/       # workspace specs, selectors, task specs
  repoctl-graph-model/     # typed graph nodes, edges, snapshots
  repoctl-policy-model/    # policy declarations and rule data
  repoctl-engine/          # read/analysis services: discovery, graph, policy, affected, ci, pr, ai
  repoctl-scaffold/        # file-generation services: init, new, templates, skills
  repoctl-runner/          # execution services: tasks, proto checks, iac plan, codegen checks
  repoctl-adapters/        # concrete fs/git/process/cargo/bun/uv/buf/iac/template/schema adapters
```

The first implementation should avoid one crate per command. Use internal modules inside the capability crates and split only when a boundary becomes independently reusable, release-worthy, or dependency-heavy. The facade boundary must be present early. The CLI should consume only `crates/repoctl` plus CLI-only dependencies.

## 5. Facade API

The facade crate is the public API of the system. It exposes typed requests and reports. It hides adapter wiring and internal crate topology.

Representative facade:

```rust
pub struct Repoctl {
    services: RepoctlServices,
}

impl Repoctl {
    pub fn with_default_adapters() -> Result<Self, RepoctlError>;

    pub fn init(&self, request: InitRequest) -> Result<InitPlan, RepoctlError>;

    pub fn new_project(&self, request: NewProjectRequest) -> Result<RenderPlan, RepoctlError>;

    pub fn discover(&self, request: DiscoverRequest) -> Result<RepoSnapshot, RepoctlError>;

    pub fn validate_graph(
        &self,
        request: GraphValidateRequest,
    ) -> Result<ValidationReport, RepoctlError>;

    pub fn affected(&self, request: AffectedRequest) -> Result<AffectedReport, RepoctlError>;

    pub fn run_task(&self, request: TaskRunRequest) -> Result<TaskRunReport, RepoctlError>;

    pub fn ci_matrix(&self, request: CiMatrixRequest) -> Result<CiMatrixReport, RepoctlError>;

    pub fn pr_summary(&self, request: PrSummaryRequest) -> Result<PrSummary, RepoctlError>;

    pub fn ai_context(&self, request: AiContextRequest) -> Result<AiContext, RepoctlError>;

    pub fn proto(&self) -> &ProtoFacade;

    pub fn iac(&self) -> &IacFacade;

    pub fn skills(&self) -> &SkillsFacade;
}
```

The facade must return domain results, not preformatted strings. Human Markdown, terminal tables, JSON, and GitHub Actions output belong to output renderers in the CLI or a small presentation crate.

## 6. Core Data Structures

All external input crosses a validation boundary before entering the domain:

- YAML manifests are parsed into raw serde structs.
- Raw structs are converted into validated domain structs using `TryFrom`.
- Domain structs use private fields and fallible constructors for names, paths, owners, task identifiers, schema identifiers, and glob patterns.
- Parsed values carry source file context, and spans where available, for diagnostics.

Core domain types:

The snippets below describe semantic shape. Validated domain structs should keep fields private when public mutation could break invariants, and expose accessors or builders instead.

```rust
pub struct RepoRoot {
    absolute: Utf8PathBuf,
}

pub struct RepoSnapshot {
    pub root: RepoRoot,
    pub repo_manifest: RepoManifest,
    pub projects: ProjectIndex,
    pub graph: RepoGraph,
    pub generated_policy: GeneratedCodePolicy,
    pub discovered_at: SystemTime,
}

pub struct ProjectManifest {
    pub schema: SchemaId,
    pub name: ProjectId,
    pub kind: ProjectKind,
    pub path: RepoRelativePath,
    pub owners: OwnerSet,
    pub visibility: Visibility,
    pub workspaces: WorkspaceSet,
    pub depends_on: ProjectDependencySet,
    pub tasks: TaskSet,
    pub iac: Option<IacSpec>,
    pub deploy: Option<DeploySpec>,
    pub protos: Option<ProjectProtoSpec>,
    pub ai: Option<ProjectAiSpec>,
    pub policies: ProjectPolicySet,
}

pub enum ProjectKind {
    App,
    Framework,
    FoundationService,
    ProtoRoot,
    CoreInfra,
}

pub struct WorkspaceSpec {
    pub id: WorkspaceId,
    pub language: WorkspaceLanguage,
    pub toolchain: Toolchain,
    pub root: ProjectRelativePath,
    pub manifest: ProjectRelativePath,
    pub lockfile: Option<ProjectRelativePath>,
    pub target_dir: Option<RepoRelativePath>,
    pub cache_dir: Option<RepoRelativePath>,
}

pub struct RepoGraph {
    pub nodes: GraphNodeSet,
    pub edges: GraphEdgeSet,
}

pub enum GraphNode {
    Project(ProjectId),
    Workspace { project: ProjectId, workspace: WorkspaceId },
    ProtoPackage(ProtoPackageId),
    IacTarget(IacTargetId),
    Template(TemplateId),
}

pub enum EdgeKind {
    DependsOnProject,
    ContainsWorkspace,
    ConsumesProto,
    OwnsProto,
    UsesFrameworkFacade,
    UsesFrameworkInternal,
    UsesFoundationClient,
    UsesFoundationInternal,
    UsesCoreInfraModule,
    OwnsIac,
    RunsTask,
}
```

Request and response types:

```rust
pub struct InitRequest {
    pub repo_root: RepoRoot,
    pub name: RepoName,
    pub profile: InitProfile,
    pub layout: RepoLayout,
    pub languages: LanguageSelection,
    pub iac: Option<IacProvider>,
    pub ci: Option<CiProvider>,
    pub protos: Option<ProtoToolchain>,
    pub dry_run: bool,
}

pub struct InitPlan {
    pub operations: Vec<FileOperation>,
    pub warnings: Vec<Diagnostic>,
    pub next_steps: Vec<NextStep>,
}

pub struct AffectedRequest {
    pub repo_root: RepoRoot,
    pub base: GitRef,
    pub head: GitRef,
    pub tasks: TaskSelection,
}

pub struct AffectedReport {
    pub changed_files: Vec<ChangedFile>,
    pub directly_affected: ProjectSet,
    pub transitively_affected: ProjectSet,
    pub workspaces: WorkspaceSelection,
    pub tasks: TaskSelection,
    pub risk_flags: Vec<RiskFlag>,
    pub reasons: ReasonTree,
    pub suggested_reviewers: OwnerSet,
}

pub struct TaskRunPlan {
    pub jobs: Vec<TaskJob>,
    pub concurrency: NonZeroUsize,
}

pub struct TaskJob {
    pub project: ProjectId,
    pub workspace: WorkspaceId,
    pub task: TaskName,
    pub cwd: RepoRelativePath,
    pub command: CommandSpec,
    pub env: EnvOverlay,
}

pub struct CommandSpec {
    pub program: ToolName,
    pub args: Vec<CommandArg>,
}
```

Important invariants:

- `ProjectId` is globally unique.
- `ProjectManifest.path` is repo-relative, normalized, inside the repo, and matches the file location.
- `WorkspaceSpec.root`, `manifest`, and `lockfile` are project-relative.
- `depends_on` only references declared or discoverable projects.
- `public_facades` and `internal` areas are disjoint.
- Generated-code glob patterns are compiled during validation.
- No domain path stores `..`, absolute paths, NUL bytes, or platform separators that escape the repo root.
- `CommandSpec` is argv-shaped; command strings from PRD-compatible manifests are normalized before use.

## 7. Trait Design

Traits separate domain behavior from concrete infrastructure. They are not added for ceremony; each trait marks a boundary where tests need an in-memory implementation or future functionality needs another provider.

Repository and manifest traits:

```rust
pub trait RepoLocator {
    fn locate(&self, start: &Utf8Path) -> Result<RepoRoot, RepoctlError>;
}

pub trait RepoFileSystem {
    fn read_file(&self, path: &RepoRelativePath) -> Result<FileBytes, RepoctlError>;

    fn write_plan(&self, operations: &[FileOperation]) -> Result<WriteReport, RepoctlError>;

    fn walk(&self, request: WalkRequest) -> Result<Vec<RepoRelativePath>, RepoctlError>;
}

pub trait ManifestParser {
    fn parse_repo(&self, source: ManifestSource) -> Result<RepoManifest, RepoctlError>;

    fn parse_project(&self, source: ManifestSource) -> Result<ProjectManifest, RepoctlError>;

    fn parse_template(&self, source: ManifestSource) -> Result<TemplateManifest, RepoctlError>;
}

pub trait SchemaValidator {
    fn validate(&self, schema: SchemaId, value: JsonValue) -> Result<(), RepoctlError>;
}
```

Graph and policy traits:

```rust
pub trait ProjectDiscoverer {
    fn discover(&self, root: &RepoRoot) -> Result<ProjectManifests, RepoctlError>;
}

pub trait GraphBuilder {
    fn build(&self, input: GraphBuildInput) -> Result<RepoGraph, RepoctlError>;
}

pub trait BoundaryInspector {
    fn inspect(&self, snapshot: &RepoSnapshot) -> Result<Vec<BoundaryFinding>, RepoctlError>;
}

pub trait PolicyRule: Send + Sync {
    fn name(&self) -> PolicyRuleName;

    fn evaluate(&self, context: PolicyContext<'_>) -> Result<Vec<PolicyFinding>, RepoctlError>;
}
```

Toolchain and execution traits:

```rust
pub trait WorkspaceInspector: Send + Sync {
    fn language(&self) -> WorkspaceLanguage;

    fn inspect(
        &self,
        snapshot: &RepoSnapshot,
        workspace: &WorkspaceSpec,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError>;
}

pub trait TaskPlanner {
    fn plan(&self, request: TaskPlanRequest) -> Result<TaskRunPlan, RepoctlError>;
}

pub trait ProcessRunner: Send + Sync {
    fn run(&self, command: ProcessCommand) -> Result<ProcessOutput, RepoctlError>;
}

pub trait ToolchainAdapter: Send + Sync {
    fn toolchain(&self) -> Toolchain;

    fn environment(
        &self,
        snapshot: &RepoSnapshot,
        workspace: &WorkspaceSpec,
    ) -> Result<EnvOverlay, RepoctlError>;
}
```

Template, Git, proto, and IaC traits:

```rust
pub trait TemplateSourceResolver {
    fn resolve(&self, source: TemplateSource) -> Result<ResolvedTemplateSource, RepoctlError>;
}

pub trait TemplateEngine {
    fn render(&self, request: RenderRequest) -> Result<RenderedTemplate, RepoctlError>;
}

pub trait GitProvider {
    fn changed_files(&self, base: &GitRef, head: &GitRef) -> Result<Vec<ChangedFile>, RepoctlError>;
}

pub trait ProtoToolchainAdapter {
    fn check(&self, request: ProtoCheckRequest) -> Result<ProtoCheckReport, RepoctlError>;
}

pub trait IacProviderAdapter {
    fn provider(&self) -> IacProvider;

    fn plan(&self, request: IacPlanRequest) -> Result<IacPlanReport, RepoctlError>;
}
```

Trait usage rules:

- Use traits at adapter and capability-service boundaries.
- Prefer concrete domain types inside the domain model.
- Keep traits small and focused; avoid a single "repository service" trait that knows every command.
- Do not use `async_trait` unless a trait must be object-safe and async. Prefer synchronous traits for parsing, graph building, policy, and planning. Use async only for process orchestration if it materially simplifies bounded concurrent execution.
- CLI output rendering should be replaceable but does not belong in the domain layer.

## 8. Data Flow

Discovery and validation:

```text
CLI args
  |
  v
Repoctl facade
  |
  v
RepoLocator
  |
  v
RepoFileSystem.walk
  |
  v
ManifestParser + SchemaValidator
  |
  v
Validated manifests
  |
  v
WorkspaceInspectors
  |
  v
GraphBuilder
  |
  v
PolicyRule set
  |
  v
RepoSnapshot + ValidationReport
  |
  v
CLI renderer
```

Affected analysis:

```text
base/head refs
  |
  v
GitProvider.changed_files
  |
  v
path owner matching
  |
  v
direct affected nodes
  |
  v
RepoGraph reverse traversal
  |
  v
task/workspace selection
  |
  v
risk classifiers
  |
  v
AffectedReport
  |
  +--> repoctl ci matrix
  |
  +--> repoctl pr summary
  |
  +--> repoctl run --affected
```

Template and file-generation flow:

```text
init/new request
  |
  v
TemplateSourceResolver
  |
  v
TemplateManifest validation
  |
  v
TemplateEngine.render
  |
  v
FileOperation plan
  |
  v
conflict/path validation
  |
  +-- dry-run --> RenderPlan
  |
  v
RepoFileSystem.write_plan
  |
  v
post-render graph validation
```

Task execution:

```text
run request
  |
  v
RepoSnapshot
  |
  v
TaskPlanner
  |
  v
TaskRunPlan
  |
  v
ToolchainAdapter.env
  |
  v
ProcessRunner.run with bounded concurrency
  |
  v
TaskRunReport
```

## 9. Manifest Design

`repo.yaml` is repo-level policy and defaults. It controls layout, default owners, language defaults, proto configuration, CI behavior, IaC policy, template sources, AI context paths, and global policy modes.

`project.yaml` is the source of truth for one functional project. It declares kind, owners, workspaces, dependencies, tasks, IaC, deploy roots, proto ownership/consumption, AI editable areas, and local policies.

`protos/project.yaml` is the source of truth for proto source ownership and consumers.

`template.yaml` describes a template package. It declares inputs, file mappings, render modes, conditions, and post-render repoctl validations.

The implementation should generate JSON Schema from Rust types with `schemars` and validate user-provided manifests with `jsonschema`. Runtime domain validation still remains mandatory because schema checks cannot prove path containment, graph references, or policy consistency.

YAML parser choice: use `serde_norway` as the initial parser because `serde_yaml` is deprecated and `serde_yml` has a RustSec advisory. The YAML adapter must be isolated so a later parser change does not affect the domain model.

## 10. Discovery Pipeline

Discovery is deterministic and bounded:

1. Resolve repo root from `--repo`, nearest `repo.yaml`, or Git root.
2. Load and validate `repo.yaml`.
3. Walk discoverable roots with `.gitignore` support and explicit caps.
4. Parse every `project.yaml` and `protos/project.yaml`.
5. Normalize paths and apply repo defaults.
6. Validate manifest references.
7. Inspect language/IaC/proto workspaces through adapter traits.
8. Build graph edges.
9. Run policy checks.
10. Return a `RepoSnapshot`.

`target/**`, VCS internals, generated caches, and ignored paths are excluded by default. The walker must never follow paths outside the repo root.

## 11. Graph Design

The graph has project nodes, workspace nodes, proto package nodes, IaC target nodes, and template nodes.

Graph construction combines declared manifest edges and adapter-discovered edges. Declared edges are authoritative for cross-language relationships. Adapter-discovered edges enrich validation, especially for Rust path dependencies, Bun workspace dependencies, uv sources, and IaC module references.

Boundary violations are graph diagnostics:

- `app -> app` is denied.
- `app -> framework internal` is denied.
- `app -> foundation internals` is denied.
- `framework -> app` is denied.
- `foundation -> app` is denied.
- `generated/**` direct edits are denied.
- `core-infra/**` and prod IaC changes receive high-risk flags.

## 12. Affected Analysis

Affected analysis takes:

```text
base commit
head commit
changed files
RepoSnapshot
policy rules
task names
```

It returns:

```text
directly affected projects
transitively affected projects
affected workspaces
affected tasks
risk flags
reason tree
suggested reviewers
```

Path-to-owner mapping is evaluated first. Then graph propagation applies:

- Project file changes affect the project.
- Framework facade changes affect reverse consumers.
- Proto source changes affect owner, generated clients, and consumers.
- `core-infra/**` affects core IaC targets and high-risk review.
- App IaC affects only that app's IaC targets.
- Foundation IaC affects only that foundation's IaC targets.
- `repo.yaml`, templates, skills, and repoctl tooling affect repo-wide validation and relevant hygiene tasks.

Every affected result must include a reason tree so PR summaries and CI matrices are explainable.

## 13. Task Runner

The task runner is generic. It does not understand every language. It:

- Resolves selected projects and workspaces.
- Reads task definitions from `project.yaml`.
- Orders work by dependency graph.
- Sets `cwd` to the workspace root.
- Injects toolchain environment such as `CARGO_TARGET_DIR` or `UV_CACHE_DIR`.
- Spawns child processes with argv form.
- Streams or captures output based on caller options.
- Returns a structured summary.

The PRD shows task commands as strings for readability. The implementation should parse those strings into argv and reject shell-only constructs for v0.2. Do not invoke `sh -c` for repo task execution.

Concurrency uses bounded workers and structured join handling. Task panics or join errors become diagnostics with project, workspace, task, and command context.

## 14. Template System

Template operations are plan-first:

1. Resolve template source.
2. Load `template.yaml`.
3. Validate inputs.
4. Build MiniJinja context.
5. Render into memory.
6. Validate target paths.
7. Detect conflicts.
8. Apply file operations.
9. Run post-render repoctl validations.

Render modes:

- `managed`: repoctl owns the whole generated file.
- `block`: repoctl updates marked blocks and preserves user content outside them.
- `copy`: byte-for-byte copy with no template rendering.

v0.2 supports builtin and local templates. External Git templates are deferred until the trust model includes pinned refs, checksums, source allowlists, and no arbitrary hook execution.

## 15. Init And New Commands

`repoctl init` creates the functional skeleton:

```text
repo.yaml
AGENTS.md
.repoctl/
.agents/skills/
.claude/skills/
.github/
protos/
apps/
foundations/
frameworks/
core-infra/
templates/
tools/
docs/
target/
```

It must not create root language workspace files.

`repoctl new app` creates a local project boundary with `project.yaml`, `AGENTS.md`, README, workspaces, local IaC, deploy intent, docs, tests, and initial tasks.

`repoctl new framework` creates a capability project with facade and internal areas. Apps may depend on facade areas only.

`repoctl new foundation` creates a company-level service with service workspace, public clients, proto ownership, IaC, deploy, docs, and runbooks.

All create commands support `--dry-run`, conflict reporting, and idempotent managed-block updates.

## 16. Proto System

Source proto files live only under `protos/`. Ownership and consumers are declared in `protos/project.yaml` and project manifests.

The proto module provides:

- Owner lookup for a proto path.
- Consumer lookup for a proto path or package.
- Compatibility checks through the configured buf toolchain.
- Generated-code policy checks.
- Consumer-local output routing.

Generated code is allowed under consumer-local generated directories but must not be directly edited. Direct edits are detected from changed files and generated-code glob policies.

## 17. IaC System

IaC design is hybrid:

- Shared baseline and modules live in `core-infra/`.
- App-specific IaC lives under `apps/<app>/iac/`.
- Foundation-specific IaC lives under `foundations/<service>/iac/`.

The IaC module routes plan commands to the owning project and provider. v0.2 supports plan only. It never applies infrastructure changes.

Risk classification:

- `core-infra/**`: high risk, platform and security review.
- `apps/*/iac/stacks/prod/**`: high risk, app owner and platform review.
- `foundations/*/iac/stacks/prod/**`: high risk, foundation owner and platform review.

## 18. AI Context And Skills

AI context is generated from the same `RepoSnapshot` used by CI:

- project kind and owners
- workspaces and commands
- allowed edit paths
- do-not-edit paths
- dependencies
- proto ownership and consumers
- IaC boundaries
- risk flags

Skill sync maintains `.agents/skills` and `.claude/skills` from repo policy. Managed blocks allow repoctl to upgrade generated content while preserving user-authored sections.

## 19. CLI Surface

v0.2 commands:

```text
repoctl init
repoctl new app
repoctl new framework
repoctl new foundation
repoctl graph validate
repoctl graph print
repoctl explain
repoctl affected
repoctl run
repoctl pr summary
repoctl context
repoctl lint-boundaries
repoctl codegen check
repoctl proto check
repoctl proto owners
repoctl proto consumers
repoctl iac plan
repoctl skills sync
repoctl skills check
repoctl template list
repoctl template render
repoctl ci matrix
repoctl ci summarize
```

All commands should support machine-readable output where it matters:

- `--format human`
- `--format json`
- `--format github-actions` for CI matrix commands

CLI command handlers should look like:

```text
parse args -> build request -> call facade -> render response -> choose exit code
```

They should not contain repo discovery, graph traversal, policy decisions, template rendering, or task execution logic.

## 20. Diagnostics

Diagnostics must be stable and actionable:

```text
code
severity
message
file path
span when available
project/workspace context when available
help text
```

Exit code policy:

- `0`: success.
- `1`: validation, policy, or task failure.
- `2`: invalid CLI usage.
- `3`: environment/toolchain unavailable.
- `4`: internal bug surfaced as a controlled diagnostic.

## 21. Security Design

Security controls:

- Reject path traversal and absolute output paths in manifests and templates.
- Enforce byte-length caps for strings and element-count caps for collections.
- Compile and validate glob patterns up front.
- Use argv process spawning, not shell concatenation.
- No arbitrary template hooks in v0.2.
- No automatic IaC apply.
- No direct edits to generated code.
- Avoid logging secrets and environment dumps.
- Treat Git template support as untrusted remote input and keep it out of v0.2 unless the full trust model lands.

## 22. Dependency Decisions

Dependency research is recorded in [repoctl v0.2 dependency research](../docs/research/repoctl-v0.2-dependency-research.md).

Initial dependency direction:

- CLI: `clap`.
- YAML: `serde_norway` behind an adapter.
- JSON and schema: `serde`, `serde_json`, `schemars`, `jsonschema`.
- Paths: `camino`.
- Traversal and globs: `ignore`, `globset`.
- Graphs: `petgraph` for cross-language repo graph, `cargo_metadata` and possibly `guppy` inside Rust workspace adapters.
- Templates: `minijinja`.
- Validation: domain newtypes first, `validator` where serde-bound validation is clearer.
- Errors: `thiserror` in libraries, `anyhow` in CLI/application boundary.
- Async/process orchestration: `tokio` with explicit features.

## 23. PRD Coverage Matrix

| PRD Area | Design Coverage |
| --- | --- |
| Product positioning as monorepo control plane | Sections 1, 3, 5 |
| v0.2 functional top-level layout | Sections 1, 15 |
| Apps as complete local engineering units | Sections 6, 9, 15, 18 |
| One-command app creation | Sections 14, 15 |
| Framework extraction path and facade-only use | Sections 6, 11, 15 |
| Repo quality pressure through graph and policy | Sections 7, 10, 11, 21 |
| Top-level directory structure | Sections 1, 15 |
| `apps/`, `frameworks/`, `foundations/`, `core-infra/`, `protos/` semantics | Sections 9, 11, 15, 16, 17 |
| No root language workspace in generated repos | Sections 1, 15 |
| Rust/Bun/uv workspace strategy | Sections 6, 7, 13 |
| Root target/cache strategy | Sections 6, 13 |
| `repo.yaml` | Sections 6, 9, 10 |
| `project.yaml` variants | Sections 6, 9, 15, 16, 17 |
| Dependency boundary rules | Sections 7, 11, 21 |
| Hybrid core infra and app/foundation IaC | Sections 12, 17 |
| `repoctl init` | Sections 14, 15 |
| `.agents/skills` and `.claude/skills` | Sections 15, 18 |
| Template system and MiniJinja | Sections 7, 8, 14 |
| CLI command surface | Sections 5, 19 |
| CI design | Sections 5, 8, 12, 19 |
| Affected analysis | Sections 8, 12 |
| Proto ownership and generated-code policy | Sections 11, 12, 16, 21 |
| AI context | Sections 8, 18 |
| PR summary | Sections 5, 8, 12, 18 |
| Rust crate structure | Sections 3, 4, 5 |
| Policy design | Sections 7, 11, 21 |
| MVP milestones and daily workflows | Covered by implementation and verification plans |

## 24. Open Design Decisions

- Whether task command schema should evolve from `command: "cargo check --workspace"` to `argv: ["cargo", "check", "--workspace"]`. The implementation should accept the PRD string form but normalize to argv internally.
- Whether to split all target crates immediately or keep modules inside fewer crates until graph and manifest APIs stabilize. The facade crate and CLI boundary should not be deferred.
- Whether generated schemas are committed under `.repoctl/schemas/` or generated on demand during `repoctl init`.
- Whether external Git templates are shipped in a v0.2 minor release or held for v0.3 after threat modeling.
