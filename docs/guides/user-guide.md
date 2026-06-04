# repoctl User Guide

This guide is for people using `repoctl` inside a monorepo. It focuses on daily workflows: creating projects, validating boundaries, understanding impact, preparing CI data, and generating context for AI agents.

## Mental model

`repoctl` treats a monorepo as a graph of functional projects:

- Apps are product or business surfaces. They own local API, web, jobs, deploy intent, docs, tests, and app-local infrastructure.
- Frameworks are reusable capabilities extracted from apps. Their public facade is stable; their internal area is protected.
- Foundation services are company-level business services with owned APIs, proto contracts, and clients.
- Proto roots own source proto packages. Generated code is treated as managed output.
- Core infrastructure owns shared infrastructure, separate from app-local IaC.
- Tools are developer, agent, and repository automation projects under `tools/`.

The graph is built from `repo.yaml` and every discovered `project.yaml`. Most commands first resolve the repository root, parse those manifests, validate domain values, then run the requested operation.

## Initialize a repository

```bash
repoctl init --name acme --repo ./acme
```

By default this creates a functional layout with repo-level policy, generated agent guidance, common roots, and next steps. Use `--dry-run` when you want to inspect the planned files without writing them:

```bash
repoctl init --name acme --repo ./acme --dry-run
```

`repoctl init` currently supports the `functional` layout and the `startup` or `enterprise` profiles:

```bash
repoctl init --name acme --profile enterprise --layout functional
```

## Create projects

Project names can be passed as bare slugs. `repoctl` places them under the correct functional root:

```bash
repoctl new app catalog --owner @catalog
# apps/catalog

repoctl new framework runtime --owner @platform
# frameworks/runtime

repoctl new foundation identity --owner @identity
# foundations/identity

repoctl new tool skills --owner @platform
# tools/skills
```

You can also pass explicit paths when you want the command to be unambiguous:

```bash
repoctl new app apps/catalog --stack rust-api,bun-web --iac pulumi --owner @catalog
```

Interactive prompting is available for human output when a project path is omitted. In automation, pass every required value explicitly and prefer JSON output.

## Validate the graph

Run graph validation after changing manifests, dependencies, proto ownership, generated-code policy, or project layout:

```bash
repoctl graph validate
```

Validation modes let you choose how much environment metadata to inspect:

```bash
repoctl graph validate --mode structural
repoctl graph validate --mode metadata
repoctl graph validate --mode full
```

Use `structural` for manifest and policy validation without package-manager or registry access. Use `metadata` when local language metadata is available. Use `full` when you want the broadest validation before release.

Use path-specific checks when a PR touches only a few files:

```bash
repoctl graph validate \
  --changed-file apps/catalog/api/src/lib.rs \
  --changed-file protos/company/identity/v1/user.proto
```

For a readable graph dump:

```bash
repoctl graph print
```

To inspect one project or graph node:

```bash
repoctl explain apps.catalog
```

## Check boundaries

Boundary checks catch policy violations such as app-to-app dependency, framework-internal dependency, generated-code edits, and production-risk paths.

```bash
repoctl lint-boundaries \
  --changed-file apps/catalog/api/src/lib.rs
```

Use `--format json` when another tool needs structured diagnostics:

```bash
repoctl lint-boundaries --format json
```

## Compute affected work

Use `repoctl affected` to understand what a change touches:

```bash
repoctl affected \
  --base origin/main \
  --head HEAD \
  --tasks check,test
```

You can also pass explicit files, which is useful in scripts and tests:

```bash
repoctl affected \
  --changed-file apps/catalog/api/src/lib.rs \
  --tasks check,test
```

The report includes directly affected projects, transitively affected projects, workspaces, task names, suggested reviewers, reasons, diagnostics, and risk flags.

## Run tasks

Tasks are declared in project manifests. Use `--dry-run` to review planned commands:

```bash
repoctl run check --affected --dry-run
```

Run for specific projects or workspaces:

```bash
repoctl run test --project apps.catalog
repoctl run build --workspace apps.catalog:api
```

Limit concurrency when a task is expensive:

```bash
repoctl run test --affected --concurrency 4
```

## Generate CI data

For local inspection:

```bash
repoctl ci matrix --tasks check,test,build
```

When no base/head range is available, choose a fallback explicitly:

```bash
repoctl ci matrix --tasks check,test,build --fallback all
repoctl ci matrix --tasks check,test,build --fallback none
repoctl ci matrix --tasks check,test,build --fallback error
```

For GitHub Actions matrix JSON:

```bash
repoctl ci matrix \
  --base origin/main \
  --head HEAD \
  --tasks check,test,build \
  --format github-actions
```

Use `repoctl ci summarize` when a workflow needs a repo-level CI summary.

Render or refresh the maintained GitHub Actions workflow:

```bash
repoctl ci workflow --provider github-actions
repoctl ci workflow --provider github-actions --write
```

Run one matrix entry locally:

```bash
repoctl ci run-step --project apps.catalog --workspace api --task check
```

## Adopt existing repositories

Adoption is plan-first. Generate a reviewed plan from existing standalone repositories:

```bash
repoctl adopt plan \
  --source ~/projects/cellis \
  --dest ~/projects/cellis/universe \
  --exclude synapse \
  --format json > target/repoctl/adopt-plan.json
```

Use overrides when placement confidence is low or when the inferred lane is wrong:

```bash
repoctl adopt plan \
  --source source \
  --dest dest \
  --map operon=frameworks/operon \
  --kind skills=tool \
  --owner operon=@platform
```

Apply and verify the reviewed plan:

```bash
repoctl adopt apply --plan target/repoctl/adopt-plan.json
repoctl adopt verify --plan target/repoctl/adopt-plan.json
```

The plan reports placement decisions, copied files, generated manifests, dependency rewrites, CI operations, hygiene warnings, prerequisites, and verification commands. Source repositories are not modified.

## Check hygiene

Run hygiene checks after adoption or any broad copy operation:

```bash
repoctl hygiene check
repoctl hygiene clean --dry-run
```

Hygiene detects nested `.git`, nested `.github`, `node_modules`, `target`, `dist`, frontend caches, and other generated artifacts. Clean plans only include generated artifacts; source files and lockfiles are not deleted.

## Work with templates

List templates:

```bash
repoctl template list
```

Render a built-in template:

```bash
repoctl template render builtin:app \
  --input name=catalog \
  --dry-run
```

Render local templates with the `local:` prefix:

```bash
repoctl template render local:templates/app \
  --input name=catalog
```

Templates are plan-first. Inspect the planned operations before writing files when changing shared templates.

## Proto, generated code, and IaC

Find proto owners and consumers:

```bash
repoctl proto owners company.identity.v1
repoctl proto consumers protos/company/identity/v1/user.proto
```

Check generated-code policy:

```bash
repoctl codegen check --base origin/main --head HEAD
repoctl proto check --base origin/main --head HEAD
```

Plan infrastructure commands without applying:

```bash
repoctl iac plan --affected --env staging
repoctl iac plan --project apps.catalog --env prod
repoctl iac plan --core --env prod
```

`repoctl iac plan` reports commands and risk flags. It does not run provider apply operations.

## AI context and PR summaries

Build project-scoped AI context:

```bash
repoctl context apps.catalog --for ai --format json
```

Build a PR summary:

```bash
repoctl pr summary --base origin/main --head HEAD
```

The PR summary is designed to surface affected projects, risk, reviewers, and recommended checks.

## Output formats

Use human output for terminal workflows:

```bash
repoctl graph validate --format human
```

Use JSON output for scripts:

```bash
repoctl affected --format json
```

Use GitHub Actions output only where the command supports a CI-shaped payload, especially `repoctl ci matrix`:

```bash
repoctl ci matrix --tasks check,test --format github-actions
```

## Practical workflow

For a normal PR:

1. Scaffold or edit inside the right functional project.
2. Run `repoctl graph validate`.
3. Run `repoctl affected --base origin/main --head HEAD --tasks check,test`.
4. Run the smallest meaningful task set, often through `repoctl run <task> --affected`.
5. Use `repoctl pr summary` to capture impact and review hints.

When a command returns diagnostics, fix the manifest or boundary issue first. Downstream commands rely on the graph being valid.
