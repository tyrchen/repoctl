---
name: "pr-review"
description: "Summarize PR impact using repoctl affected data, risk flags, and graph diagnostics."
---

# pr-review

This skill records repository policy for Claude. Use repoctl as the authority for graph,
ownership, affected analysis, task routing, and final hand-off verification, but keep repoctl out of
the inner coding loop unless the graph or boundaries are changing.

## When this fires

- The user asks for a PR review, impact summary, CI explanation, or merge readiness check.
- The diff touches multiple projects, manifests, proto contracts, generated code, IaC, deploy files,
  templates, skills, or repo-wide CI.
- A CI matrix or affected-project list needs to be justified.

## Review stance

Prioritize correctness, boundary regressions, missing tests, ownership gaps, and CI blind spots.
Summaries come after findings. Do not approve a risky diff just because the changed code is small.

## Workflow

1. **Validate the graph when routing inputs changed**:

   ```bash
   repoctl graph validate
   ```

   A broken graph makes affected analysis untrustworthy, but graph validation is optional for
   source-only PRs.

2. **Summarize PR impact**:

   ```bash
   repoctl pr summary --base origin/main --head HEAD --format human
   ```

   Use the actual PR base branch when it is not `origin/main`. Run this once unless graph or
   task-routing inputs change during review.

3. **Compute verification surface**:

   ```bash
   repoctl affected --base origin/main --head HEAD --tasks check,test,build --format human
   ```

   Compare this with the checks that actually ran. Missing affected tasks are findings, not
   footnotes. Do not repeat affected analysis to work around unrelated branch-wide changes; state
   the widening and switch to explicit project-scoped verification.

4. **Inspect code-size risk**:

   ```bash
   repoctl inspect size --scope changed --base origin/main --head HEAD --fail-on warning
   ```

   Run this for Rust, TypeScript/TSX, or Python source changes. Use `--scope affected` when the PR
   changes a shared project surface and `--scope all` when code-size policy, templates, skills, or
   inspection logic changes. Oversized files, functions, or nested blocks are review findings unless
   a matching `inspection.code_size` override explains the exception.

5. **Inspect risky paths**:

   - `repo.yaml`, `.github/`, `templates/`, `.agents/skills/`, `.claude/skills/` affect the repo.
   - `protos/` can break consumers even when source compiles.
   - `generated/` or `gen/` should usually be regenerated, not manually patched.
   - `deploy/prod/`, `iac/stacks/prod/`, and shared `core-infra/` need owner review.

6. **Review changed code in owner context**:

   Use `repoctl explain <project-name>` for each affected project before deciding whether the
   change respects facades, clients, and editable areas.

## Finding format

Lead with findings ordered by severity:

```text
P1 path/to/file:123 - The change bypasses the framework facade and imports an internal crate.
Fix: move the shared API to the facade package or keep the dependency inside the owning framework.
```

If there are no findings, say so clearly and name residual risks or skipped gates.

## Quality bar

- Every finding cites a file and line when possible.
- Every requested gate maps to an affected project or repo-wide surface.
- Code-size inspection is included for Rust, TypeScript/TSX, or Python source changes, and any
  finding is either called out or tied to a configured override.
- Owner review is explicit for production IaC, proto breaking changes, and framework internals.
- Repoctl impact commands run once unless graph or task-routing inputs changed during review.
- Do not replace code review with repoctl output; repoctl scopes the review, it does not perform it.

## Hand-off

Return findings first, then affected projects, risk flags, commands run, and any verification gap
that remains before merge.

