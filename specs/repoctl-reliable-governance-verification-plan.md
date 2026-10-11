# Reliable Governance Verification

This records the acceptance checks for the [design contract](repoctl-reliable-governance-design.md)
and the implementation delivered for 0.7.0. Verified on macOS with Rust 1.99.0;
the PR workflow additionally exercises Linux and real Buf compatibility.

## Acceptance evidence

| Contract | Automated evidence |
| --- | --- |
| G1 | Real process tests at concurrency 1 and 8; multi-command prerequisites, shared diamond, independent parallel readiness, failed descendant skipping, missing targets and cycles. |
| G2 | Deadline and output-overflow tests execute real Python processes; failure returns bounded diagnostics. |
| G3 | Runner tests cover all eight project dependency edge kinds and cyclic graphs. CLI tests verify infrastructure propagation, evidence, transitive reviewers and unowned-path diagnostics. |
| G4 | Isolated Git repositories test both rename owners, local untracked/deleted changes, merge-base comparison and invalid refs. |
| G5 | Generated workflow YAML is parsed; matrix-selected tasks execute locally. Shell tests cover successful, empty, failed and cancelled aggregation. Dependency installation tests cover two toolchains in one directory and stopping on installation failure. |
| G6 | Generic Cargo/Node migration tests preserve aliases, target tables, features and optional/default settings; missing workspace inheritance fails. The existing adoption apply/validation regression remains passing. |
| G7 | Offline fixtures cover Cargo, Node, normalized PEP 621, uv and Poetry path dependencies, malformed manifests and duplicate package names. |
| G8 | Engine test pins one immutable snapshot while files change and verifies the next discovery refreshes. Doctor composes all sections using the pinned snapshot. |
| G9 | Real execution tests verify restoration, source/dependency/environment/runtime invalidation, bypass, corrupt/escaped entries, failed-result exclusion, unsafe dry-run contracts, empty directories, executable bits and symlink rejection. |
| G10 | Fake executable tests verify argv and failing reports. Buf 1.73.0 is also executed against actual deleted-field schemas using both directory and Git comparison sources. |
| G11 | Context tests cover UTF-8 budgets, protected source, declared documentation directories and secret exclusion. Mermaid tests cover stable ordering, escaping and selected tasks; public examples compile as doctests. |

CLI regression sources are [governance.rs](../apps/repoctl-cli/tests/governance.rs)
and its [additional contracts](../apps/repoctl-cli/tests/regressions/contracts.rs),
[cache/graph/CI regressions](../apps/repoctl-cli/tests/regressions/mod.rs), and
[real Buf contract](../apps/repoctl-cli/tests/regressions/buf_contract.rs).

## Gates

Run and require success:

```text
make build
cargo test --workspace
make test
cargo doc --workspace --no-deps
cargo +nightly fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings -W clippy::pedantic
cargo audit
cargo deny check
make check-agent-sync
REPOCTL_TEST_BUF=<Buf 1.73.0 executable> make test-buf
git diff --check
```

The workspace suite contains 118 regular tests and eight doctests. The separately
enabled real Buf test adds one passing integration test. Audit has no vulnerability
findings; deny passes advisories, bans, licenses and sources. Non-failing deny
warnings concern duplicate dependency versions and unused license allowances.
Changed Markdown relative links and both documentation/spec indexes are checked.

## Doctor and code-size scope

This source repository has no `repo.yaml`; its architecture is the Cargo workspace,
not an invented repoctl project graph. Use a temporary repository generated with
`repoctl init`, copy the source into an owned `tools.repoctl` workspace, and declare
a check task. This keeps graph inputs outside the source checkout.

```text
repoctl doctor --repo <fixture> --agent --format json --changed-file <each new Rust module>
repoctl inspect size --repo <fixture> --scope changed --fail-on warning --format json --changed-file <each new Rust module>
repoctl inspect size --repo <fixture> --scope all --format json
```

Doctor passes all sections and selects one project/one task. New-module inspection
scans 25 Rust files with zero findings. The all-source scan retains 13 existing
warnings, confirmed by separately scanning committed master with the same defaults:

- Existing oversized files: CLI dispatcher, core domain/manifest, inspect, runner,
  scaffold and facade roots.
- Existing oversized CLI `run` function and rendering blocks; the unchanged runner
  provider-capability blocks.

Master has 14 warnings. Extracting adoption, execution and impact modules removes
one old block warning and reduces facade effective LOC from 3256 to 1811 and runner
from 2673 to 2359. CLI dispatch grows from 642 to 676 lines. No thresholds or
exclusions were changed to hide findings. Existing dispatcher/model size remains
explicit maintenance debt; this PR does not claim the entire source tree meets the
default size policy.

## PR review follow-up

All three review findings are valid and fixed:

- CI planning retains typed `selected_tasks` separately from the command closure.
  Both affected and fallback-all matrices emit only originally selected identities;
  toolchain metadata and execution still include prerequisites. The new CLI
  regression verifies same-name prerequisites in an unaffected project, two-command
  preparation, exactly one affected row, correct toolchain flags, and execution
  counts. Independently selected tasks remain rows in the fallback-all case.
- The graph builder uses `WorkspaceInspector::inspect_all`; existing adapters retain
  their default per-workspace implementation. Node/Python overrides prepare name
  indexes and parsed manifests once per construction, without a global cache. An
  instrumented real-filesystem test checks exactly 32 reads for 32 packages, then
  checks 64 cumulative reads and updated edges after editing a manifest and
  rebuilding with the same builder.
- Task planning rejects more than 256 distinct prerequisites across all commands
  of a task identity, before dependency expansion or cloning. Boundary tests use
  individually valid commands with repeated prerequisites: 256 distinct targets
  plan successfully; 257 fail with `task.too_many_prerequisites` before resolving
  a deliberately missing target.

