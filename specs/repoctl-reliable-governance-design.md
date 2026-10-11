# Reliable Multilingual Governance

## Product decision

repoctl's product is a shared, validated model of multilingual architecture,
explainable change impact, and AI editing boundaries. Humans, local task execution,
CI, and agents must obtain consistent answers from that model. This change completes
the reliability work and adds the six capabilities identified in the repository
review. Nx informs explicit task dependencies and cache inputs/outputs; Buf owns
Protobuf compatibility rules. Neither tool becomes a mandatory runtime dependency
for repositories that do not use its corresponding capability.

## Scope and acceptance contract

| ID | Deliverable | Acceptance |
| --- | --- | --- |
| G1 | Dependency-aware execution | A consumer starts only after every prerequisite succeeds, at every concurrency; independent ready tasks run concurrently. Multiple commands belonging to one task all execute in manifest order. Missing targets and cycles are errors. |
| G2 | Bounded processes | Commands use argv, a configurable deadline and bounded stdout/stderr. Timeout/output overflow fails the command and terminates/reaps the direct child. All worker handles are joined; failed prerequisites cause explicit skipped diagnostics. |
| G3 | Complete impact | Every project dependency edge, including infrastructure and internal surfaces, participates in reverse propagation. Reviewers include transitive owners; explanations retain edge evidence. Shared policy/CI/lockfile changes select all projects. |
| G4 | Correct Git change detection | Ref ranges compare merge-base to head; NUL-delimited name-status decoding includes both rename paths. Without refs, staged, unstaged, deleted, and untracked paths are included. Invalid refs fail clearly; paths and refs are bounded. |
| G5 | Executable CI | A standalone generated repo installs a pinned released CLI in each independent job, fetches full history, supplies explicit refs, consumes a task matrix, installs selected toolchains and dependencies, and executes checks. Required aggregation rejects failed/cancelled jobs and accepts intentional empty selection. |
| G6 | General adoption | Index every source package, with no organization/package-name allowlist. Rewrite only the source of a dependency; preserve features, optional/default settings, package aliases and target tables. Duplicate package names fail explicitly. |
| G7 | Multilingual offline graph | Discover Cargo path dependencies, Node package-name/local-path dependencies and Python project/uv/Poetry local dependencies without invoking package managers or registries. Classify facade/client/internal areas uniformly and retain manifest evidence. |
| G8 | Shared snapshots | Doctor discovers one immutable offline graph and shares it among sections. A later invocation discovers again, so filesystem edits never reuse stale command state. Structural mode uses declared edges; metadata uses offline edges; full may run Cargo metadata. |
| G9 | Local task cache | Opt-in caching hashes explicit inputs, dependency source closure, configuration, commands, selected environment and runtime probes; successful hits restore declared artifacts and replay output. Failed, skipped, corrupt or incomplete results are never accepted as hits. |
| G10 | Buf compatibility | `proto check --against <source>` plans lint and breaking; `--execute` runs them and returns violations as failing diagnostics. Ownership/consumer data accompanies the report. Doctor remains planning-only. |
| G11 | Bounded context and impact visualization | Context can include explicitly selected changed source and declared docs, within a byte budget, with content hashes and omission reasons. Graph/affected Mermaid exports include typed evidence and deterministic IDs, with no HTML execution or implicit publishing. |

## Architecture

- core: validated execution/cache/context configuration and typed reports.
- engine: local language inspectors, shared immutable discovery and graph exports.
- runner: Git decoding, impact propagation, task planning, scheduling, bounded process
  adapter, cache storage and context enrichment. Keep these in focused modules.
- scaffold: templates only; generated CI logic belongs in a dedicated facade module.
- facade: compose services and doctor sections. Adoption inventory, rewriting and
  execution live in a dedicated module rather than the root dispatcher.
- CLI: flags, validated requests, reports and exit codes; no graph/business logic.

No remote cache, long-lived daemon, cloud deployment execution, interactive web
application, arbitrary import-language parser, or automatic package-manager install
is added to local discovery. Mermaid is a portable graph artifact, not a hosted UI.

## Task graph and execution

