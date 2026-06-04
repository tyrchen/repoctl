# repoctl v0.3 Adoption And Monorepo Hardening Spec

Status: Draft

Source learning: migration of existing Cellis standalone repositories into a
functional `universe` monorepo.

Related specs:

- [repoctl v0.2 Design Spec](repoctl-v0.2-design.md)
- [repoctl v0.2 Implementation Plan](repoctl-v0.2-impl-plan.md)
- [repoctl v0.2 Verification Plan](repoctl-v0.2-verification-plan.md)

## 1. Problem Statement

`repoctl v0.2` is strong at initializing a clean functional monorepo and
scaffolding new projects. Real teams also need to adopt existing standalone
repositories into that monorepo without losing intent:

- classify each source repository by purpose before copying it;
- choose the correct monorepo lane;
- strip VCS metadata and build artifacts;
- generate accurate `project.yaml` manifests from existing code;
- convert internal package references from registry/git dependencies to
  monorepo-local dependencies;
- preserve buildability across Rust, Node, frontend, SDK, and IaC packages;
- produce CI that actually executes the affected matrix, not only computes it;
- avoid fragile ordering bugs when local TypeScript packages must be built before
  consumers type-check;
- make validation useful even when package registries or language metadata are
  temporarily unavailable.

Today, an operator can complete this with shell commands plus manual review, but
the high-risk decisions are exactly the ones `repoctl` should make explicit and
plan-first.

## 2. Design Thesis

Adoption is not a copy command. It is a graph reconstruction workflow.

`repoctl adopt` should turn one or more existing repositories into a reviewed
render plan:

1. inspect source repositories;
2. infer project purpose and monorepo placement;
3. infer workspaces, tasks, IaC, deploy, facades, public clients, and generated
   artifacts;
4. rewrite internal dependencies to monorepo-local edges;
5. generate or update CI;
6. verify that the graph and builds still work.

Every irreversible or broad operation must be visible before it is applied. The
operator should be able to run a dry plan, inspect confidence scores and reasons,
override placement, then apply the plan.

## 3. Goals

- Add `repoctl adopt` for importing existing standalone repositories into a
  functional monorepo.
- Add first-class project kinds for `tools/<name>` and optionally componentized
  `core-infra/<name>` projects.
- Generate project manifests by inspecting real manifests, not by applying
  generic scaffolds.
- Support dependency rewrites for Rust path dependencies and Node `file:`
  dependencies.
- Add explicit task prerequisites so local packages such as
  `@cellis/operon-infra` are built before consumers type-check.
- Improve GitHub Actions generation so a repo initialized by `repoctl` can run
  all relevant project cases.
- Split validation into structural and metadata modes so package registry
  failures do not masquerade as graph model failures.
- Detect generated artifact leakage after migration.

## 4. Non-Goals

- Do not merge unrelated language workspaces into a root workspace.
- Do not delete or mutate source repositories outside the destination monorepo.
- Do not run `npm audit fix`, `cargo update`, or broad dependency upgrades as
  part of adoption.
- Do not infer business ownership from commit history.
- Do not apply IaC changes or deploy adopted applications.
- Do not support arbitrary shell hooks from source repositories.

## 5. New Command Surface

### 5.1 `repoctl adopt plan`

```bash
repoctl adopt plan \
  --source ~/projects/cellis \
  --dest ~/projects/cellis/universe \
  --exclude synapse \
  --format human
```

Options:

- `--source <PATH>`: directory containing source repos or a single source repo.
- `--dest <PATH>`: initialized functional monorepo.
- `--include <NAME>`: source repo names to include; repeatable.
- `--exclude <NAME>`: source repo names to skip; repeatable.
- `--map <SOURCE=DEST>`: placement override, for example
  `operon=frameworks/operon`.
- `--kind <SOURCE=KIND>`: project-kind override.
- `--owner <SOURCE=@owner>`: owner override.
- `--rewrite-deps <auto|off|report-only>`: default `auto`.
- `--ci <update|off|report-only>`: default `update` in apply mode.
- `--verification <structural|metadata|build>`: default `metadata`.
- `--format <human|json|github-actions>`.

