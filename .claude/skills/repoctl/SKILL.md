---
name: "repoctl"
description: "Use repoctl as the source of truth for monorepo graph, affected, CI, context, and task workflows."
---

# repoctl

This skill records repository policy for Claude. Use repoctl as the authority for graph,
ownership, affected analysis, task routing, and final hand-off verification, but keep repoctl out of
the inner coding loop unless the graph or boundaries are changing.

## When this fires

- The user asks which projects are affected, which checks to run, or why CI picked a matrix.
- A change touches `repo.yaml`, `project.yaml`, workspaces, task wiring, generated-code policy,
  proto ownership, CI routing, templates, skills, or cross-project dependencies.
- A todo batch or goal is complete and needs final repoctl-scoped verification.
- Before using broad package-manager commands in a large repo when project routing is unclear.

If the repository has no `repo.yaml`, do not guess the graph. Run `repoctl init --dry-run` only when
the user is asking to adopt repoctl; otherwise inspect the existing repo layout manually.

## Verification budget

- Choose the smallest repoctl surface that answers the current question.
- Keep repoctl out of the inner coding loop for source-only edits, formatting, lint fixes, and test
  fixes.
- Run repoctl validation once at the end of a todo batch or goal.
- Do not recompute affected projects after source-only edits unless task-routing inputs changed.
- Use at most one affected dry-run when task selection is unclear or expensive.
- Do not run per-project/per-task dry-run matrices.
- Run `repoctl skills check` only when agent instructions, skills, skill sources, or sync behavior
  changed.
- If unrelated branch changes widen `repoctl affected`, state that and switch to explicit
  project-scoped verification.

## What to produce

- A short statement of the relevant project(s), owners, and changed surfaces.
- The exact repoctl commands used, with `--format json` when the result feeds automation.
- A scoped final verification recommendation; broader gates belong to shared policy, templates,
  graph code, or structural changes.
- Any heavyweight gate skipped and why it was not relevant.

## Reference Commands

Treat these as reference commands, not a mandatory sequence.

- **Validate graph inputs**:

   ```bash
   repoctl graph validate --format human
   ```

  Run before editing only when graph inputs may change: `repo.yaml`, `project.yaml`, workspaces,
  task wiring, generated-code policy, proto ownership, CI routing, templates, skills, or
  cross-project dependency boundaries. Also run once at hand-off for structural changes.

- **Explain an owned project**:

   ```bash
   repoctl explain <project-name>
   ```

  Use before changing a project manifest, dependency, task, proto ownership, IaC root, deploy
  environment, or AI editable area.

- **Compute impact or select verification**:

   ```bash
   repoctl affected --base origin/main --head HEAD --tasks check,test,build --format human
   ```

  Run once per todo batch when impact, CI routing, PR readiness, or verification selection matters.
  Use the merge-base or PR base that matches the review target. If `origin/main` is not the target
  branch, name the actual base explicitly in the hand-off.

- **Inspect changed code shape at hand-off**:

   ```bash
   repoctl inspect size --scope changed --base origin/main --head HEAD --fail-on warning
   ```

  Run this for Rust, TypeScript/TSX, or Python source changes before review. Use `--scope affected`
  when a change fans out across an owned project, and `--scope all` when shared thresholds,
  excludes, templates, skills, or graph/inspection logic change. Treat oversized files, functions,
  and nested blocks as refactoring findings unless an explicit `inspection.code_size` override with
  a concrete reason applies.

- **Dry-run only when routing is unclear**:

   ```bash
   repoctl run check --affected --dry-run
   ```

  Use no more than one affected dry-run to confirm an expensive or ambiguous task selection. Do not
  expand this into a per-project/per-task dry-run matrix.

- **Generate agent context only when it helps**:

   ```bash
   repoctl context <project-name> --format json
   ```

  Use context packs for multi-file edits or unfamiliar ownership. Do not treat context as a
  replacement for reading the source files that will be changed.

## Quality bar

- Do not hand-edit generated state to silence repoctl diagnostics.
- Do not substitute package-manager commands for repoctl affected analysis when the question is impact.
- Do not skip `repoctl inspect size` for Rust, TypeScript/TSX, or Python source changes; explain any
  code-size findings, configured overrides, or intentionally broader scope in the hand-off.
- Do broaden to repo-wide checks when `repo.yaml`, templates, skills, root CI, or graph validation
  logic changes.
- Do not run per-project/per-task dry-run matrices.
- Final responses should name skipped heavyweight gates and why they were not relevant.

## Hand-off

Report the graph status when relevant, affected projects, selected commands, and any owner or
boundary concerns. If diagnostics remain, stop and show the concrete diagnostic rather than claiming
the repo is ready.

