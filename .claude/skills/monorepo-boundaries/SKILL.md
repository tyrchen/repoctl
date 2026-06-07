---
name: "monorepo-boundaries"
description: "Respect app, framework, foundation, proto, generated-code, and IaC boundaries."
---

# monorepo-boundaries

This skill records repository policy for Claude. Use repoctl as the authority for graph,
ownership, affected analysis, task routing, and final hand-off verification, but keep repoctl out of
the inner coding loop unless the graph or boundaries are changing.

## When this fires

- A change touches imports, package manifests, workspace membership, generated code, proto ownership,
  IaC, deployment config, or multiple project roots.
- The user asks whether code should live in an app, framework, foundation service, proto root, or
  core infrastructure.
- A review includes app-to-app imports, framework internals, source proto edits, or production IaC.

## Boundary model

- Apps may depend on framework facades and foundation clients, not on other apps.
- Framework internals are private. Consumers use the facade paths declared in `project.yaml`.
- Foundation services expose public clients and owned contracts; business apps do not import service internals.
- Source protos live under `protos/`. Generated code is consumer-local output.
- App IaC is colocated under the app; shared infrastructure lives under `core-infra/`.
- Generated files are build artifacts. Source changes happen in source roots, templates, or proto
  definitions.

## Workflow

1. **Classify the changed path**:

   - `apps/<name>/` - product or domain application.
   - `frameworks/<name>/` - shared capability with public facade and private internals.
   - `foundations/<name>/` - platform service plus public clients.
   - `protos/` - source contracts owned by declared packages.
   - `core-infra/` - shared infrastructure.
   - `.agents/skills/`, `.claude/skills/`, `templates/`, `.github/` - repo-wide policy or tooling.

2. **Read the nearest manifest when ownership is unclear**:

   ```bash
   repoctl explain <project-name>
   ```

   If a path has no owning project and is not intentionally repo-wide, fix ownership before adding
   more files.

3. **Check dependency direction**:

   ```bash
   repoctl lint-boundaries --changed-file <path>
   ```

   Apps can call framework facades and foundation clients. Framework internals and service internals
   are not public API unless declared as facades or clients. Run `repoctl graph validate` before
   editing only when manifests, workspaces, task wiring, generated-code policy, or cross-project
   dependencies changed.

4. **Choose the right home for new code**:

   - Put product-specific behavior in an app.
   - Put reusable business capability behind a framework facade.
   - Put shared runtime/platform service ownership in a foundation service.
   - Put cross-cutting infrastructure in `core-infra/`.
   - Put contracts in `protos/` and generated outputs under consumer workspaces.

5. **Verify boundaries once at hand-off**:

   ```bash
   repoctl affected --base origin/main --head HEAD --tasks check,test
   ```

   Run affected analysis once at hand-off to select the final verification surface. Do not pair
   every boundary check with graph validation.

## Review checklist

- No app imports another app's internals.
- No app imports a framework internal package directly.
- No generated file is edited as the source of truth.
- Production IaC and deploy files have the owning team called out.
- Manifest changes include owners, tasks, workspaces, and AI editable/do-not-edit areas where
  relevant.
- Graph validation was run for structural boundary inputs, or the hand-off explains why the change
  was source-only.

## Hand-off

Name the owning project, why the chosen directory is correct, which boundary checks ran, and any
owner review required before merge.

