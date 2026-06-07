---
name: "proto-change"
description: "Change source proto contracts through owners and never edit generated code directly."
---

# proto-change

This skill records repository policy for Claude. Use repoctl as the authority for graph,
ownership, affected analysis, task routing, and final hand-off verification, but keep repoctl out of
the inner coding loop unless the graph or boundaries are changing.

## When this fires

- The user changes `.proto` files, generated clients, API contracts, event schemas, or service
  package ownership.
- A generated file appears in the diff and it is unclear whether it should be regenerated or left
  untouched.
- A PR may break consumers of a foundation service or cross-language client.

## Source of truth

The source contract lives under `protos/` unless a `project.yaml` explicitly declares another proto
source root. Generated code is consumer-local output and should not be edited to change behavior.

## Workflow

1. **Find owners and consumers**:

   ```bash
   repoctl proto owners <package-or-path>
   repoctl proto consumers <package-or-path>
   ```

   If ownership is missing, update the owning project manifest before changing the contract.

2. **Read the package context**:

   - package name and versioning convention,
   - service and message consumers,
   - generated language targets,
   - backwards-compatibility rules in the owning project docs.

3. **Edit source only**:

   Change `protos/**` or the manifest-declared source root. Do not patch `generated/`, `gen/`,
   checked-in language clients, or build output as a substitute for changing the contract.

4. **Preserve compatibility unless the user explicitly requests a breaking change**:

   - Add fields instead of renaming or reusing numbers.
   - Reserve removed field numbers and names.
   - Keep package names stable.
   - Check JSON names and language-specific reserved words when adding public fields.

5. **Run proto verification once at hand-off**:

   ```bash
   repoctl proto check --base origin/main --head HEAD
   repoctl affected --base origin/main --head HEAD --tasks check,test
   ```

   Run `repoctl proto check` at hand-off and run affected analysis once to select consumer
   verification. Do not recompute affected after source-only proto fixes. If the repo has a
   declared generation task, run it through `repoctl run` for affected consumers.

## Review checklist

- Owner and consumer lists are included in the PR summary.
- Breaking changes are explicit, justified, and routed to consumers.
- Generated output is either absent from the diff or produced by the repo's generation task.
- Affected app/framework/foundation tasks cover every consumer repoctl reports.

## Hand-off

State the package changed, owners, consumers, compatibility impact, generation status, and commands
run. If a consumer cannot be tested locally, name it as a residual risk.