Regression code lives in [review.rs](../apps/repoctl-cli/tests/regressions/review.rs)
and the package inspector's [unit tests](../crates/repoctl-engine/src/local_inspectors/packages.rs).
Build, workspace tests, nightly formatting and strict pedantic clippy pass.
Follow-up doctor/size verification uses the same temporary managed fixture and
explicit changed paths: graph and other health sections are clear; doctor reports
four inherited size warnings in core domain and runner roots (including the two
unchanged provider-capability blocks). The focused eight-file scan has no findings;
the all-source count remains 13. No threshold was relaxed. Audit/deny and real Buf
need no additional local run for this source-only follow-up: dependencies, lockfiles,
release packaging and proto/process execution are unchanged; hosted CI still runs
its complete Rust/Buf gate.

## Release boundaries and skipped gates

- Publish the ordered 0.7.0 crate chain before using generated workflows outside
  local integration fixtures. This PR does not publish a release.
- Cache is local, explicit and intended for reproducible non-secret tasks. It is
  neither a remote cache nor a filesystem sandbox.
- Timeouts terminate/reap the direct child and bound pipe capture. Cross-platform
  process-tree isolation is not provided.
- No deployment, remote registry publishing, Miri, cross-platform release packaging
  or full external managed-monorepo build is run: those are outside the changed
  surfaces. There is no new unsafe code. The PR's own CI runs the Linux Rust/Buf
  checks; the generated standalone workflow is tested structurally and through its
  selected CLI execution, and awaits the 0.7.0 release for hosted adoption.

## Cargo and adoption review follow-up

The next four P1 findings are valid and fixed. Six additional end-to-end tests in
[adoption review](../apps/repoctl-cli/tests/regressions/adoption_review.rs) and
[adoption execution](../apps/repoctl-cli/tests/regressions/adoption_execution.rs) cover:

- Virtual Cargo roots, explicit/globbed and sibling members, both explicit and glob
  exclusions, inherited workspace dependencies (including `package.workspace`),
  target/build/dev tables and implicit path packages. A framework change propagates
  to the virtual-workspace consumer. Missing members and escaped glob prefixes fail.
- Nearest pnpm/yarn/npm lockfile ownership, a nested Bun override, absent locks,
  actual child scripts and full-verification package-manager selection. A fake pnpm
  executable verifies frozen installation in the owning root and script execution
  in the child package directory.
- Stale npm/pnpm/Yarn classic/Yarn modern/Bun text locks omitted from file operations,
  typed refresh commands, one root refresh for multiple rewritten child packages,
  successful apply without copying old locks, and verification failure before refresh.
  A separate real npm check uses `--offline --no-audit --no-fund` with local packages:
  the planned command creates a v3 lock, after which `adopt verify` succeeds. Yarn
  refresh modes follow [modern](https://yarnpkg.com/cli/install) and
  [classic](https://classic.yarnpkg.com/en/docs/cli/install) documentation; Bun uses
  its documented [lockfile-only mode](https://bun.com/docs/pm/lockfile).
- Existing project manifests at planning time, files appearing between plan/apply,
  duplicate generated operations, and CI operations targeting project metadata.
  Rejections preserve existing content and happen before any copied file appears.

Build, 113 workspace tests, seven doctests, 113 nextest cases, nightly formatting,
strict pedantic clippy and API documentation pass. The managed handoff fixture's
12 focused Rust files have zero size findings. Doctor has no graph/ownership or
other health errors and retains seven inherited warnings in the touched CLI,
core domain and facade roots. The all-source scan remains at 13 existing warnings;
no policy thresholds were changed. Dependencies, lockfiles, agent instructions and
Buf execution are unchanged in this follow-up, so audit/deny, agent sync and the
separate local real-Buf gate were not repeated; hosted CI still exercises real Buf.


## Cache, CI and Python review follow-up

Four additional findings are valid and fixed; the Windows rename finding is rejected
with the primary [Rust 1.99 API contract](https://doc.rust-lang.org/std/fs/fn.rename.html):
rename replaces existing regular destination files on Windows as well as Unix.
The existing corrupt-cache regression exercises this replacement; removing the old
file first would lose the atomic replacement behavior.

Five new tests in [cache dependencies](../apps/repoctl-cli/tests/regressions/cache_dependencies.rs)
and [language/CI review cases](../apps/repoctl-cli/tests/regressions/review_round_two.rs) cover:

- A silent uncached generator changes excluded `dist` files from an environment
  input. Both cached descendants bypass with warnings and produce current results.
  Fully cached prerequisites allow hits and invalidate descendants on artifact changes;
  a mixed cached/uncached prerequisite group remains ineligible for descendant caching.
- Protoc flags for member build dependencies, renamed target dependencies, default
  build scripts, custom build paths and repository-local dependency build scripts.
  Disabled scripts and unused excluded members do not enable the flag. Detection
  shares bounded offline Cargo traversal with graph inspection.
- Direct adoption of pure Python repositories, nearest uv locks and nested overrides.
  A fake uv executable verifies `sync --frozen` at the root lock owner and the
  generated Python check in the selected package directory.
- Poetry dev/optional QA groups with local paths and normalized distribution-name
  references. Both edges and affected consumers are verified offline.

Build, 118 workspace tests, eight doctests, nextest, nightly formatting and strict
pedantic clippy pass. Doctor and changed-source size checks pass in the managed
fixture with zero findings across 11 changed Rust files. The all-source scan still
has 13 pre-existing warnings. Dependencies and release packaging did not change;
audit/deny were not repeated. Real Buf remains exercised by hosted CI.