Task identity is `(project, workspace, task)`. A node owns all matching commands in
manifest order and the union of their declared prerequisites. Planning recursively
resolves nodes, rejects cycles/missing nodes, and returns stable topological order.
Dependency depth and prerequisites per command are bounded to 256.
Concurrency limits (1..=256) apply to ready nodes; commands inside a node run sequentially.
Independent tasks continue after another branch fails; descendants are skipped.
Reports remain in plan order, regardless of completion order. Process adapter errors
and panics become task errors, and no thread is abandoned on an early return.

Each task command accepts these optional YAML fields:

```yaml
tasks:
  build:
    - workspace: web
      command: npm run build
      depends_on:
        - project: frameworks.ui
          workspace: web
          task: build
      timeout_seconds: 300
      max_output_bytes: 1048576
      cache:
        enabled: true
        inputs: ["apps/catalog/web/src/**", "apps/catalog/web/package.json"]
        outputs: ["apps/catalog/web/dist"]
        env: ["NODE_ENV"]
        runtime: ["node --version"]
```

Defaults: 300-second deadline, 1 MiB per output stream, cache disabled. Limits:
deadline 1..=86400 seconds, output 1..=16 MiB, 256 input patterns, 64 output roots,
64 environment names, 16 runtime probes. Cache paths are repository-relative;
output roots must be strictly inside the owning workspace, may not overlap inputs,
configuration, another task's outputs, or repository metadata. All caching is opt-in
for reproducible, non-secret build/check tasks. IaC/DNS/operations commands remain
uncached. `run --no-cache` bypasses both reads and writes without changing selection.

Timeouts terminate the direct child; portable process-tree isolation is outside this
change. Pipe reads also have the same deadline, so descendants holding inherited
handles cannot hang repoctl. Captures stop at their limit; overflow fails instead of
silently allocating or accepting truncated successful output.

## Cache correctness and storage

Use BLAKE3 with a versioned format. Include repoctl version, OS/architecture, full
command configuration, explicit input matches and their contents, repo/project
manifests, source files and lockfiles in the owning project's dependency closure,
dependency task results, selected environment values and runtime command outputs.
Hash file paths as well as contents; additions/deletions invalidate the key. Exclude
declared output trees and heavy generated/cache directories. This conservative
dependency hashing is intentional; fine-grained named inputs can be added later.

Storage: `.repoctl/cache/tasks/<hash>.json`; entries contain bounded output, artifact
bytes and their digests, never environment values. Write through a unique temporary
file and atomic rename. Entry size is capped at 64 MiB, with at most 4096 artifacts and 64 directory levels.
Reject symlinks and escaped paths on snapshot and restore. Validate the whole entry
before changing outputs. Corrupt/old entries are misses with a warning. A cache
failure must not masquerade as task success. Do not cache missing declared outputs.
Restore the declared output set, including removal of stale files, before success.

## Discovery and change impact

Local inspectors read bounded manifests and map dependency package names/paths to
declared workspaces. Node handles dependencies/dev/peer/optional sections, workspace
protocols and file/link paths. Python handles PEP 621 dependencies, optional groups,
uv path sources and Poetry path dependencies; normalized distribution names are
case-insensitive and treat dash/underscore/dot equally. Cargo preserves the existing
metadata inspector for explicit full verification and adds an offline TOML path
inspector. Unknown external dependencies are not internal edges. Ambiguous local
names and malformed manifests are diagnostics, not ignored success.

An affected reason records source, target, reason and optional edge kind/evidence.
Reverse traversal uses a queue and all project dependency surfaces. Reviewers come from the union of directly/transitively affected projects; risk
flags identify changed shared-infrastructure and production-IaC paths. Unowned
changed files produce a diagnostic instead of silently hiding a selection gap.

Git uses argv, `--` separators and object-id resolution to prevent option confusion.
Rename decoding selects both old and new ownership. Committed comparison is
merge-base-aware; local changes use porcelain/NUL output. Empty change sets are
legitimate; absent/invalid refs are not interchangeable with an empty successful diff.

## CI contract

Generate `graph`, `tasks` and `required` jobs. Graph installs the current pinned
repoctl release from crates.io, performs offline validation and changed code-size
inspection, then emits a matrix and an explicit `has_tasks` output. Tasks has a real
matrix with project/workspace/task/toolchain/cwd/installation metadata. Each job
installs repoctl independently. Install Cargo/Node/Bun/uv and protobuf/Buf only when
needed; frozen/locked installation is used when the corresponding lockfile exists.
Execute `ci run-step`, resolving prerequisite tasks in the same runner so artifacts
are available; use one matrix entry per task identity.

