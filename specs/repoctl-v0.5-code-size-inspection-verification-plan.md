# repoctl v0.5 Code Size Inspection Verification Plan

Status: Draft

Related specs:

- [PRD](repoctl-v0.5-code-size-inspection-prd.md)
- [Design](repoctl-v0.5-code-size-inspection-design.md)
- [Implementation Plan](repoctl-v0.5-code-size-inspection-impl-plan.md)

## 1. Verification Goals

Verify that code-size inspection is:

- syntax-aware for Rust, TypeScript/TSX, and Python;
- configurable and deterministic;
- fast enough for whole-repository and PR-scoped workflows;
- integrated with repoctl's existing affected-file and project graph model;
- safe on hostile or malformed source files.

## 2. Unit Tests

### 2.1 Configuration

Test cases:

- default config is created when `inspection` is omitted;
- each default threshold matches the PRD: file 1000, function 250, block 50;
- file rule has `include_tests: false` by default;
- function and block rules have `include_tests: true` by default;
- invalid thresholds are rejected;
- unknown languages and rules are rejected;
- override reasons are required and length-bounded;
- later matching overrides win;
- CLI rule and language filters do not mutate resolved repo config.

### 2.2 Line Index

Test cases:

- empty file;
- single-line file without final newline;
- single-line file with final newline;
- multi-line file with CRLF;
- node ending at column 0;
- very large line count overflow handling;
- excluded ranges that start or end mid-line.

### 2.3 File Selection

Test cases:

- `all` scope respects ignore rules and hard-coded heavy directories;
- `changed` scope uses explicit changed files before git range;
- `changed` scope skips deleted files;
- `affected` scope scans direct project paths only by default;
- `affected` scope includes transitive project paths with `include_transitive`;
- generated paths are skipped when configured;
- unsupported extensions are skipped before file reads;
- max file and max files limits produce diagnostics.

## 3. Language Fixture Tests

Keep fixtures under the inspector crate, for example:

```text
crates/repoctl-inspect/fixtures/code-size/rust/
crates/repoctl-inspect/fixtures/code-size/typescript/
crates/repoctl-inspect/fixtures/code-size/python/
```

### 3.1 Rust

Fixtures:

- `large_file.rs`: production file over threshold;
- `tests/large_test.rs`: test file exempt from file rule by default;
- `inline_cfg_test.rs`: production file with inline test module where only the
  test range is excluded from effective LOC;
- `large_function.rs`: function or method over 250 span lines;
- `large_block.rs`: nested `if` or `match` block over 50 span lines;
- `syntax_error.rs`: parse error emits syntax diagnostic and no function/block
  findings.

Expected assertions:

- correct diagnostic codes;
- correct path and one-based start/end lines;
- direct function body does not duplicate as a block finding;
- symbol name is present for named functions and methods.

### 3.2 TypeScript/TSX

Fixtures:

- `large-file.ts`;
- `large-component.tsx`;
- `large-function.ts`;
- `large-arrow-function.ts`;
- `large-block.ts`;
- `component.test.tsx`.

Expected assertions:

- `.tsx` uses TSX grammar;
- block-bodied arrow functions are detected as functions;
- expression-bodied arrow functions are not block findings;
- test file is exempt from file rule by default;
- nested `switch`, `try`, or `if` block is reported.

### 3.3 Python

Fixtures:

- `large_module.py`;
- `large_function.py`;
- `large_decorated_function.py`;
- `large_nested_block.py`;
- `test_large_module.py`;
- `mixed_production_and_test.py`.

Expected assertions:

- decorated function span includes decorators when appropriate;
- `async def` is detected;
- `test_*.py` is exempt from file rule by default;
- production code in mixed files still counts;
- nested `try`, `with`, `match`, or loop suites are reported.

## 4. CLI Integration Tests

Run through the compiled CLI with temporary repositories.

Commands:

```bash
repoctl inspect size --scope all --format human
repoctl inspect size --scope all --format json
repoctl inspect size --scope changed --changed-file apps/a/src/main.rs
repoctl inspect size --scope affected --changed-file apps/a/src/main.rs
repoctl inspect size --scope all --language rust --rule function
repoctl inspect size --scope all --fail-on warning
```

Assertions:

- human output is stable enough for snapshot testing;
- JSON output deserializes into `CodeSizeInspectionReport`;
- `--fail-on never` exits successfully with findings;
- `--fail-on warning` exits unsuccessfully when warning findings exist;
- missing base/head for changed or affected scope produces a useful diagnostic;
- language and rule filters reduce findings as expected.

## 5. PR And CI Integration Tests

Test cases:

- `repoctl pr summary` includes a code-size section when findings exist;
- `repoctl pr summary` omits the section when no findings exist;
- generated CI workflow can include an inspect step;
- inspect step appears after graph validation and before affected task runs;
- code-size failures do not hide graph validation diagnostics.

## 6. Performance Checks

Add a non-default ignored performance test or benchmark fixture that generates
many small source files and a few oversized files.

Measurements:

- changed scope over 50 files;
- affected scope over one project with 1000 files;
- all scope over 10000 files;
- parse-error density case.

The test should assert broad upper bounds only when stable on CI. Prefer local
benchmark documentation over brittle CI timing if the shared runner is noisy.

## 7. Security And Robustness Tests

Test cases:

- binary file with supported extension is skipped;
- file over `max_file_bytes` is skipped;
- source with invalid UTF-8 does not panic;
- path traversal in changed-file input is rejected by `RepoRelativePath`;
- too many files returns a bounded diagnostic;
- extremely long single line does not allocate per-character metadata;
- parser errors do not panic and do not produce bogus syntax findings.

## 8. Manual Verification

Before finishing implementation:

```bash
repoctl inspect size --scope all --format human
repoctl inspect size --scope changed --base origin/main --head HEAD --format json
repoctl inspect size --scope affected --base origin/main --head HEAD --include-transitive
```

Inspect:

- finding order;
- line numbers in an editor;
- test-file exemption behavior;
- generated-file skip counts;
- command exit status with each `--fail-on` mode.

## 9. Required Gates For Implementation PRs

When Rust source, manifests, or lockfiles change, run:

```bash
cargo build
cargo test
cargo +nightly fmt
cargo clippy -- -D warnings
```

For code-size implementation changes, also run the stricter lint pass when it
adds signal:

```bash
cargo clippy -- -D warnings -W clippy::pedantic
```

When dependencies or lockfiles change, run:

```bash
cargo audit
cargo deny check
```

For spec-only edits, validate Markdown links and index entries. Do not run the
Rust gate set unless Rust behavior changes.
