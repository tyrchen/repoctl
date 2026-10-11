# Reliable multilingual governance (v0.7)

repoctl connects ownership, architecture, local checks, CI and agent context through
one repository graph. Cargo, Node and Python discovery is offline by default. Use
`graph validate --mode full` when Cargo metadata resolution is explicitly needed.
Doctor shares one immutable snapshot between its sections and refreshes it on the
next invocation. Node/Python package indexes and parsed manifests are shared
within each graph construction and reread on the next construction.

## Change selection and execution

```bash
repoctl affected --base origin/main --head HEAD --tasks check,test,build
repoctl run check --affected --concurrency 4
repoctl doctor --agent
```

A committed range compares its merge-base with head. Without refs, affected
selection includes staged, unstaged, deleted and untracked paths. Renames select
both source and destination owners. Impact follows project dependencies, including
infrastructure and internal surfaces; reasons carry edge evidence and reviewers
include transitive owners. Unowned changes emit a warning.
Selection includes every workspace in each affected project, preserving consumers
of sibling workspaces under the project-scoped dependency model.

Tasks run only when every prerequisite succeeds. Multiple commands for the same
project/workspace/task execute sequentially, in manifest order. Independent ready
tasks can run concurrently; failure skips descendants and continues independent
branches. Missing prerequisites and cycles are errors. Concurrency is 1–256. The distinct
prerequisite union per task identity is capped at 256 before dependency expansion.

Commands use argv with null stdin. Default limits are 300 seconds and 1 MiB for
each output stream. Configure `timeout_seconds` (1–86400) and `max_output_bytes`
(1–16777216) on a task command. Timeout exits with status 124; output overflow with
125. repoctl terminates and reaps the direct child; process-tree isolation is not
provided. Descendants retaining pipe handles are still covered by the deadline.

## Opt-in local cache

```yaml
tasks:
  build:
    - workspace: web
      command: npm run build
      depends_on:
        - project: frameworks.ui
          workspace: web
          task: build
      timeout_seconds: 600
      cache:
        enabled: true
        inputs: ["apps/catalog/web/src/**", "apps/catalog/web/package.json"]
        outputs: ["apps/catalog/web/dist"]
        env: ["NODE_ENV"]
        runtime: ["node --version"]
```

Inputs and outputs are repository-relative. Outputs must be strictly within the
owning workspace and cannot overlap inputs, configuration, repository metadata or
another task's outputs. Input/output prefix checks are conservative: use specific
source patterns rather than a whole-workspace `**` pattern containing outputs.

The key includes command configuration, selected environment and runtime probes,
repo/project manifests and source files in the project/prerequisite dependency
closure. Standard package-manager version probes and PATH are included
automatically. Declare any additional environment or external runtime inputs;
cache only reproducible tasks whose outputs/logs contain no secrets. Filesystem
inputs outside the repository and symlinks are rejected. Heavy output/cache trees
are excluded from source hashing, including `target`, `dist`, `build`, `node_modules`
and `.venv`; do not place source inputs in these trees.

Successful entries restore declared files, executable bits and empty directories,
remove stale output files and replay captured output. Failed/skipped results are
not cached. Corrupt/incompatible entries produce a miss with a warning. Cache
entries are local to `.repoctl/cache/tasks`, bounded to 64 MiB and 4096 artifacts and 64 directory levels.
Missing declared outputs or cache I/O failures fail the task. No remote cache or
daemon is included. Put `.repoctl/cache/` in the managed repository's `.gitignore`.
Input scanning stops at 100,000 visited entries, 64 levels or 20,000 selected files;
unrelated files and empty directories count toward the traversal limit.
Only enabled cache output declarations exclude generated files from source hashing;
disabled cache outputs remain inputs.
Explicit inputs inside other tasks' enabled output trees are also hashed.

```bash
repoctl run build --no-cache
```

This bypasses both cache reads and writes. IaC and operations execution does not
use the task cache.

Cached dependents require artifact-aware fingerprints from every prerequisite
command. An uncached or partly cached prerequisite bypasses descendant caching
with a warning; successful exit status and identical logs cannot prove unchanged artifacts.

Sequential commands in one task include preceding artifact fingerprints in later
cache keys. Earlier uncached commands bypass subsequent caching, just as uncached
prerequisite tasks do.

## Buf compatibility

Keep lint/breaking policies in `buf.yaml` at the configured proto root. repoctl
delegates compatibility rules to Buf and returns owners/consumers alongside the
planned commands and execution results.

```bash
# Plan only; no Buf executable is needed.
repoctl proto check --against '.git#branch=main,subdir=protos' --format json
# Execute lint and breaking with an installed Buf CLI.
repoctl proto check --against '.git#branch=main,subdir=protos' --execute
```

`--against` is passed as one argument and may reference Git, a local baseline or a
Buf registry module. Registry comparisons use Buf's own authentication/network
behavior. Violations fail the CLI. Doctor remains planning-only.

## Standalone CI

```bash
repoctl ci workflow --write
repoctl ci matrix --base origin/main --head HEAD --tasks check,test,build --format github-actions
repoctl ci run-step --project apps.catalog --workspace web --task check --install
```