PRs use base SHA/head SHA; pushes use `before`/head, with all-project fallback for a
missing first-push base. Matrix data goes through JSON/env arguments, never unchecked
shell interpolation of manifest values. Job timeouts and read-only contents
permissions are explicit. Required succeeds only when graph succeeds and tasks
succeeds or was intentionally skipped for an empty matrix. Cancellation is failure.
A generated workflow is tested structurally and by running its CLI-selected task
locally in a standalone fixture; hosted execution is validated by the PR's own CI.

## Migration correctness

Build per-manifest package maps for all source Cargo/Node packages. Resolve Cargo
aliases through `package`; include workspace dependencies and target tables. Change
`git`, `branch`, `tag`, `rev`, `registry`, `workspace` or `version` source fields as
needed for a local path while retaining semantic options and TOML formatting.
Unresolvable workspace inheritance is an explicit error. Rewrites are per actual
manifest dependency, so a repository's dependency union cannot rewrite unrelated
tables. Python migration remains copying/manifests; Python source rewriting is not
claimed by adoption. Build generated CI from the post-adoption manifests, not the
pre-adoption empty destination graph.

## Buf and AI interfaces

```text
repoctl proto check --against '.git#branch=main,subdir=protos' --execute
repoctl proto check --against buf.build/acme/contracts --format json
repoctl context apps.catalog --changed-file apps/catalog/api/src/lib.rs --include-docs --max-bytes 65536
repoctl graph print --diagram mermaid
repoctl affected --changed-file frameworks/ui/src/index.ts --diagram mermaid
```

Buf source is a bounded non-control string passed as one argv value. Breaking runs
from the repository root with an explicit proto input so relative Git/baseline
sources have repository-relative meaning. Breaking
requires a Buf workspace. Defaults preserve planning-only `proto check`. Executed
reports include process outputs and failed diagnostics; doctor never uses execute.
Buf owns rules/categories through `buf.yaml`; repoctl does not duplicate its parser.

Context source inclusion is explicit, project-scoped and excludes do-not-edit,
generated, binary, secret-like and symlink paths. Documentation inclusion uses
declared files or directories, at most 512 total selected files and 32 directory levels. Budget is UTF-8 bytes (default 64 KiB, max 1 MiB), not a model-specific
token estimate. Include each snippet's repo path/hash/truncation and reasons for
omissions, and bound serialized enriched output including metadata. Reject a budget
too small for mandatory metadata. Context reads use canonical containment checks.
Diagram IDs are generated independently of labels; encode labels safely and sort
nodes/edges. Affected diagrams show changed files, propagation evidence and tasks.

## Verification and rollout

Backward-compatible YAML defaults retain uncached execution and planning-only proto
checks. Additive JSON fields have serde defaults. Public request/type changes are
documented for Rust callers; bump the workspace to 0.7.0 for the changed Rust API.
No release/publish is performed by this PR. Generated workflows pin 0.7.0 and
require publishing the ordered crate chain before external use; integration fixtures
exercise the same CLI from the local build before release.

Regression tests must cover concurrency ordering, multiple commands, diamond
dependencies, independent failures, missing dependencies, cycles, deadlines,
output overflow, all dependency surfaces, rename/deletion/local changes, reviewer
propagation, multilingual offline graphs, shared doctor discovery, generic migration
and preserved Cargo options, standalone CI execution and empty/cancelled aggregate,
cache hit/restoration/invalidation/corruption/path escapes, Buf argv/nonzero exits,
context budget/UTF-8/exclusions and deterministic safe diagrams.

Run full Rust build/test/nightly fmt/clippy gates, agent sync, supply-chain audit/deny,
and targeted end-to-end CLI fixtures. Validate specs/docs indexes and links. Since
the tool's source repo has no repo.yaml, run doctor/inspect on representative managed
fixtures and report that scope honestly. Record completed behavior, exact validation
and residual limitations in the PR. All G1..G11 acceptance items are required before
the implementation is called complete.

## Sources

- [Nx caching model](https://nx.dev/docs/concepts/how-caching-works)
- [Buf breaking usage](https://buf.build/docs/breaking/usage/)
- [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)
- [Tokio child lifecycle](https://docs.rs/tokio/latest/tokio/process/struct.Child.html)
- [BLAKE3 crate](https://docs.rs/blake3/latest/blake3/)
