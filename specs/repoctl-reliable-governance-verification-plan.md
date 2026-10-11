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

The workspace suite contains 104 regular tests and five doctests. The separately
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