Output:

- source repo inventory;
- inferred purpose and destination;
- confidence and reasons;
- files to copy;
- files and directories excluded;
- manifest operations;
- dependency rewrites;
- lockfile updates required;
- CI changes;
- verification plan;
- warnings and unresolved decisions.

### 5.2 `repoctl adopt apply`

```bash
repoctl adopt apply \
  --plan target/repoctl/adopt-plan.json
```

`apply` consumes a saved plan. It should not re-infer placement unless the user
passes `--refresh`. This makes reviewable adoption possible in a PR workflow.

### 5.3 `repoctl adopt verify`

```bash
repoctl adopt verify --plan target/repoctl/adopt-plan.json
```

Runs the verification plan without copying. It is useful after a human edits a
plan, updates package locks, or adds missing tools such as `protoc`.

## 6. Source Repository Inventory

For each source repo, collect:

- root path and repo name;
- VCS marker presence;
- README summary, first heading, and domain keywords;
- primary manifests:
  - `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`;
  - `package.json`, package lockfiles, Vite/Tsup/Vitest/TS config;
  - `Pulumi.yaml`, `package.json` under `infra/`;
  - `operon.yaml`;
  - `Makefile`;
  - `project.yml`, `project.yaml`;
  - workflow files under `.github/workflows`;
- top-level directories and conventional subdirectories:
  - `apps`, `crates`, `src`, `frontend`, `infra`, `docs`, `specs`,
    `agent`, `skills`;
- dependency references to known internal packages and registries;
- generated/build artifact directories;
- required local tools inferred from build scripts (`protoc`, `buf`,
  `cargo-lambda`, `pulumi`, `node`, `npm`, `bun`, etc.).

Inventory must not traverse into ignored heavy directories:

- `.git`;
- `target`;
- `node_modules`;
- `dist`;
- `.next`;
- `.turbo`;
- package-manager caches.

The inventory result is a typed `AdoptionInventory`, serializable to JSON.

## 7. Placement Inference

Placement inference produces a ranked list of candidates:

```text
source: operon
candidates:
  - dest: frameworks/operon
    kind: framework
    confidence: 0.96
    reasons:
      - README describes a reusable serverless framework
      - exposes Rust crates and TypeScript packages
      - apps depend on package names matching this repo
```

### 7.1 Inference Rules

Framework:

- README or package names describe reusable runtime, SDK, framework, library, or
  capability.
- Other repos depend on its crates or packages.
- Contains public reusable packages (`operon`, `@cellis/operon-client`,
  `@cellis/operon-infra`).

Foundation service:

- Service provides shared business/platform capability used by multiple apps.
- README describes registry, identity, invitation, billing, auth, or access
  service.
- Has an API/service plus public clients or shared operational role.

App:

- Product or domain application.
- May contain nested app services, workers, frontend, and app-local infra.
- Should not be imported by other app projects.

Core infrastructure:

- Account/platform infrastructure, networking, clusters, DNS, IAM, security,
  observability, CI/CD bootstrap.
- Pulumi/Terraform/OpenTofu without product runtime code.

Tool:

- Developer/agent/automation tooling that is not a product app and not a shared
  runtime framework.
- Examples: skill plugins, local generators, repo maintenance scripts.

Proto root:

- Owns source API contracts under `protos/`.

### 7.2 Confidence And Overrides

If the top candidate confidence is below `0.75`, `apply` must fail unless the
plan has an explicit override.

If multiple candidates are close, human output must show the conflict:

```text
membrane: ambiguous
  core-infra/membrane 0.72: EKS/platform infra spec
  foundations/membrane 0.41: service-like README language
required: --map membrane=core-infra/membrane
```

## 8. Project Kind Model Improvements

### 8.1 Tools Lane

Add:

```rust
ProjectKind::Tool
```

Conventions:

