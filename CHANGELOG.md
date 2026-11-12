# Changelog

All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [unreleased]

### Bug Fixes

- **(runner)** hand toolchains absolute target and cache directories (#6) - ([51b3ab1](https://github.com/commit/51b3ab12d4630c7b13ee84052c3e341acc90a011)) - Tyr Chen
- **(scaffold)** omit protos/ from generated skills when repo.yaml has no protos block (#10) - ([cad2825](https://github.com/commit/cad282512df69f14d9e0cd2418e5ea5a61c4ad12)) - Tyr Chen
- **(scaffold)** stop generating docs/index.md in init and new projects - ([a5cb3e2](https://github.com/commit/a5cb3e24a66abec5f6cbce30ff11cd68ce3a6f38)) - Tyr Chen
- keep generated CI version test in sync - ([0295d14](https://github.com/commit/0295d140f9aee748d3d7164a971674884ac6f336)) - Tyr Chen

### Features

- **(runner)** attribute root Cargo workspace changes to the projects they reach (#8) - ([621b89a](https://github.com/commit/621b89af89aff5c19b335aab1d1f87596cd9af6f)) - Tyr Chen
- **(scaffold)** join new crates to a root Cargo workspace (#9) - ([df9b9c0](https://github.com/commit/df9b9c068ad568302d7358c5118f5812d5cdf07a)) - Tyr Chen

### Miscellaneous Chores

- fix repoctl inspect size issue - ([566231d](https://github.com/commit/566231d55b88358a6003fecb1f5bc070b35c6457)) - Tyr Chen
- update deps - ([680d493](https://github.com/commit/680d4931cab2abc32c8fcf98723939b39afb7eea)) - Tyr Chen
- bump version to 0.7.1 (#7) - ([5b85a1e](https://github.com/commit/5b85a1e2e91c4d6737884cee2983b11cca87d55d)) - Tyr Chen

### Other

- [codex] Add repoctl doctor agent handoff (#3)

## What changed

- Added `repoctl doctor` with `--agent` output that aggregates read-only
repo health sections and prints only actionable diagnostics for agent
hand-off.
- Added `repoctl skills diff` and skill drift diff payloads, plus
`skills sync --dry-run` parity through the shared skills report.
- Updated checked-in and generated root `AGENTS.md` / `CLAUDE.md`
guidance so routine repoctl verification uses `repoctl doctor --agent`.
- Wrote the v0.6 doctor/agent handoff spec under `specs/` and linked it
from the spec index.
- Fixed tesseract-observed usability gaps: changed-file inspect scope
default, unsupported local template schema diagnostics, root `.git`
hygiene exclusion, provider capability empty-state diagnostics, and
proto empty-state diagnostics.
- Bumped the workspace and internal crate versions to `0.6.0`.

## Why

The tesseract verification pass showed that repoctl had the right
primitives, but agents still had to run and interpret a noisy command
matrix. `doctor --agent` gives agents one final read-only repoctl
hand-off command while preserving deeper subcommands for targeted
investigation.

## Validation

- `cargo +nightly fmt`
- `cargo build`
- `cargo test`
- `cargo clippy -- -D warnings`
- `make check-agent-sync`
- `git diff --check`
- `/Users/tchen/.target/debug/repoctl --version` -> `repoctl 0.6.0`

Manual tesseract checks against
`/Users/tchen/projects/cellis/tesseract`:

- `repoctl doctor --agent --repo /Users/tchen/projects/cellis/tesseract
--base origin/main --head HEAD`
- `repoctl doctor --agent --repo /Users/tchen/projects/cellis/tesseract
--changed-file frameworks/tesseract-infra/src/stack.ts`
- `repoctl skills diff --repo /Users/tchen/projects/cellis/tesseract
--format human`
- `repoctl inspect size --repo /Users/tchen/projects/cellis/tesseract
--changed-file frameworks/tesseract-infra/src/stack.ts --format json`
- `repoctl template render local:templates/tesseract-api --repo
/Users/tchen/projects/cellis/tesseract --dry-run`
- `repoctl hygiene check --repo /Users/tchen/projects/cellis/tesseract`
- `repoctl provider capabilities --repo
/Users/tchen/projects/cellis/tesseract --workspace apps.todomvc:iac`
- `repoctl provider capabilities --repo
/Users/tchen/projects/cellis/tesseract`
- `repoctl proto owners cellis.auth.v1 --repo
/Users/tchen/projects/cellis/tesseract`
- `repoctl template list --repo /Users/tchen/projects/cellis/tesseract`

Expected tesseract diagnostics remain visible: `apps.todomvc` depends on
framework internals, generated skills are out of sync, generated
artifact directories exist, and the repo has no declared proto packages. - ([ec2fe23](https://github.com/commit/ec2fe23f601c2e19ec0f97b4ce861d3532215505)) - Tyr Chen
- [codex] Parallelize repoctl doctor sections (#4)

## What changed

This PR includes the existing local `Improve doctor agent diagnostics`
commit plus a new `Parallelize doctor sections` commit.

The new commit changes `Repoctl::doctor` to run independent doctor
sections concurrently with `std::thread::scope`, while preserving the
original output order. It also converts a worker panic into a blocked
doctor section instead of letting the whole doctor call unwind, and adds
a test assertion that the reported section order remains stable.

## Why

`repoctl doctor` previously ran graph, affected, skills, hygiene,
codegen, inspect-size, proto, and iac sections sequentially. Several
sections repeat repo discovery and changed-file work, so the command's
wall-clock time was bounded by the sum of independent checks. Running
the sections in parallel should reduce latency without changing the
public request/report API.

A deeper optimization is still available later: sharing one discovered
snapshot and changed-file list across sections would remove duplicated
work inside runner/inspector calls, but that requires broader facade API
changes.

## Validation

- `cargo +nightly fmt --all --check`
- `cargo test -p repoctl doctor`
- `cargo build`
- `cargo test`
- `cargo clippy -- -D warnings`

Skipped repoctl hand-off gates:

- `repoctl doctor --agent` could not run in this checkout because no
`repo.yaml` is present at the repository root.
- `repoctl inspect size --scope changed --base origin/master --head HEAD
--fail-on warning` failed for the same missing `repo.yaml` reason. - ([78ebb6b](https://github.com/commit/78ebb6b534fabdfa7b985af2d79203fccb24bb2d)) - Tyr Chen
- Fix ordered release publishing - ([b6a63bf](https://github.com/commit/b6a63bfc7150a2dd822898b3c266f225cb170a2d)) - Tyr Chen
- Build reliable multilingual governance with Nx-style tasks and Buf checks (#5)

repoctl previously planned checks without reliably enforcing
prerequisite completion, bounded execution, complete change propagation
or standalone CI execution. This change makes its multilingual graph
drive dependable local tasks, CI and agent context, with Nx-inspired
explicit cache contracts and Buf-owned compatibility rules.

- Add dependency-aware task scheduling, bounded argv execution and
opt-in local BLAKE3 artifact caching. Input scans cap every visited
entry and depth; disabled cache outputs and explicit inputs inside other
output trees remain fingerprinted. Sequential commands hash preceding
artifact fingerprints; cached descendants bypass when prior commands or
prerequisites lack complete fingerprints; distinct task prerequisites
are capped before expansion.
- Correct merge-base, rename and local Git selection; treat pinned root
toolchains and root Cargo/pnpm workspace configuration and
Make/Just/Task routers as repo-wide inputs; propagate all dependency
surfaces with evidence and transitive reviewers, selecting sibling
consumers under the project-scoped graph model. Offline discovery covers
virtual Cargo members, inherited and ordinary-package path dependency
closures, Node packages and Python/Poetry groups, with per-graph
manifest reuse and deepest-owner public/internal classification for
nested projects.
- Generate executable CI matrices for selected tasks, preserving
prerequisite execution and toolchain setup. Detect protoc across
member/local dependency build scripts; independently install the pinned
CLI and enforce a required gate. Execute real Buf lint and breaking
checks.
- Adopt arbitrary Cargo/Node/Python and README-only documentation
sources with nearest lock ownership, option-preserving dependency
rewrites, explicit lock refreshes and verification enforcement. Preserve
legacy binary Bun locks or explicitly regenerate text locks after
rewrites. Cargo manifests resolve to actual workspace/lock owners and
produce one task group and full verification with existing locks
enforced per root; standalone nested packages remain separate; workspace
names are unique across languages. Unlocked Node packages honor the
nearest declared packageManager. Original source digests bind saved
plans to reviewed bytes and are preflighted before writes. Saved-plan
bytes and all collections are bounded at the shared apply/verify
boundary, with generated plans meeting the same limits. Canonical roots
and shared traversal budgets prevent recursive destination imports and
unbounded planning; preflight rejects duplicate project identities and
exclusive creation protects project files. Saved report-only CI previews
never write workflows.
- Add source/document context with a shared total documentation
traversal budget and repository-relative exclusions, escaped
deterministic Mermaid diagrams, bilingual guides, research and the
G1–G11 specification and verification contract under `specs/`.

Validation: `make build`, `cargo test --workspace` (150 regular tests +
9 doctests), `make test` (150 passes; real-Buf case separately
exercised), nightly format check, strict pedantic clippy, API docs,
Markdown links/indexes and diff checks pass. Supply-chain and agent-sync
checks passed for the original implementation; the latest review reuses
the existing BLAKE3 dependency and reruns audit/deny after the facade
dependency-list change. Buf 1.73.0 detects actual field removal with
directory and Git baselines; hosted Linux CI exercises real Buf.
Regression tests cover review findings, including saved-plan
application, binary locks, cross-source budgets, cache eligibility and
virtual workspaces. A real offline npm refresh also passes.

Release/maintenance boundaries: Rust API additions require 0.7.0 and
generated workflows pin it; publish the ordered crate chain before
external adoption. This PR performs no release or deployment. Timeout
cleanup covers the direct child rather than portable process-tree
isolation. This source repo has no `repo.yaml`, so doctor/size
verification uses temporary managed fixtures. Doctor and the latest
seven changed Rust files pass without findings. The previous scoped
check retained only the existing core/domain.rs file-size warning after
a documentation-only change; its other 13 changed Rust files were clean.
All-source inspection retains 13 existing size warnings (master has 14);
no thresholds were relaxed. See
`specs/repoctl-reliable-governance-verification-plan.md` for acceptance
evidence and skipped unrelated gates. - ([880c73c](https://github.com/commit/880c73ce4cbc8cd172acdedf3ad2fa3c66581ba4)) - Tyr Chen

---
## [0.5.1](https://github.com/compare/v0.4.0..v0.5.1) - 2026-06-07

### Miscellaneous Chores

- update docs - ([2b69bcb](https://github.com/commit/2b69bcbed342b3d12cdb15bf8a9db7f375e14b50)) - Tyr Chen

### Other

- [codex] Implement repoctl v0.5 code size inspection (#2)

## Summary

- Adds repoctl v0.5 code-size inspection specs and dependency research.
- Adds `repoctl-inspect` with syntax-aware Rust, TypeScript/TSX, and
Python scanning for oversized files, functions, and nested blocks.
- Adds repo manifest `inspection.code_size` configuration, CLI `repoctl
inspect size`, JSON/human reports, PR summary integration, and generated
CI workflow inspection step.
- Bumps workspace and internal crate versions to 0.5.0.

## Validation

- `cargo +nightly fmt --check`
- `cargo build`
- `cargo test`
- `cargo clippy -- -D warnings -W clippy::pedantic`
- `cargo audit`
- `cargo deny check` (passes; existing unmatched-license allowance
warnings only)
- `cargo install --path apps/repoctl-cli --locked --force` and `repoctl
--version` -> 0.5.0
- `repoctl inspect size --repo ~/projects/cellis/tesseract --scope all
--format human` -> 16 findings across 12 files
- `repoctl inspect size --repo ~/projects/cellis/universe --scope all
--format human` -> 24 findings across 17 files, plus 2 syntax
diagnostics in TSX files

## Notes

- A code-review subagent reviewed the implementation; valid findings
were fixed before this PR was created. - ([a54f37e](https://github.com/commit/a54f37ebca8965f234d2080e667c7114ac7da784)) - Tyr Chen
- Release repoctl 0.5.1 - ([efa8bbf](https://github.com/commit/efa8bbf2e306f0ebde274326341be872dfbb2795)) - Tyr Chen

---
## [0.4.0] - 2026-06-07

### Bug Fixes

- fix init target directory - ([df3c4fd](https://github.com/commit/df3c4fd47d8e633babef6bcbe43eb2a44aa22fbd)) - Tyr Chen

### Documentation

- expand README and guides - ([0220f64](https://github.com/commit/0220f643af986856b69992d90a2584be74f3f8c7)) - Tyr Chen

### Features

- generate usable monorepo scaffolds - ([a420d82](https://github.com/commit/a420d82fba0779ac6ca1abae3bf91301ef1540a5)) - Tyr Chen
- generate functional agent skills - ([6f43a00](https://github.com/commit/6f43a007d38a16127d09db041bcb2f0d0c9414fb)) - Tyr Chen

### Miscellaneous Chores

- int the project - ([0b08b72](https://github.com/commit/0b08b7262388bcfe5dd35d88a9d0ecc14ecdbe7d)) - Tyr Chen
- add design/impl/verification specs - ([eaa8040](https://github.com/commit/eaa8040648d91babba146b727aee9ef3e1ca068f)) - Tyr Chen
- support gitignore - ([d196664](https://github.com/commit/d19666482b29b4d58aa4fb0ffcaf036c7d199637)) - Tyr Chen
- make it ready for publish - ([cb131a8](https://github.com/commit/cb131a82ff1cb790b4ded6f2beaf97575dcdacd4)) - Tyr Chen

### Other

- Implement repoctl phase 0-3 baseline - ([e125baf](https://github.com/commit/e125bafadfa58060d9de4772edc4052e5651a6f6)) - Tyr Chen
- Implement repoctl phase 4-6 workflows - ([3485f4d](https://github.com/commit/3485f4dd484c4e48018aad044f2eebe75221f266)) - Tyr Chen
- Complete repoctl v0.2 command surface - ([b1d5e07](https://github.com/commit/b1d5e07653fa6f937ea93aa3392a85133c52bd35)) - Tyr Chen
- Add interactive new project wizard - ([bcc96d3](https://github.com/commit/bcc96d38d75168259a95d976db6bec15990ff6ea)) - Tyr Chen
- install rustfmt and clippy components - ([49a88f5](https://github.com/commit/49a88f598b8e2987aeb1398ce50bf4e4f7426abf)) - Tyr Chen
- install components for pinned toolchain - ([eee0983](https://github.com/commit/eee0983797c4a7b8c2d66f300901eed8c6f3a69c)) - Tyr Chen
- add rust components after toolchain install - ([708cdca](https://github.com/commit/708cdcacd9dedff46100ebba1e42bdef8358eaae)) - Tyr Chen
- Implement repoctl v0.3 adoption workflow - ([9b6fea5](https://github.com/commit/9b6fea5120cd07984bdfbb604de2ed83c309e2b1)) - Tian Chen
- Add operations session planning - ([cef2c08](https://github.com/commit/cef2c0886236660ac4a40111d90c5cd80fe33473)) - Tyr Chen
- Bump version to 0.4.0 - ([440c174](https://github.com/commit/440c1744cd2c7e8b0ec810e9140bc27070e1d8d2)) - Tyr Chen
- Merge pull request #1 from tyrchen/feat/v0.4-ops-session-review

[codex] Add operations session planning - ([267fae6](https://github.com/commit/267fae6d38ad4d2eb63b8872ec701d30ca8340a0)) - Tyr Chen

### Tests

- allow sync fixture filesystem setup - ([99c50fe](https://github.com/commit/99c50fec769af7fd8dd277decbfe8000e526c3fe)) - Tyr Chen

<!-- generated by git-cliff -->
