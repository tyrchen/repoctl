---
name: "app-creation"
description: "Create new apps, frameworks, and foundation services with repoctl templates and manifests."
---

# app-creation

This skill records repository policy for Codex. Use repoctl as the authority for graph,
ownership, affected analysis, task routing, and final hand-off verification, but keep repoctl out of
the inner coding loop unless the graph or boundaries are changing.

## When this fires

- The user asks for a new service, app, framework, shared library, client, worker, or foundation
  capability inside the monorepo.
- A new directory under `apps/`, `frameworks/`, or `foundations/` is needed.
- The user is tempted to copy an existing project by hand.

## Choose the project kind

- **App**: product/domain behavior deployed independently. It may use framework facades and
  foundation clients.
- **Framework**: reusable capability for apps. Public facade is stable; internals stay private.
- **Foundation service**: platform-owned service with public clients and optional proto ownership.

If the requested code is shared but has no runtime service, prefer a framework. If it owns a
contract or platform runtime, prefer a foundation service.

## Workflow

1. **Pick the scaffold command**:

   ```bash
   repoctl new app apps/<name> --stack rust-api,bun-web
   repoctl new framework frameworks/<name> --languages rust,typescript
   repoctl new foundation foundations/<name> --clients rust,typescript,python --proto <package>
   ```

   Include `--iac pulumi`, `--iac terraform`, or `--iac opentofu` only when the project actually
   deploys infrastructure.

2. **Review generated metadata before adding domain code**:

   - `project.yaml`: name, kind, owners, workspaces, tasks, proto ownership, IaC, deploy roots.
   - `README.md`: local commands and boundary notes.
   - `AGENTS.md` / `AGENTS.md`: project-local edit constraints.
   - source files: runnable starter code and tests, not placeholders.

3. **Make the generated project immediately useful**:

   Replace example names with domain language, keep the task names (`check`, `test`, `build`) stable,
   and add missing owner handles. Do not remove privacy settings such as `publish = false` or
   `"private": true` unless the repo policy explicitly permits publishing.

4. **Validate once after the scaffold is complete**:

   ```bash
   repoctl graph validate
   repoctl run check --project <project-name>
   ```

   Keep graph validation as the final scaffold validation. Run the real project check once at
   hand-off. Run test or build only when the scaffold includes tests, generated clients, package
   metadata, build outputs, deploy artifacts, or task wiring that need verification. Use a dry-run
   only when task routing itself is unclear.

## Review checklist

- The project kind matches the ownership model.
- Manifest owners and task commands are real, not example placeholders.
- Generated packages are private and internal by default.
- Project docs and agent instructions point to the correct project name and path.
- The first commit leaves `repoctl graph validate` green.
- Project checks ran once at hand-off, with test/build gates included only when the scaffold needed
  them.

## Hand-off

Report the command used, generated project name, owner fields that still need team-specific tuning,
and the validation commands run.