- `tools/<name>/project.yaml`
- project name `tools.<name>`
- allowed workspaces: TypeScript, Rust, Python, shell/docs-only.
- default tasks are optional.
- tools may depend on frameworks and foundation public clients, but product apps
  should not depend on tool internals.

This lets `tools/skills` be owned, explained, included in affected analysis, and
checked by CI.

### 8.2 Componentized Core Infra

Current `core-infra` validation treats `core-infra` as a single project root.
That is too coarse for real platform repos that have independently owned
components such as `nucleus` and `membrane`.

Add one of these models. Prefer Model A.

Model A:

```rust
ProjectKind::CoreInfra
ProjectKind::CoreInfraComponent
```

Conventions:

- lane root manifest: `core-infra/project.yaml`, name `core-infra`, kind
  `core-infra`;
- component manifest: `core-infra/<name>/project.yaml`, name
  `core-infra.<name>`, kind `core-infra-component`;
- component manifests may define independent workspaces, IaC roots, owners, and
  tasks.

Model B:

- relax `ProjectKind::CoreInfra` so paths can be `core-infra` or
  `core-infra/<name>`;
- project name convention is `core-infra` for the lane root and
  `core-infra.<name>` for components.

Policy:

- apps may not import core infra internals;
- framework/foundation dependencies on core infra should be rare and explicit;
- changes under `core-infra/**/prod/**`, `Pulumi.prod.yaml`, and production
  stacks are high risk.

## 9. Manifest Synthesis

`repoctl adopt` must synthesize `project.yaml` from inventory.

### 9.1 Workspace Detection

Rust:

- root `Cargo.toml` package becomes workspace `service` for foundation apps or
  `rust` for app/framework workspaces;
- root `Cargo.toml` workspace becomes workspace `rust`;
- nested `crates/*` and `apps/*` remain inside the owning project;
- `Cargo.lock` is retained per functional project.

Node:

- each `package.json` with a lockfile becomes a TypeScript/JavaScript workspace;
- infer workspace names from directory role:
  - `frontend`;
  - `infra`;
  - `client-ts`;
  - `<app>-frontend`;
  - `<app>-infra`.
- preserve `npm` when lockfile is `package-lock.json`, `pnpm` for
  `pnpm-lock.yaml`, `yarn` for `yarn.lock`, `bun` for `bun.lock`.

IaC:

- `Pulumi.yaml` under `infra/` means project `iac.root = infra`;
- stack files infer environments from `Pulumi.<stack>.yaml`;
- root-level Pulumi projects in `core-infra/<name>` keep their current root.

Docs/spec-only:

- a project without build manifests may be adopted as docs/spec-only with
  `tasks` omitted.

### 9.2 Task Synthesis

Rust:

```yaml
tasks:
  check:
    - workspace: rust
      command: cargo check --workspace --all-features
  test:
    - workspace: rust
      command: cargo test --workspace
  build:
    - workspace: rust
      command: cargo build --workspace
```

Single package Rust projects use `cargo check`, `cargo test`, and `cargo build`
without `--workspace`.

Node frontend:

```yaml
tasks:
  check:
    - workspace: frontend
      command: npx tsc --noEmit
  build:
    - workspace: frontend
      command: npm run build
```

Pulumi TypeScript:

```yaml
tasks:
  check:
    - workspace: infra
      command: npx tsc --noEmit
  build:
    - workspace: infra
      command: npx tsc --noEmit
```

If `package.json` has `lint` or `test`, preserve them as additional task
commands only when they are not placeholders like
`echo "Error: no test specified" && exit 1`.

## 10. Dependency Rewrite Engine

Adoption must support dependency rewrites as first-class operations, not as ad
hoc string edits.

### 10.1 Internal Dependency Registry

Build an internal package index from adopted projects:

- Rust crate names from `Cargo.toml`;
- TypeScript package names from `package.json`;
- Python package names when supported;
- public facade paths from framework manifests;
- public client paths from foundation manifests.

Each package maps to:

- owning project;
- workspace;
- package root;
- public/internal surface;
- package manager.

### 10.2 Cargo Rewrites

