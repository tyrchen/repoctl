# repoctl v0.2 Verification Plan

Status: Draft

Source PRD: [01-initial-spec.md](01-initial-spec.md)

Design: [repoctl-v0.2-design.md](repoctl-v0.2-design.md)

## 1. Verification Goals

Verification must prove that `repoctl` makes a functional monorepo understandable and safely operable without turning the repo root into a language workspace.

The highest-risk areas are:

- CLI accidentally accumulating business logic.
- Facade crate failing to expose enough core behavior to non-CLI frontends.
- Trait boundaries becoming too coarse to test or extend.
- Manifest validation and path containment.
- Project graph correctness.
- Boundary policy enforcement.
- Affected analysis correctness.
- Template rendering safety.
- Task execution without shell injection.
- Generated-code and prod-IaC risk detection.
- CI matrix stability.
- AI context accuracy.

## 2. Test Layers

Unit tests:

- Facade request/response construction.
- Domain newtypes and validation.
- Manifest raw-to-domain conversion.
- Glob compilation and matching.
- Graph edge construction.
- Policy rule evaluation.
- Affected propagation.
- Template path validation and render modes.
- Task command normalization.
- Diagnostic formatting.

Integration tests:

- Facade-level tests for every command family without invoking the CLI binary.
- CLI smoke tests that assert argument parsing and output rendering only.
- Temporary repo fixtures for `init`, `new`, `graph validate`, `affected`, `ci matrix`, `context`, and `pr summary`.
- Git fixtures with base/head commits.
- Fake toolchain binaries for Cargo, Bun, uv, buf, and IaC providers.
- Golden output tests for human and JSON formats.

End-to-end tests:

- `repoctl init --dry-run`.
- `repoctl init --name acme --profile startup --layout functional`.
- `repoctl new app apps/catalog --stack rust-api,bun-web,uv-jobs --iac pulumi`.
- `repoctl graph validate`.
- `repoctl run check --project apps.catalog` using fake commands.
- `repoctl affected --base <base> --head <head>`.
- `repoctl ci matrix --tasks check,test,build --format github-actions`.

Security tests:

- Path traversal rejection.
- Absolute path rejection.
- Symlink escape rejection for file writes.
- Oversized manifest rejection.
- Excessive project count and glob count rejection.
- Shell metacharacter rejection in v0.2 command strings.
- Generated-code direct edit detection.
- Prod IaC risk flag detection.

Architecture tests:

- `apps/repoctl-cli` depends on the facade crate and CLI-only dependencies.
- `apps/repoctl-cli` does not depend directly on `repoctl-engine`, `repoctl-scaffold`, `repoctl-runner`, domain crates, or adapter crates.
- Command-level behavior is grouped into coarse capability crates instead of one crate per command.
- Core use cases can be exercised with in-memory `RepoFileSystem`, fake `GitProvider`, fake `ProcessRunner`, fake `TemplateEngine`, fake `ProtoToolchainAdapter`, and fake `IacProviderAdapter`.
- Output renderers consume typed reports and do not trigger discovery, graph traversal, policy evaluation, template rendering, or task execution.

## 3. Fixture Matrix

Fixtures should live under `crates/*/fixtures` or an integration-test fixture directory once the crate layout is chosen.

Required fixtures:

```text
minimal-empty-repo
  repo.yaml only, no projects

functional-fullstack
  apps/catalog
  frameworks/service-runtime
  frameworks/observability
  foundations/identity
  protos/acme/identity/v1
  core-infra/modules

invalid-manifests
  duplicate project names
  project path mismatch
  unknown schema
  invalid owner
  invalid project id
  unknown dependency
  workspace outside project
  absolute path
  path traversal

boundary-violations
  app imports app
  app imports framework internal
  framework imports app
  foundation imports app
  app imports foundation internals

affected-cases
  app-only change
  framework facade change
  framework internal change
  proto source change
  app IaC dev change
  app prod IaC change
  core-infra change
  repo.yaml change
  template change
  skills change

template-cases
  managed file create
  managed file conflict
  managed block update
  conditional file render
  target path traversal
  missing required input

task-runner-cases
  successful fake command
  failing fake command
  missing executable
  command with rejected shell syntax
  bounded parallel execution

facade-cases
  discover through facade
  validate graph through facade
  affected through facade
  init dry-run through facade
  task plan through facade with fake process runner
  cli smoke calls facade
```

