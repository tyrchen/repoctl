# repoctl v0.5 Code Size Inspection PRD

Status: Draft

Related specs:

- [repoctl v0.2 Design Spec](repoctl-v0.2-design.md)
- [repoctl v0.3 Adoption And Monorepo Hardening Spec](repoctl-v0.3-adoption-spec.md)
- [repoctl v0.4 Operations Session Review Spec](repoctl-v0.4-operations-session-review-spec.md)
- [repoctl v0.5 Code Size Inspection Design](repoctl-v0.5-code-size-inspection-design.md)
- [repoctl v0.5 Code Size Inspection Implementation Plan](repoctl-v0.5-code-size-inspection-impl-plan.md)
- [repoctl v0.5 Code Size Inspection Verification Plan](repoctl-v0.5-code-size-inspection-verification-plan.md)

## 1. Problem Statement

Large files, oversized functions, and oversized nested blocks are early signals
that a change is becoming hard to review and maintain. Today repoctl can explain
project boundaries, affected tasks, generated-code policy, IaC risk, and PR
impact, but it does not inspect code shape inside source files.

Agents and maintainers currently discover this too late:

- a production source file grows past the point where ownership and cohesion are
  obvious;
- a function accumulates several responsibilities and becomes difficult to test;
- a nested block hides a second workflow inside a larger workflow;
- a PR only touches a few lines in an already oversized unit, but repoctl cannot
  show that the affected surface needs refactoring.

This should be a first-class inspection capability, with configurable thresholds
and fast scans for whole repositories and PR-scoped changes.

## 2. Goals

- Add code-size inspection for these default rules:
  - file too large: production source file over 1000 effective lines of code;
  - function too large: function-like item over 250 span lines;
  - block too large: nested executable block over 50 span lines.
- Make all thresholds configurable at repo and path/language override levels.
- Support whole-repository scans, changed-file scans, and affected-project scans.
- Initially support Rust, TypeScript/TSX, and Python.
- Use syntax-aware detection for functions and blocks. For supported languages,
  do not rely on regexes to identify functions or blocks.
- Produce deterministic human and JSON reports that can be used in local review,
  CI, and PR summaries.
- Respect repoctl's existing ignore, generated-code, graph, and affected-change
  model.
- Keep the implementation fast enough for normal local use without spawning
  language compilers, linters, or type checkers.

## 3. Non-Goals

- Do not automatically rewrite or refactor code.
- Do not compute cyclomatic complexity, cognitive complexity, or maintainability
  indexes in v0.5.
- Do not require Rust, TypeScript, or Python toolchains to be installed for
  inspection.
- Do not inspect vendored dependencies, lockfiles, generated artifacts, build
  outputs, minified bundles, or binary files by default.
- Do not block test files on the file-size rule by default. Test handling must
  be configurable per rule.
- Do not make Tree-sitter dependencies part of `repoctl-core`.

## 4. User Stories

As a maintainer, I can run:

```bash
repoctl inspect size --scope all
```

and see every oversized file, function, and block in the repository.

As a PR author, I can run:

```bash
repoctl inspect size --scope changed --base origin/main --head HEAD
```

and see only violations in changed source files.

As a platform owner, I can run:

```bash
repoctl inspect size --scope affected --base origin/main --head HEAD
```

and inspect every supported source file in directly affected projects, with an
option to include transitive projects.

As a repo owner, I can configure stricter or looser limits per language or path:

```yaml
inspection:
  code_size:
    rules:
      file:
        max_lines: 1000
        include_tests: false
      function:
        max_lines: 250
      block:
        max_lines: 50
    overrides:
      - paths:
          - "crates/core/src/domain.rs"
        rules:
          file:
            max_lines: 1600
        reason: "single DTO module kept together for public schema stability"
```

As a reviewer, I can read a PR summary that says:

```text
Code size:
- crates/core/src/domain.rs: file has 1248 effective LOC, limit 1000
- apps/web/src/routes/admin.tsx: function AdminRoute has 287 lines, limit 250
```

## 5. Rule Semantics

### 5.1 File Too Large

Default threshold: 1000 effective lines of code.

The file rule applies to production source files by default. Test source is
excluded from this rule unless `include_tests: true`.

Effective LOC means source lines with non-whitespace code after excluding:

- blank lines;
- language comment nodes;
- generated-code ranges and files skipped by repoctl policy;
- test ranges when the file rule has `include_tests: false`.

The report must include both `effectiveLines` and `physicalLines` so humans can
understand why a file was or was not reported.

### 5.2 Function Too Large

Default threshold: 250 span lines.

The function rule applies to language-specific function-like units:

- Rust: free functions, methods, trait default methods, impl items, closures
  when the closure body has a block;
- TypeScript/TSX: function declarations, methods, constructors, function
  expressions, generator functions, arrow functions with block bodies;
- Python: functions, async functions, methods, decorated functions.

Span lines are measured from the first syntactic line of the function-like node
to the last syntactic line of that node. The report should include the symbol
name when available and a fallback node kind when not.

### 5.3 Block Too Large

Default threshold: 50 span lines.

The block rule applies to nested executable bodies, not to a direct function
body already governed by the function rule.

Initial block targets:

- Rust: `if`, `else`, `for`, `while`, `loop`, `match` arms with block bodies,
  async blocks, unsafe blocks if any dependency grammar emits them even though
  repoctl crates forbid unsafe code;
- TypeScript/TSX: `if`, `else`, `for`, `for await`, `while`, `do`, `switch`,
  `try`, `catch`, block statements, class static blocks;
- Python: nested suites under `if`, `elif`, `else`, `for`, `while`, `with`,
  `try`, `except`, `finally`, `match`, and nested function/class bodies when
  configured.

Direct function bodies are suppressed for this rule to avoid duplicate findings.
If a block and its parent block both exceed the limit, both may be reported when
they describe different refactoring units.

## 6. Test Classification

Test classification is path-first and syntax-refined.

Default test paths:

- Rust: `**/tests/**`, `**/benches/**`, `**/*_test.rs`;
- TypeScript/TSX: `**/__tests__/**`, `**/*.test.ts`, `**/*.test.tsx`,
  `**/*.spec.ts`, `**/*.spec.tsx`;
- Python: `**/tests/**`, `**/test_*.py`, `**/*_test.py`.

Syntax-refined test ranges:

- Rust: `#[cfg(test)] mod ...` ranges and functions with `#[test]`;
- TypeScript/TSX: `describe`, `it`, `test`, and `expect` callback ranges only
  when captured by a language query and inside test paths by default;
- Python: functions named `test_*`, methods named `test_*`, and classes named
  `Test*`.

The file rule excludes test ranges only when the implementation can identify
them safely. It must not treat an entire Rust production file as test-only just
because it contains an inline `#[cfg(test)]` module.

## 7. Command Surface

### 7.1 `repoctl inspect size`

```bash
repoctl inspect size \
  --scope all \
  --format human
```

Options:

- `--repo <PATH>`: repository root or path inside the repo.
- `--scope <all|changed|affected>`: scan mode. Default is `all`.
- `--base <REF>` and `--head <REF>`: git range for `changed` or `affected`.
- `--changed-file <PATH>`: explicit changed source file. Repeatable.
- `--include-transitive`: include transitively affected projects for
  `--scope affected`.
- `--language <rust,typescript,python>`: language filter. Repeatable or
  comma-delimited.
- `--rule <file,function,block>`: rule filter. Repeatable or comma-delimited.
- `--fail-on <never|error|warning>`: process exit behavior. Default `never` for
  local human output and `error` for CI templates.
- `--format <human|json>`: output format. Default `human`.

No mutating mode is required.

### 7.2 Integration Points

- `repoctl pr summary` should include a concise code-size section when findings
  exist in changed files or affected projects.
- `repoctl ci workflow` should be able to include a non-mutating inspection step
  after graph validation and before task execution.
- `repoctl graph validate` should not run code-size inspection by default. This
  keeps graph validation structural and fast.

## 8. Configuration

Configuration lives under `repo.yaml`:

```yaml
inspection:
  code_size:
    enabled: true
    generated_code: skip
    max_files: 50000
    max_file_bytes: 2000000
    rules:
      file:
        enabled: true
        max_lines: 1000
        severity: warning
        include_tests: false
      function:
        enabled: true
        max_lines: 250
        severity: warning
        include_tests: true
      block:
        enabled: true
        max_lines: 50
        severity: warning
        include_tests: true
    languages:
      rust:
        enabled: true
      typescript:
        enabled: true
      python:
        enabled: true
    excludes:
      - "**/target/**"
      - "**/node_modules/**"
      - "**/dist/**"
      - "**/.next/**"
    overrides: []
```

Validation requirements:

- `max_lines` must be `1..=100000`.
- `max_files` must be `1..=1000000`.
- `max_file_bytes` must be `1..=50000000`.
- path glob count must be bounded.
- override `reason` must be non-empty and length-bounded.
- unknown languages, rules, severities, and generated-code modes are errors.

## 9. Reporting

Human output should be stable and terse:

```text
Code size inspection: 3 findings across 2 files

warning  inspect.code_size.file_too_large
  crates/core/src/domain.rs: file has 1248 effective LOC, limit 1000

warning  inspect.code_size.function_too_large
  apps/repoctl-cli/src/main.rs: function render_ops_plan has 287 lines, limit 250

warning  inspect.code_size.block_too_large
  apps/web/src/routes/admin.tsx: block in AdminRoute spans 63 lines, limit 50
```

JSON output must include:

- scan scope and git range;
- resolved configuration digest;
- files considered, scanned, skipped, and errored;
- total duration in milliseconds;
- findings with rule, severity, path, language, project, symbol, node kind,
  start line, end line, measured line count, configured limit, and explanation;
- skipped files with bounded reason counts.

## 10. Success Criteria

- Whole-repository scan handles at least 10000 supported source files on a
  developer machine without invoking external language tools.
- Changed-file scan over 50 changed source files completes quickly enough to be
  part of a pre-PR local workflow.
- Findings are deterministic across runs on the same checkout.
- Rust, TypeScript/TSX, and Python fixture coverage catches function and nested
  block oversize cases.
- Configuration overrides can make a finding disappear only when the override
  matches the path, rule, and language.
- Test-file exemption applies to file-size findings by default and can be
  disabled.