For each adopted Rust manifest:

- parse TOML with a structured TOML parser;
- find dependencies with `registry = "chromatin"` or known internal package
  names;
- replace app/foundation dependency on `operon` with a relative path to the
  framework facade package:

```toml
operon = { path = "../../frameworks/operon/operon", features = [...] }
```

- keep version/registry metadata inside the owning framework itself when it is
  publish metadata for that framework's internal crates;
- preserve features, optional flags, default-features, package renames, and
  target-specific dependencies;
- update `Cargo.lock` only in verification/build mode, not in structural mode.

If a dependency would point to another app, emit an error unless an explicit
policy exception exists.

### 10.3 Node Rewrites

For each adopted `package.json`:

- parse JSON structurally;
- find known internal package dependencies:
  - `@cellis/operon-infra`;
  - `@cellis/operon-client`;
  - future framework/foundation public packages.
- rewrite to relative `file:` dependencies:

```json
"@cellis/operon-infra": "file:../../../frameworks/operon/operon-infra"
```

- update lockfiles with the matching package manager when network is available;
- if network is unavailable, emit `lockfile.stale` with exact commands to run.

Published package `publishConfig.registry` in the producing package should not
be rewritten unless the user passes `--private-only`. It is part of the package's
publish contract, not a consumer dependency.

### 10.4 Local Package Buildability

Local `file:` TypeScript packages need `dist` available during consumer
type-checks. `repoctl adopt` should fix this in one of two ways:

Preferred:

- add `prepare: npm run build` to local producer packages that publish compiled
  `dist` and are consumed through `file:`.

Alternative:

- add task prerequisites in the manifest:

```yaml
tasks:
  check:
    - workspace: infra
      command: npx tsc --noEmit
      depends_on:
        - project: frameworks.operon
          workspace: infra-ts
          task: build
```

The task runner and CI generator must respect task prerequisites.

## 11. Toolchain Model Improvements

TypeScript is a language. `npm`, `pnpm`, `yarn`, and `bun` are toolchains. The
runner must not infer Bun cache variables for an npm workspace.

Add or enforce:

```rust
pub enum Toolchain {
    Cargo,
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Uv,
    Custom(String),
}
```

Environment behavior:

- npm: `NPM_CONFIG_CACHE=<repo_root>/target/npm`;
- pnpm: `PNPM_HOME` or `STORE_DIR` only when configured;
- bun: `BUN_INSTALL_CACHE_DIR=<repo_root>/target/bun`;
- cargo: `CARGO_TARGET_DIR=<repo_root>/target/rust` or project target dir;
- uv: `UV_CACHE_DIR=<repo_root>/target/uv`.

Manifest `cache_dir` should be interpreted by the selected toolchain, not by
language alone.

## 12. CI Generation Improvements

The generated `repoctl.yml` should execute work, not only validate the graph and
print a matrix.

### 12.1 Command Surface

Add:

```bash
repoctl ci workflow --provider github-actions --format yaml
repoctl ci matrix --tasks check,test,build --fallback all --format github-actions
repoctl ci run-step --task check --project apps.catalog --workspace web
```

`ci workflow` renders a maintained workflow template from the repo graph.

### 12.2 GitHub Actions Requirements

Generated workflow must include:

- `graph` job:
  - install repoctl;
  - run structural graph validation;
  - compute affected matrix.
- Rust job:
  - install pinned Rust toolchain if any workspace has `rust-toolchain.toml`;
  - install `protoc` when a Rust build script uses `prost-build` or source
    contains `build.rs` invoking proto compilation;
  - run manifest task commands.
- Node job:
  - setup Node version from `package.json.engines`, `.nvmrc`, or default;
  - install dependencies with correct package manager;
  - run local package prerequisites;
  - run manifest task commands.
- IaC job:
  - type-check Pulumi/OpenTofu/Terraform project code;
  - never apply.
- Required aggregate job:
  - name matches `repo.yaml.ci.required_check`;
  - fails if any required job fails.

### 12.3 Fallback Behavior

