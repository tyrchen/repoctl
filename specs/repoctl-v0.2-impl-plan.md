# repoctl v0.2 Implementation Plan

Status: Draft

Source PRD: [01-initial-spec.md](01-initial-spec.md)

Design: [repoctl-v0.2-design.md](repoctl-v0.2-design.md)

Verification: [repoctl-v0.2-verification-plan.md](repoctl-v0.2-verification-plan.md)

Research: [repoctl v0.2 dependency research](../docs/research/repoctl-v0.2-dependency-research.md)

## 1. Implementation Strategy

Build `repoctl` from the center outward:

1. Facade crate and thin CLI boundary.
2. Validated manifests and domain model.
3. Repo discovery and graph construction.
4. Policy validation.
5. Plan-first file generation.
6. Affected analysis and task execution.
7. CI, PR, proto, IaC, and AI outputs.

This avoids building a template tool or CLI shell before the data model is trustworthy. The first useful executable should validate a real repo and explain what it sees.

The current source repository can keep its existing root Cargo workspace during implementation. The no-root-language-workspace rule applies to monorepos generated or adopted by `repoctl`.

The CLI app must depend on the facade crate for all repoctl behavior. Command handlers should only translate CLI arguments into facade requests, call the facade, render the typed response, and map diagnostics to exit codes.

Command behavior should be grouped into a few capability crates, not one crate per command:

- `repoctl-engine`: discovery, graph validation, policy evaluation, affected analysis, CI matrix, PR summary, AI context, and explain views.
- `repoctl-scaffold`: init, new project creation, template rendering, and skills sync/check.
- `repoctl-runner`: task execution, proto checks, IaC plan routing, and generated-code/codegen checks.

## 2. Dependency Direction

Add dependencies only when the phase needs them.

Initial likely dependencies:

- `clap` for CLI derive.
- `serde`, `serde_json`, `serde_norway` for manifest parsing.
- `schemars`, `jsonschema` for schema generation and validation.
- `camino` for UTF-8 repo paths.
- `ignore`, `globset` for discovery and policy matching.
- `petgraph` for cross-language graph representation.
- `cargo_metadata` for Rust workspace discovery.
- `minijinja` for templates.
- `thiserror` for library errors and `anyhow` at the CLI boundary.
- `tokio` for process execution and bounded concurrency.
- `winnow` only where a real grammar parser is needed.
- `typed-builder` only for structs with many fields.

Do not add `serde_yaml` or `serde_yml`.

## 3. Phase 0: Repository And CLI Bootstrap

Goal: make the project capable of hosting a serious CLI implementation without putting core behavior in the CLI.

Work:

- Add or update `rust-toolchain.toml` to latest stable Rust 1.96.0.
- Add crate-level lints required by project policy.
- Create the facade crate, preferably `crates/repoctl`, as the only core crate the CLI directly consumes.
- Replace the placeholder `apps/server` binary with a `repoctl` CLI binary or create `apps/repoctl-cli`.
- Add top-level CLI parsing with subcommand handlers that build typed facade requests.
- Keep command stubs in the facade, not in the CLI, while a use case is incomplete.
- Add shared error and diagnostic types.
- Add the first adapter traits: `RepoLocator`, `RepoFileSystem`, `ManifestParser`, `GraphBuilder`, `ProcessRunner`, and `TemplateEngine`.
- Add in-memory or fake implementations for tests before concrete adapters become complex.
- Create coarse capability crates only when needed: `repoctl-engine`, `repoctl-scaffold`, and `repoctl-runner`.
- Add Makefile targets only when a new recurring workflow is introduced.

Deliverable:

- `repoctl --help` works.
- `repoctl --version` works.
- `apps/repoctl-cli` depends on the facade crate and CLI-only dependencies.
- Empty command handlers compile behind typed facade request/response boundaries.
- Core behavior is callable from Rust tests without spawning the CLI binary.

Verification:

```bash
cargo build
cargo test
cargo +nightly fmt
cargo clippy -- -D warnings
```

## 4. Phase 1: Manifest Domain

Goal: parse and validate `repo.yaml`, `project.yaml`, and `template.yaml`.

Work:

- Create domain newtypes for project names, workspace names, owner handles, task names, schema IDs, and repo-relative paths.
- Create core request/response data structures used by the facade, including `DiscoverRequest`, `RepoSnapshot`, `ValidationReport`, `InitRequest`, `InitPlan`, `AffectedRequest`, `AffectedReport`, `TaskRunPlan`, and `TaskRunReport`.
- Implement raw serde structs for repo, project, proto-root, and template manifests.
- Convert raw structs to validated domain structs with `TryFrom`.
- Add source-aware diagnostics.
- Add JSON Schema generation.
- Add manifest fixture tests for valid and invalid examples.
- Add hard caps for manifest bytes, string lengths, owners, workspaces, tasks, patterns, and dependencies.