Generated workflows have graph, task-matrix and required jobs, full Git history,
explicit refs and independent pinned CLI installation. They install the selected
toolchains/dependencies and execute tasks with their prerequisites. Matrix rows
include only originally selected tasks; prerequisite-only tasks run within their
dependent job, while setup metadata includes the full closure. Dependency
installation is explicit through `--install`; local graph discovery never installs
packages. Frozen/locked installation is used when a corresponding lockfile exists.
Empty matrices intentionally skip task jobs; cancellation/failure never passes the
required gate. Buf's maintained action is used in setup-only mode.

**Release prerequisite:** generated workflows pin `repoctl-cli =0.7.0`. Publish the
ordered workspace crate chain before using those workflows in external repos.
This development PR does not publish releases. Local integration fixtures use the
just-built CLI to verify the generated workflow contract before release.

## Context and diagrams

```bash
repoctl context apps.catalog --changed-file apps/catalog/api/src/lib.rs --include-docs --max-bytes 65536
repoctl graph print --diagram mermaid
repoctl affected --changed-file frameworks/ui/src/index.ts --diagram mermaid
```

Source inclusion is explicit and project-scoped; declared docs may be files or
directories. Protected/generated, secret-like, binary and symlink paths are
excluded. Enriched JSON includes hashes, truncation and omission reasons. Budgets
are serialized UTF-8 bytes (default 64 KiB, maximum 1 MiB), including metadata.
At most 256 explicit changed files and 512 total selected files are supported;
documentation walking is limited to 32 directory levels and 4096 visited entries
across all declared roots, including empty directories and excluded entries. A budget too small for
mandatory metadata is an error. Mermaid output uses deterministic IDs and escaped
labels and is not published automatically. Documentation exclusions use repository
relative paths, so checkout ancestor names do not hide permitted documents.

## Adoption and Rust callers

Adoption indexes all Cargo/Node packages, rejects duplicate names and preserves
Cargo aliases, features, optional/default-feature settings and target tables when
rewriting dependency sources. Unresolvable workspace inheritance is an error.
Placement uses README/content indicators or explicit `--map` overrides. Task
synthesis uses actual workspace names, existing Node scripts/TypeScript config and
Python compile checks. Python dependency-source rewriting is not included.
Cargo virtual workspaces expand explicit/globbed members and excludes offline,
including inherited, target-specific and implicit member path dependencies. Ordinary
Cargo packages also expose dependencies of project-local path packages. Pinned root
toolchain, `.cargo/config.toml`/`.cargo/config` and root Make/Just/Task routing changes select every workspace. Adoption
groups Cargo members under their actual workspace root and shared lockfile, emits
`--workspace` tasks and verifies each owning root once. Independent nested packages
remain separate, and workspace names are unique across all languages.
Node child packages keep their scripts while inheriting the nearest lockfile owner
and package manager; CI installs in the owner directory. Unlocked packages do not
declare invented lockfiles.
Without a lockfile, tasks, CI and full verification use the nearest declared
`packageManager` (inherited from an ancestor when absent), then default to npm.
Python packages inherit their nearest existing `uv.lock`, with frozen installation
in its owner directory. Offline discovery includes Poetry development/test groups.
CI protoc requirements include Cargo members, local path dependencies and active
default/custom build scripts.

Automatic Node rewrites omit stale locks and expose required `lockfileRefreshes`.
After `adopt apply`, run these commands in their declared directories, commit the
regenerated locks, then run `adopt verify`; missing required locks fail verification.
Apply executes file operations only. Planning and apply preflight reject existing
project.yaml/project.yml, other file conflicts, duplicate output paths and duplicate
project identities, including collisions with existing destination projects.
A destination nested in the source is excluded from discovery and copying. Source
and destination cannot be identical. Migration caps selected files/final operations
at 20,000, entries at 100,000, candidates at 256 and depth at 64; inspected text is
limited to 256 KiB per file and 64 MiB total. Oversized sources fail explicitly.
Source-backed operations record BLAKE3 digests of original bytes, including binary
files and rewritten manifests. Apply verifies all digests before writing; changed,
missing or unreadable sources fail preflight. Regenerate old plans without digests.
Streaming source hashing is capped at 256 MiB per file and 1 GiB total.
Saved plans are capped at 64 MiB before parsing. Apply and verify validate all
collection limits at the same boundary; generated plans meet that size limit too.
A README-only source root is recognized directly and can be mapped to a
documentation tool without inventing build tasks.
Saved `--ci report-only` plans retain a workflow preview without writing it during
apply; use `--ci update` to write the generated workflow.
Legacy `bun.lockb` selects Bun and is preserved as binary data. When dependencies
are rewritten, the explicit refresh creates `bun.lock` and generated project
manifests use that new text lockfile.


Rust request/command structs gained execution/cache/context/proto options; the
workspace version is bumped to 0.7.0 for these API changes. Prefer default request
construction plus explicit field assignment. JSON additions have defaults where
older reports/requests need compatibility; YAML execution/cache options are
optional and preserve uncached, planning-only behavior by default.

See the [design and acceptance contract](../../specs/repoctl-reliable-governance-design.md).