## 4. Facade And Trait Verification

The facade crate is the primary integration surface. Every user-visible command family must have a facade test before a CLI golden test.

Facade checks:

- `Repoctl::with_default_adapters` wires concrete adapters without exposing them to the CLI.
- Each facade method accepts a typed request and returns a typed report.
- Facade methods return diagnostics with stable codes and source context.
- Facade methods are callable from tests without constructing `clap` types.
- Facade request/response types are documented and do not expose internal adapter types.

Trait checks:

- `RepoFileSystem` can be replaced by an in-memory implementation for manifest, template, and init tests.
- `GitProvider` can be replaced by fixed changed-file fixtures for affected analysis.
- `ProcessRunner` can be replaced by a fake runner that records cwd, argv, env, and concurrency.
- `TemplateEngine` can be replaced by a deterministic fake renderer for planning tests.
- `WorkspaceInspector` can be implemented independently for Rust, Bun, uv, proto, and IaC workspaces.
- `PolicyRule` implementations are individually testable and report exact graph edges.

CLI boundary checks:

- CLI handlers contain no manifest parsing, graph construction, policy logic, template rendering, changed-file analysis, or process spawning.
- CLI handlers build request structs, call the facade, render reports, and map diagnostics to exit codes.
- CLI JSON output is derived from typed reports, not from string parsing.

## 5. Manifest Verification

Each manifest parser test must check both success and failure diagnostics.

Repo manifest checks:

- `schema` is present and supported.
- `name` is valid and length-bounded.
- `layout` must be `functional` for v0.2.
- Language defaults are internally consistent.
- `protos.root`, `core_infra_root`, skills roots, template roots, and context outputs are repo-relative.
- Policy modes are known enum values.

Project manifest checks:

- `name` matches kind and path convention.
- `kind` is one of app, framework, foundation-service, proto-root, core-infra.
- `owners` is non-empty unless repo defaults provide one.
- `workspaces` have unique names.
- Workspace roots and manifests stay under the project root.
- Tasks reference existing workspaces.
- Proto `owns` and `consumes` patterns are valid and under `protos/`.
- AI editable and do-not-edit patterns are project-relative.

Template manifest checks:

- Inputs have supported types and valid defaults.
- File targets are repo-relative after rendering.
- `when` expressions are limited to MiniJinja expression evaluation.
- Post-render actions can only call supported `repoctl` validations.

## 6. Graph And Policy Verification

Graph tests must assert exact nodes and edges, not just command success.

Required graph assertions:

- Every project has one project node.
- Every workspace belongs to exactly one project.
- Proto ownership points from package/path pattern to owner project.
- Reverse dependency edges are generated for framework and foundation consumers.
- App-local packages do not become globally importable projects.
- Public facade edges and internal edges are distinguishable.
- Graph construction can run with fake workspace inspectors.

Required policy assertions:

- Cross-app dependency reports the source app, target app, and offending edge.
- Framework internal dependency reports the internal path or package where possible.
- Foundation internal dependency reports the intended public client alternative when known.
- Generated-code direct edit reports the changed generated file and owning project.
- High-risk IaC reports required owners.
- Every policy rule can be tested in isolation through the `PolicyRule` trait.

## 7. Affected Verification

Affected tests must verify both the affected set and the reason tree.

Expected examples:

- Change `apps/catalog/web/src/page.tsx` affects `apps.catalog:web` tasks only.
- Change `frameworks/observability/rust/crates/observability-facade/**` affects the framework and reverse consumers.
- Change `frameworks/observability/rust/crates/observability-internal/**` affects the framework and direct facade validation, but not app consumers unless facade outputs change.
- Change `protos/acme/identity/v1/identity.proto` affects the proto owner, generated clients, and consumers.
- Change `apps/catalog/iac/stacks/prod.yaml` affects catalog IaC and emits prod risk flags.
- Change `core-infra/network/**` affects core IaC targets and emits platform/security review flags.
- Change `repo.yaml` affects repo-wide validation and CI hygiene.
- Affected analysis can run with a fake `GitProvider` and a prebuilt `RepoSnapshot`.

For CI matrix output, tests must verify:

- Empty matrices are represented in a GitHub Actions-safe way.
- Required check aggregation remains stable.
- Matrix entries include project and workspace identifiers.
- No duplicate matrix entries are emitted.

## 8. Template Verification

Template tests must compare the planned operations before applying writes.

Checks:

- `--dry-run` emits the same operation list without writing files.
- Render planning can run through the facade with an in-memory filesystem.
- Managed files are reproducible.
- Managed block updates preserve user content outside markers.
- Existing unmanaged files cause conflicts unless an explicit overwrite mode is provided.
- Rendered paths cannot escape the repo.
- Builtin template output passes manifest validation.
- Local templates cannot run arbitrary commands.
- CLI `init` and `new` tests verify the CLI only renders the facade's `InitPlan` or `RenderPlan`.

Golden snapshots should include generated `project.yaml`, `AGENTS.md`, README, workspace manifests, and CI skeleton snippets.

## 9. Task Runner Verification

Use fake executables placed in a temporary `PATH`.

Checks:

- `cwd` is the workspace root.
- Environment variables such as `CARGO_TARGET_DIR` and `UV_CACHE_DIR` are injected as configured.
- Commands are spawned as argv, not through shell.
- `ProcessRunner` receives argv and never receives a shell-concatenated command string.
- Failures include project, workspace, task, executable, and exit status.
- Missing toolchains produce environment diagnostics.
- Parallelism is bounded.
- Interrupted tasks are awaited or terminated cleanly.

## 10. CLI And Output Verification

Human output must be stable enough for users. JSON output must be stable enough for CI.

CLI tests should be smoke and golden-output tests only. Behavioral assertions belong at facade or capability-service level.

Commands requiring JSON tests:

- `graph print --format json`
- `affected --format json`
- `ci matrix --format github-actions`
- `pr summary --format json`
- `context --format json`

Commands requiring human golden tests:

- `init --dry-run`
- `graph validate`
- `explain`
- `lint-boundaries`
- `proto owners`
- `proto consumers`
- `iac plan --affected`

## 11. Milestone Acceptance Gates

Milestone 0: Functional init

```bash
repoctl init --name acme --profile startup --layout functional --dry-run
repoctl init --name acme --profile startup --layout functional
repoctl graph validate
repoctl skills check
```

Acceptance:

- No root language workspace is created.
- Expected functional directories exist.
- Generated manifests validate.
- Generated skills are synchronized.
- Init can be exercised through the facade without invoking the CLI.

Milestone 1: Project and workspace graph

```bash
repoctl graph validate
repoctl graph print --format json
repoctl explain apps.catalog
repoctl lint-boundaries
```

Acceptance:

- Graph nodes and edges match fixture expectations.
- Boundary violations fail with precise diagnostics.
- `graph validate` CLI output matches the facade validation report.

Milestone 2: App creation

```bash
repoctl new app apps/catalog --stack rust-api,bun-web,uv-jobs --iac pulumi
repoctl graph validate
repoctl run check --project apps.catalog
```

Acceptance:

- Generated app has local workspaces, tasks, docs, IaC, and AI boundaries.
- Fake or real toolchain checks run only in the app workspaces.
- Project creation can be planned through the facade and rendered by the CLI.

Milestone 3: Affected and CI matrix

```bash
repoctl affected --base origin/main --head HEAD
repoctl ci matrix --tasks check,test,build --format github-actions
repoctl run test --affected
```

Acceptance:

- Direct and transitive affected sets match the reason tree.
- Matrix output is valid JSON for GitHub Actions.
- Affected analysis uses fake Git fixtures in tests and concrete Git only in adapter tests.

Milestone 4: Proto system

```bash
repoctl proto owners protos/acme/identity/v1/identity.proto
repoctl proto consumers protos/acme/identity/v1/identity.proto
repoctl proto check
```

Acceptance:

- Ownership and consumer queries match manifests.
- Generated-code direct edits are detected.
- Buf compatibility checks are invoked when configured.
- Buf calls happen through `ProtoToolchainAdapter`.

Milestone 5: AI context and skills

```bash
repoctl context apps.catalog --for ai
repoctl pr summary --base origin/main --head HEAD
repoctl skills check
```

Acceptance:

- Context includes allowed edits, do-not-edit paths, dependencies, and commands.
- PR summary includes changed projects, affected workspaces, proto impact, IaC impact, risk flags, reviewers, and suggested commands.

Milestone 6: IaC plan

```bash
repoctl iac plan --affected
repoctl iac plan --project apps.catalog --env dev
repoctl iac plan --core --env staging
```

Acceptance:

- Plans route to the right provider and root.
- Prod and core-infra risk flags are emitted.
- No apply operation exists.
- Provider execution happens through `IacProviderAdapter`.

## 12. Code Quality Gates

For Rust source, public APIs, tests, examples, generated Rust artifacts, Cargo manifests, or feature flags:

```bash
cargo build
cargo test
cargo +nightly fmt
cargo clippy -- -D warnings
```

For Rust code changes where pedantic linting adds signal:

```bash
cargo clippy -- -D warnings -W clippy::pedantic
```

For dependency, lockfile, license, supply-chain, or release packaging changes:

```bash
cargo audit
cargo deny check
```

For documentation-only changes:

```bash
git diff --check
```

Then proofread touched Markdown and verify spec/doc indexes. Rust build gates are not required for documentation-only changes.

## 13. PRD Traceability Matrix

| PRD Requirement | Verification Surface |
| --- | --- |
| CLI is a thin layer over core crates | Architecture tests, facade integration tests, CLI smoke tests |
| Core functionality is exposed by a facade crate | Facade request/response tests for every command family |
| Code is organized with focused traits | Fake adapter tests and isolated trait implementation tests |
| Let apps be maintainable | `new app`, `explain`, AI context, local task execution, boundary tests |
| Let new apps be easy to create | Template tests, init/new end-to-end tests, generated manifest validation |
| Let core capability extraction be easy | Framework facade/internal graph tests, reverse affected tests |
| Push repo toward good structure | Policy lint tests, generated-code tests, proto ownership tests, IaC risk tests |
| Avoid root language workspace | Init fixture tests and generated file assertions |
| Preserve functional top-level layout | Init/new fixture golden tests |
| Keep language workspaces local to projects | Workspace manifest validation and generated layout tests |
| Enforce app/framework/foundation dependency rules | Graph and policy tests |
| Keep core infra separate from app/foundation IaC | IaC path ownership and risk flag tests |
| Keep proto source centralized | Proto manifest and generated-code policy tests |
| Generate AI skills and context | Skills sync/check tests and AI context golden tests |
| Support MiniJinja templates without arbitrary hooks | Template render planning and security tests |
| Make CI affected-driven | Affected reason-tree tests and GitHub Actions matrix tests |
| Make PR impact visible | PR summary golden tests |
| Make AI edits safer | Context output tests and skills sync tests |
