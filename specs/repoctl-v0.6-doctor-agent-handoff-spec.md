# repoctl v0.6 Doctor And Agent Handoff Spec

## Status

Accepted for implementation on the v0.6 agent verification budget branch.

## Problem

Real use against `~/projects/cellis/tesseract` showed that repoctl has enough primitives to inspect
a functional monorepo, but agents must still hand-assemble a large command matrix to understand
repository health. The current experience has three problems:

- high signal is mixed with verbose routing output;
- repo diagnostics and CLI defects both look like command failures;
- generated agent docs still teach several reference commands instead of one final verification
  entry point.

The tesseract run also exposed smaller usability gaps: local non-repoctl templates fail with a YAML
schema error, `skills check` does not show the expected diff, `inspect size --changed-file` expands
too broadly, hygiene reports root `.git` as generated leakage, provider capability reports can be
empty without explanation, and proto lookup against a repo with no declared proto packages returns
an empty success with no hint.

## Goals

- Add `repoctl doctor` as the single read-only repository health command for humans and agents.
- Add `repoctl doctor --agent` to emit only errors and warnings that require modification.
- Update root and generated agent docs so routine repoctl verification uses only `repoctl doctor`.
- Make doctor distinguish repo diagnostics from repoctl execution failures.
- Add a skills diff/preview path so generated skill drift is reviewable.
- Improve tesseract-observed diagnostics for templates, hygiene, provider capabilities, proto, and
  code-size changed-file scans.
- Bump repoctl to `0.6.0`.

## Non-Goals

- Do not run mutating tasks from `doctor`.
- Do not make `doctor` replace CI gates; it selects and summarizes local verification health.
- Do not make repoctl render arbitrary third-party template schemas.
- Do not fix tesseract's repo policy violations in this repoctl change.

## Required Behavior

### `repoctl doctor`

Add a top-level command:

```bash
repoctl doctor --repo . --base origin/main --head HEAD
repoctl doctor --repo . --changed-file path/to/file --agent
```

Inputs:

- `--repo`: repository root or path inside repo.
- `--base`, `--head`: changed-file detection refs.
- `--changed-file`: explicit changed files, repeatable.
- `--tasks`: comma-separated task names, default `check,test,build`.
- `--agent`: compact output for agents.
- `--format`: `human`, `json`, or `github-actions`.

Doctor must be read-only. It may run graph validation, affected analysis, skills check, hygiene
check, codegen check, code-size inspection, proto check, IaC planning in dry-run mode, provider
capability inspection, and ops planning. It must not run project tasks, sync skills, write workflows,
render templates into the repo, clean generated artifacts, or apply operations plans.

Human output should provide a short summary by section. Agent output should only print diagnostics
that require edits, grouped by section. If there are no diagnostics requiring edits, agent output
should say the repository is doctor-clean.

JSON output must include a stable status model:

```json
{
  "status": "ok|diagnostics|blocked",
  "commandSucceeded": true,
  "hasErrors": false,
  "sections": []
}
```

`blocked` is reserved for repoctl execution failures that prevent a section from running. Repo
policy errors, skill drift, hygiene warnings, and code-size findings are `diagnostics`.

### Agent Docs

Generated root `AGENTS.md` and `CLAUDE.md`, plus this repository's checked-in root files, should
route routine final verification through:

```bash
repoctl doctor --agent
```

They may still mention specialized repoctl commands in skills and detailed docs, but root agent docs
must not present a multi-command final verification sequence.

### Skills Diff

Add:

```bash
repoctl skills diff --repo .
repoctl skills sync --dry-run
```

`skills diff` should show changed generated skill files and a compact line-level diff. JSON output
should include changed paths and expected/current snippets or diff text. `skills sync --dry-run`
should render the same drift diagnostics without writing files.

### Code-Size Changed Files

When `repoctl inspect size --changed-file ...` is used without an explicit `--scope`, the effective
scope should be changed files, not all files. Output must report the effective scope.

### Template Diagnostics

When a local template exists but its schema is not `repoctl.template/v1`, report an explicit
diagnostic such as:

```text
template schema cellis.tesseract/template/v1 is not renderable by repoctl
```

`template list` should continue listing repoctl-renderable templates and should not crash on
non-repoctl template metadata.

### Hygiene

Hygiene checks should not report the repository root `.git` directory as generated leakage. Nested
`.git` directories remain diagnostics. Human output should aggregate common generated directories
clearly enough to scan.

### Provider Capabilities

If no provider capabilities are found, return a warning diagnostic explaining the likely reason:
unknown provider metadata, unsupported workspace, or no recognized provider package.

### Proto Empty State

If a proto owner or consumer lookup returns no matches and the repo has no declared proto packages,
return a warning diagnostic explaining that no proto packages are declared. `proto check` may still
surface graph diagnostics, but proto empty state should be explicit.

## Acceptance Criteria

- `repoctl doctor --agent --repo ~/projects/cellis/tesseract --base origin/main --head HEAD`
  prints only actionable diagnostics and no full affected-task matrix.
- Root `AGENTS.md` and `CLAUDE.md` contain `repoctl doctor --agent` as the routine repoctl
  verification command.
- Generated root agent docs contain the same doctor command.
- `repoctl skills diff --repo ~/projects/cellis/tesseract` reports skill drift without writing.
- `repoctl inspect size --repo ~/projects/cellis/tesseract --changed-file frameworks/tesseract-infra/src/stack.ts --format json`
  reports changed scope and scans only the explicit changed-file surface.
- `repoctl template render local:templates/tesseract-api --repo ~/projects/cellis/tesseract --dry-run`
  reports an unsupported template schema diagnostic rather than an unknown-field YAML parse error.
- `repoctl hygiene check --repo ~/projects/cellis/tesseract` does not report root `.git`.
- `repoctl provider capabilities --repo ~/projects/cellis/tesseract --workspace apps.todomvc:iac`
  explains why no capabilities were found.
- `repoctl proto owners cellis.auth.v1 --repo ~/projects/cellis/tesseract` warns when no proto
  packages are declared.
- Version reports `0.6.0`.

## Verification Plan

- Unit tests for doctor status aggregation, skill diff detection, changed-file inspect scope, root
  `.git` hygiene exclusion, unsupported template schema diagnostics, provider empty diagnostics, and
  proto empty-state diagnostics.
- Full Rust gates: `cargo build`, `cargo test`, `cargo +nightly fmt`, and
  `cargo clippy -- -D warnings`.
- Agent sync gate: `make check-agent-sync`.
- Manual tesseract checks for the acceptance criteria above.
