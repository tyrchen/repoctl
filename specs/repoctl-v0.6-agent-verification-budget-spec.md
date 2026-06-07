# repoctl v0.6 Agent Verification Budget Spec

## Status

Draft for implementation.

## Problem

repoctl-generated agent instructions currently encourage agents to run repoctl throughout a coding
session: graph validation before work, affected analysis, per-task dry-runs, task execution, then
more validation after small edits. In practice this turns repoctl into the inner loop instead of the
routing and hand-off authority.

The desired behavior is a large reduction in repoctl calls during agent sessions. Agents should
focus on reading and changing code, then run the smallest meaningful repoctl verification once when
the current todo batch or goal is complete.

## Goals

- Reduce routine repoctl calls in agent sessions by roughly 90%.
- Keep repoctl authoritative for graph, ownership, affected analysis, and task routing.
- Move verification from the inner coding loop to the final hand-off stage.
- Prevent generated skill text from reintroducing mandatory command sequences.
- Keep high-risk structural changes protected by graph and boundary checks.

## Non-Goals

- Do not weaken CI requirements.
- Do not remove repoctl validation from structural, proto, IaC, CI, template, skill, or graph changes.
- Do not replace repoctl affected analysis when the user asks for impact, CI routing, or PR readiness.

## Required Agent Policy

Agent-facing docs generated or maintained by repoctl must use this policy:

- repoctl commands are reference tools, not a mandatory sequence.
- Agents must not run per-project/per-task dry-run matrices.
- Agents should use at most one dry-run when task selection is unclear or expensive.
- Agents should not recompute affected projects after source-only edits, formatting, lint fixes, or
  test fixes.
- Agents should run repoctl validation once at the end of a todo batch or goal.
- Agents should run `repoctl graph validate` before editing only when graph inputs may change:
  `repo.yaml`, `project.yaml`, workspaces, task wiring, generated-code policy, proto ownership, CI
  routing, templates, skills, or cross-project dependency boundaries.
- Agents should run `repoctl skills check` only when agent instructions, skills, skill sources, or
  sync behavior changed.
- If `repoctl affected` is widened by unrelated branch changes, agents should state that and switch
  to explicit project-scoped verification.

## Skill Generation Requirements

repoctl skill generation must stop emitting language that says the skill is generated and must stay
synchronized with `repoctl skills sync`. That language makes local tuning look temporary and causes
agents to treat generated defaults as higher priority than repository policy.

Generated or scaffolded skill text should instead describe the skill as repository policy and should
emphasize scoped final verification.

### repoctl Skill

The repoctl skill workflow must be choice-based, not a linear checklist.

Required behavior:

- Explain that agents should choose the smallest repoctl surface that answers the question.
- Put `repoctl graph validate` behind graph-input changes or final hand-off.
- Put `repoctl affected` behind impact or verification selection and require one call per todo batch.
- Put `repoctl inspect size` at hand-off for relevant source changes.
- Limit dry-run guidance to one affected dry-run when task selection is unclear.
- Explicitly prohibit per-project/per-task dry-run matrices.

### app-creation Skill

The app-creation skill should scaffold first and verify once after the project is complete.

Required behavior:

- Keep `repoctl graph validate` as a final scaffold validation.
- Replace check/test dry-run examples with a real project check.
- Run test/build only when the scaffold includes tests, generated clients, package metadata, build
  outputs, deploy artifacts, or task wiring that need verification.
- Dry-run only when task routing itself is unclear.

### monorepo-boundaries Skill

The boundaries skill should use repoctl only for unclear ownership or final boundary verification.

Required behavior:

- Do not pair every boundary check with graph validation.
- Run `repoctl graph validate` only when manifests, workspaces, task wiring, generated-code policy,
  or cross-project dependencies changed.
- Run affected analysis once at hand-off.

### proto-change Skill

The proto skill should keep owner and consumer discovery, then defer broad verification.

Required behavior:

- Keep `repoctl proto owners` and `repoctl proto consumers` for ownership discovery.
- Run `repoctl proto check` at hand-off.
- Run affected analysis once at hand-off to select consumer verification.
- Do not recompute affected after source-only proto fixes.

### pr-review Skill

The PR review skill may still use repoctl impact commands, but each command should run once unless
graph or task-routing inputs change during review.

Required behavior:

- Graph validation is optional for source-only PRs.
- Affected analysis should not be repeated to work around unrelated branch-wide changes.
- Code review findings remain primary; repoctl scopes review and verification.

## Root Agent Docs Requirements

Root `AGENTS.md` and `CLAUDE.md` should include a repoctl verification budget:

- Keep repoctl out of the inner coding loop.
- Validate once at todo-batch or goal completion.
- Treat common repoctl commands as reference commands, not a required sequence.
- Explain when `repoctl skills check` is relevant.
- Ask agents to report skipped heavyweight gates and why they were not relevant.

Project-local `AGENTS.md` and `CLAUDE.md` generated by repoctl should avoid generated markers and
should say project checks run once at hand-off, not after every code change.

## Acceptance Criteria

- Generated repoctl skills no longer include `@generated by repoctl`.
- Generated repoctl skills no longer say they must be kept synchronized with `repoctl skills sync`.
- Generated agent instructions do not contain per-project/per-task dry-run matrices.
- Root agent docs distinguish reference commands from mandatory command sequences.
- Project-local agent docs say repoctl project checks run once at hand-off.
- A typical single-project source edit requires zero repoctl calls before editing and one scoped
  verification pass before hand-off.
- A structural change still requires graph validation and relevant affected or boundary checks.

## Verification Plan

Use fixture-generated docs and skills to assert text-level behavior:

- No generated marker appears in generated skill or project-local agent docs.
- `repoctl run check --project <project-name> --dry-run` does not appear in app-creation guidance.
- `repoctl run test --project <project-name> --dry-run` does not appear in app-creation guidance.
- `repoctl run test --affected --dry-run` does not appear in repoctl skill guidance.
- The phrase `once at hand-off` or equivalent appears in root docs and project-local docs.

Use behavioral review against sample tasks:

- Source-only one-project edit: expected agent flow is read/edit/local language command if needed,
  then one final scoped repoctl check.
- Multi-project manifest edit: expected agent flow includes graph validation and final affected
  analysis.
- App scaffold: expected agent flow includes scaffold, source review, one graph validation, and one
  project check; test/build are risk-based.