When changed-file detection cannot run, or when no base/head is provided,
`repoctl ci matrix` should support:

- `--fallback all`: include all projects with requested tasks;
- `--fallback none`: include nothing;
- `--fallback error`: fail.

Default should be provider-aware:

- local command without base/head: `none`;
- GitHub pull request with base/head: affected;
- GitHub push without reliable merge base: all.

### 12.4 Static Versus Dynamic Matrix

The workflow generator may initially emit static matrices from current
manifests, but it must mark them as generated and provide a command to refresh:

```bash
repoctl ci workflow --provider github-actions --write
```

Long term, dynamic matrices should come from `repoctl ci matrix`.

## 13. Validation Modes

Graph validation should have modes:

```bash
repoctl graph validate --mode structural
repoctl graph validate --mode metadata
repoctl graph validate --mode full
```

Structural:

- parse manifests;
- validate paths, ownership, policy declarations, workspace files exist;
- do not call package managers or registries.

Metadata:

- structural plus language metadata that can run offline where lockfiles and
  caches are enough;
- package manager failures become environment diagnostics with advice.

Full:

- metadata plus package-manager/index access and optional task planning checks.

Exit behavior:

- model/policy errors use normal validation failure exit code;
- environment failures use an environment-specific exit code;
- diagnostics must distinguish `registry unreachable` from `dependency boundary
  violation`.

This prevents a DNS failure from hiding whether the manifest graph itself is
valid.

## 14. Artifact Hygiene

Add:

```bash
repoctl hygiene check
repoctl hygiene clean --dry-run
```

Checks:

- nested `.git`;
- nested `.github` outside repo root;
- `node_modules`;
- Cargo `target`;
- frontend `dist`;
- package-manager caches;
- accidentally copied large binary artifacts;
- ignored generated files that are still present in the working tree.

Clean should be plan-first and scoped to generated artifacts only. It must never
delete source files or lockfiles.

Adoption must run hygiene check after copy and before final verification.

## 15. Copy Plan Semantics

Adoption copy operations must support:

- include/exclude globs;
- default excludes for VCS/build/cache artifacts;
- conflict detection;
- file mode preservation for scripts;
- symlink policy:
  - preserve relative symlinks that remain inside the project;
  - reject symlinks that escape source or destination roots unless explicitly
    allowed;
- checksum summary for copied files;
- rollback plan for files created by the apply step.

The source repository must remain untouched.

## 16. Verification Plan

Adoption should generate a verification plan with ordered stages.

Stage 1:

```bash
repoctl graph validate --mode structural
repoctl hygiene check
```

Stage 2:

```bash
repoctl graph validate --mode metadata
repoctl run check --project <project> --dry-run
```

Stage 3:

Language checks inferred from manifests:

- `cargo check --workspace --all-features` for Rust workspaces;
- `npm ci` followed by `npm run build` for frontends and SDK packages;
- `npx tsc --noEmit` for Pulumi TypeScript packages;
- docs/spec-only projects skipped with explicit note.

Prerequisites:

- detect and report missing `protoc` before running `cargo check`;
- detect unavailable package registries and emit retry commands;
- set toolchain cache dirs inside the repo target directory.

## 17. Reporting

Human output should lead with decisions and unresolved risk:

```text
Adoption plan: 10 source repos, 9 projects, 1 skipped

Placement
  operon      -> frameworks/operon       framework            0.96
  atp         -> foundations/atp         foundation-service   0.84
  nucleus     -> core-infra/nucleus      core-infra-component 0.91
  skills      -> tools/skills            tool                 0.88
  synapse     skipped by --exclude

Rewrites
  Cargo: 6 manifests updated from chromatin registry to local framework paths
  npm:   10 package.json files updated to file: dependencies

Warnings
  histone requires protoc for build.rs
  ligand frontend bundle exceeds Vite chunk warning threshold
```

JSON output should include stable IDs:

- `adoption.source.inventory`;
- `adoption.placement.low_confidence`;
- `adoption.copy.conflict`;
- `adoption.dependency.rewrite`;
- `adoption.lockfile.stale`;
- `adoption.tool.missing`;
- `adoption.hygiene.generated_artifact`;
- `adoption.verify.failed`.

## 18. Data Model Sketch

```rust
pub struct AdoptionPlan {
    pub source_root: Utf8PathBuf,
    pub dest_root: Utf8PathBuf,
    pub sources: Vec<AdoptedSource>,
    pub operations: Vec<FileOperation>,
    pub dependency_rewrites: Vec<DependencyRewrite>,
    pub manifest_syntheses: Vec<ProjectManifestSynthesis>,
    pub ci_operations: Vec<FileOperation>,
    pub verification: VerificationPlan,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct AdoptedSource {
    pub name: String,
    pub source_path: Utf8PathBuf,
    pub destination_path: RepoRelativePath,
    pub inferred_kind: ProjectKind,
    pub confidence: f32,
    pub reasons: Vec<String>,
    pub inventory: SourceInventory,
    pub skipped: bool,
}

pub struct DependencyRewrite {
    pub file: RepoRelativePath,
    pub package: String,
    pub from: String,
    pub to: String,
    pub surface: DependencySurface,
    pub owner_project: ProjectName,
}

pub struct VerificationPlan {
    pub prerequisites: Vec<ToolPrerequisite>,
    pub commands: Vec<ProcessCommand>,
}
```

## 19. Acceptance Criteria

Create a fixture equivalent to the Cellis migration:

```text
source/
  operon/
  atp/
  chromatin/
  golgi/
  histone/
  ligand/
  membrane/
  nucleus/
  skills/
  synapse/
dest/
  repo.yaml
  apps/
  foundations/
  frameworks/
  core-infra/
  tools/
```

Run:

```bash
repoctl adopt plan --source source --dest dest --exclude synapse --format json > plan.json
repoctl adopt apply --plan plan.json
repoctl graph validate --repo dest --mode structural
repoctl hygiene check --repo dest
repoctl ci workflow --repo dest --provider github-actions --write
```

Expected:

- `synapse` is absent from `dest`;
- `operon` is under `frameworks/operon`;
- `atp`, `chromatin`, and `ligand` are under `foundations`;
- `golgi`, `histone`, and `versicle`-like apps are under `apps`;
- `nucleus` and `membrane` can be represented as independently owned
  `core-infra` components;
- `skills` is represented as `tools.skills`;
- no nested `.git`, nested `.github`, `node_modules`, `target`, or `dist`
  directories remain;
- Rust app/service dependencies on Operon use local path dependencies;
- Node consumers use local `file:` dependencies for internal Operon packages;
- lockfiles are updated or stale lock diagnostics are emitted;
- generated CI includes graph, Rust, Node, IaC/type-check, and required aggregate
  jobs;
- local package producer tasks run before consumer type-check/build tasks;
- structural validation does not access package registries;
- full verification identifies missing `protoc` as a prerequisite instead of a
  graph error.

## 20. Implementation Phases

Phase 1: model hardening

- add `ProjectKind::Tool`;
- add componentized core infra model;
- add explicit `Toolchain`;
- add task prerequisites.

Phase 2: hygiene and validation modes

- implement structural/metadata/full validation modes;
- implement hygiene check and clean plans;
- distinguish environment diagnostics from policy diagnostics.

Phase 3: inventory and placement

- implement source inventory;
- implement placement inference with confidence and overrides;
- serialize adoption plans.

Phase 4: dependency rewrite engine

- implement structured Cargo rewrites;
- implement structured package.json rewrites;
- implement lockfile update/stale diagnostics;
- add local package `prepare` recommendations.

Phase 5: CI workflow generator

- generate GitHub Actions workflows from manifests and task prerequisites;
- add fallback behavior to `ci matrix`;
- add aggregate required-check job support.

Phase 6: end-to-end adoption fixtures

- add Cellis-like fixture;
- verify plan/apply/validate/hygiene/ci workflow end to end;
- document migration workflow in the user guide.