Deliverable:

- A library API can load a manifest from bytes and return a validated domain object.

Verification:

```bash
cargo test manifest
cargo clippy -- -D warnings -W clippy::pedantic
```

## 5. Phase 2: Repo Discovery And Snapshot

Goal: construct a validated `RepoSnapshot` from a repo root.

Work:

- Implement repo-root resolution behind `RepoLocator`.
- Load `repo.yaml`.
- Walk roots with `ignore` behind `RepoFileSystem`.
- Discover `project.yaml` files deterministically.
- Parse and validate project manifests.
- Normalize repo-relative paths.
- Enforce project path uniqueness and manifest location consistency.
- Exclude `target/**` and generated caches from discovery.
- Add `repoctl graph validate` using discovery without graph policies first.
- Expose discovery through the facade as `Repoctl::discover`.

Deliverable:

- `repoctl graph validate` can validate an empty repo and a multi-project fixture.
- The same validation can be run directly through the facade in integration tests.

Verification:

```bash
cargo test discovery
cargo test graph_validate
```

## 6. Phase 3: Graph And Policy

Goal: make repo boundaries enforceable.

Work:

- Build graph nodes for projects, workspaces, proto packages, and IaC targets.
- Add declared dependency edges from manifests.
- Add `WorkspaceInspector` trait and adapter-discovered edges for Rust path dependencies using `cargo_metadata`.
- Add facade/internal edge classification for frameworks.
- Add foundation public client/internal classification.
- Implement policy rules for cross-app dependencies, facade-only framework use, foundation-client-only use, generated-code direct edits, and high-risk IaC.
- Implement each boundary rule as a small `PolicyRule` implementation.
- Add `repoctl graph print`.
- Add `repoctl explain`.
- Add `repoctl lint-boundaries`.

Deliverable:

- Boundary violations in fixtures produce precise diagnostics.

Verification:

```bash
cargo test graph
cargo test policy
cargo test explain
```

## 7. Phase 4: Init, Templates, And Skills

Goal: create functional layout safely and reproducibly.

Work:

- Implement template source resolution for builtin and local templates.
- Implement `TemplateSourceResolver` and `TemplateEngine` traits.
- Implement MiniJinja rendering with a constrained context behind the template engine trait.
- Implement plan-first file operations.
- Implement managed, block, and copy render modes.
- Implement conflict detection and `--dry-run`.
- Build builtin templates for repo initialization and default skills.
- Implement `repoctl init`.
- Implement `repoctl skills sync` and `repoctl skills check`.
- Ensure `repoctl init` never creates root language workspace files.

Deliverable:

- A temporary directory can be initialized as a functional monorepo and validated.

Verification:

```bash
cargo test template
cargo test init
cargo test skills
```

Manual acceptance:

```bash
repoctl init --name acme --profile startup --layout functional --dry-run
repoctl init --name acme --profile startup --layout functional
repoctl graph validate
repoctl skills check
```

## 8. Phase 5: New Project Commands

Goal: create apps, frameworks, and foundation services from templates.

Work:

- Implement `repoctl new app`.
- Implement stack parsing for `rust-api`, `bun-web`, and `uv-jobs`.
- Implement app-local IaC template selection.
- Implement `repoctl new framework` with facade and internal areas.
- Implement `repoctl new foundation` with service and clients.
- Add post-render validation for project manifests and graph.
- Add fixture-backed golden output tests.

Deliverable:

- New projects are immediately visible to graph validation and AI context.

Verification:

```bash
cargo test new_app
cargo test new_framework
cargo test new_foundation
```

Manual acceptance:

```bash
repoctl new app apps/catalog --stack rust-api,bun-web,uv-jobs --iac pulumi
repoctl new framework frameworks/service-runtime --languages rust,typescript --facade true
repoctl new foundation foundations/identity --service rust --clients rust,typescript,python --iac pulumi --proto acme.identity.v1
repoctl graph validate
```

## 9. Phase 6: Affected Analysis And Task Runner

Goal: make CI and local runs affected-driven.

Work:

- Implement Git changed-file adapter for base/head commits.
- Keep Git access behind `GitProvider`.
- Map changed files to owning projects, workspaces, proto packages, templates, skills, and IaC targets.
- Implement graph propagation for project, framework facade, proto, IaC, and repo-wide changes.
- Store reason trees.
- Implement `repoctl affected`.
- Implement task selection by project/workspace/affected set.
- Normalize command strings to argv and reject shell-only syntax.
- Inject toolchain environment variables.
- Keep process spawning behind `ProcessRunner`.
- Keep language-specific environment setup behind `ToolchainAdapter`.
- Run tasks with bounded concurrency and structured summaries.
- Implement `repoctl ci matrix` and `repoctl ci summarize`.

Deliverable:

- A PR fixture can produce affected output, CI matrices, and task runs using fake toolchains.

Verification:

```bash
cargo test affected
cargo test task_runner
cargo test ci_matrix
```

Manual acceptance:

```bash
repoctl affected --base origin/main --head HEAD
repoctl ci matrix --tasks check,test,build --format github-actions
repoctl run test --affected
```

## 10. Phase 7: Proto System

Goal: centralize proto ownership and generated-code policy.

Work:

- Parse `protos/project.yaml`.
- Map proto path patterns to owners and consumers.
- Implement `repoctl proto owners`.
- Implement `repoctl proto consumers`.
- Implement `ProtoToolchainAdapter`.
- Implement `repoctl proto check` with buf invocation when configured.
- Detect generated-code direct edits.
- Connect proto changes to affected analysis and PR summary.

Deliverable:

- Proto ownership and consumer impact are explainable from manifests.

Verification:

```bash
cargo test proto
cargo test generated_code_policy
```

Manual acceptance:

```bash
repoctl proto owners protos/acme/identity/v1/identity.proto
repoctl proto consumers protos/acme/identity/v1/identity.proto
repoctl proto check
```

## 11. Phase 8: AI Context And PR Summary

Goal: expose repo graph knowledge to AI agents and reviewers.

Work:

- Implement `repoctl context <project> --for ai`.
- Include editable paths, do-not-edit paths, dependencies, commands, proto context, IaC boundaries, and policy rules.
- Implement `repoctl pr summary`.
- Include changed projects, affected workspaces, affected tasks, proto impact, IaC impact, risk flags, suggested reviewers, and suggested commands.
- Add Markdown and JSON output tests.

Deliverable:

- A fixture PR produces a stable human PR summary and machine-readable impact data.

Verification:

```bash
cargo test ai_context
cargo test pr_summary
```

Manual acceptance:

```bash
repoctl context apps.catalog --for ai
repoctl pr summary --base origin/main --head HEAD
repoctl skills check
```

## 12. Phase 9: IaC Plan

Goal: route IaC plans without making repoctl a deploy system.

Work:

- Parse app, foundation, and core-infra IaC specs.
- Implement `IacProviderAdapter`.
- Implement provider adapters for Pulumi, Terraform, and OpenTofu plan commands.
- Implement `repoctl iac plan --affected`.
- Implement `repoctl iac plan --project <project> --env <env>`.
- Implement `repoctl iac plan --core --env <env>`.
- Emit prod and core-infra risk flags.
- Ensure no apply command exists.

Deliverable:

- IaC changes route to the correct root and provider with review risk context.

Verification:

```bash
cargo test iac
cargo test risk_flags
```

Manual acceptance:

```bash
repoctl iac plan --affected
repoctl iac plan --project apps.catalog --env dev
repoctl iac plan --core --env staging
```

## 13. Release Readiness

Before tagging v0.2:

- All milestone acceptance commands pass against a full fixture repo.
- CLI architecture check confirms `apps/repoctl-cli` does not depend on capability, domain, or adapter crates directly.
- Capability crate check confirms command-level behavior is grouped under `repoctl-engine`, `repoctl-scaffold`, and `repoctl-runner`, not split into one crate per command.
- Facade integration tests cover each public command family without invoking the CLI binary.
- `cargo build`, `cargo test`, `cargo +nightly fmt`, and `cargo clippy -- -D warnings` pass.
- `cargo audit` and `cargo deny check` pass because dependencies and lockfiles will have changed.
- `repoctl init` output is manually inspected to confirm no root language workspace is generated.
- `repoctl pr summary` is manually inspected on at least one realistic multi-project change.
- Docs and specs indexes are updated.

## 14. Key Risks And Mitigations

YAML parser risk:

- Isolate the YAML parser in an adapter and keep domain validation independent.

Root workspace confusion:

- Document that the `repoctl` source repo may use a Rust workspace, while generated target repos must not.

Task command injection:

- Normalize to argv and reject shell-only syntax in v0.2.

Graph false positives:

- Require reason trees and exact fixture assertions for affected propagation.

Template overwrite risk:

- Use plan-first writes, managed markers, conflict detection, and dry-run support.

Remote template supply-chain risk:

- Keep external Git templates deferred until pinned refs, checksums, and source allowlists are implemented.

IaC blast radius:

- Implement plan only, never apply, and require explicit risk flags for prod and core-infra paths.

## 15. Recommended First Implementation Slice

The first slice should be:

1. Facade crate with typed request/response shells.
2. Thin CLI bootstrap that calls the facade.
3. `repo.yaml` and `project.yaml` domain parsing.
4. Repo discovery behind traits.
5. `repoctl graph validate`.
6. A minimal fixture with one app, one framework, one foundation, and one proto root.

This slice proves the core model before templates, CI, or task execution add complexity.
