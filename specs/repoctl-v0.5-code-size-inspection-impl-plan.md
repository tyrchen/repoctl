# repoctl v0.5 Code Size Inspection Implementation Plan

Status: Draft

Related specs:

- [PRD](repoctl-v0.5-code-size-inspection-prd.md)
- [Design](repoctl-v0.5-code-size-inspection-design.md)
- [Verification Plan](repoctl-v0.5-code-size-inspection-verification-plan.md)

## 1. Implementation Strategy

Implement the feature in small vertical slices:

1. domain and configuration DTOs;
2. deterministic file selection;
3. Rust parsing and findings;
4. TypeScript/TSX parsing and findings;
5. Python parsing and findings;
6. CLI and report rendering;
7. PR and CI integration.

Each slice should include focused tests before moving to the next language.

## 2. Dependency Changes

Add workspace dependencies when implementation starts:

```toml
memchr = "2.8.1"
rayon = "1.12.0"
tree-sitter = "0.26.9"
tree-sitter-rust = "0.24.2"
tree-sitter-typescript = "0.23.2"
tree-sitter-python = "0.25.0"
```

Update existing `ignore` from `0.4.25` to `0.4.26` in the same dependency PR if
the lockfile resolves cleanly.

Because this changes parser and supply-chain dependencies, run:

```bash
cargo audit
cargo deny check
```

## 3. Phase 1: Core DTOs And Config

Files:

- `crates/core/src/domain.rs`
- `crates/core/src/manifest.rs`
- `crates/core/src/lib.rs`

Tasks:

- add `CodeSizeInspectionRequest`;
- add `CodeSizeInspectionReport`;
- add `CodeSizeFinding`;
- add `CodeSizeInspectionSummary`;
- add `CodeSizeScope`;
- add `CodeLanguage`;
- add `CodeSizeRuleKind`;
- add `InspectionFailOn`;
- add `GeneratedCodeInspectionMode`;
- add `RepoInspectionConfig` and `CodeSizeConfig`;
- add raw manifest parsing for `inspection.code_size`;
- validate config bounds and override reason text;
- export all public DTOs from `repoctl_core`.

Tests:

- default config is applied when `inspection` is omitted;
- invalid rule names, languages, severities, and bounds produce diagnostics;
- overrides resolve in deterministic order;
- JSON serialization uses camelCase.

## 4. Phase 2: Inspector Crate Skeleton

Files:

- `crates/repoctl-inspect/Cargo.toml`
- `crates/repoctl-inspect/src/lib.rs`
- root `Cargo.toml`

Tasks:

- add `InspectorService`;
- wire `RepoctlEngine` and `RunnerService` or the existing git provider;
- define internal `FileSelection`, `SelectedSourceFile`, `SourceFileBytes`,
  `LanguageDetector`, and `ResolvedCodeSizeConfig`;
- implement `all`, `changed`, and `affected` scope selection;
- reuse `ignore` traversal for repository and project-root scans;
- enforce `max_files` and `max_file_bytes`;
- return empty reports with useful diagnostics before parsing is added.

Tests:

- changed scope prefers explicit `changed_files`;
- changed scope uses base/head when explicit files are absent;
- affected scope scans direct project roots;
- affected scope includes transitive projects only when requested;
- unsupported extensions are skipped before reads;
- deleted changed files are skipped cleanly.

## 5. Phase 3: Shared Scan Infrastructure

Files:

- `crates/repoctl-inspect/src/line_index.rs`
- `crates/repoctl-inspect/src/report.rs`
- `crates/repoctl-inspect/src/rules.rs`

Tasks:

- implement binary prefix detection;
- implement physical line counting with `memchr`;
- implement `LineIndex` for byte-to-line mapping;
- implement effective LOC calculation from excluded ranges;
- implement deterministic finding sorting;
- implement generated-code and test-path classifiers;
- implement override resolution per path/language/rule.

Tests:

- files with and without final newline count correctly;
- end-column-zero spans do not overcount by one line;
- excluded comment/test ranges reduce effective LOC;
- generated-code skip reasons are bounded;
- rule filters suppress unrequested rules.

## 6. Phase 4: Rust Language Support

Files:

- `crates/repoctl-inspect/src/languages/rust.rs`
- `crates/repoctl-inspect/src/languages/mod.rs`

Tasks:

- initialize `tree_sitter_rust::LANGUAGE`;
- compile Rust queries for functions, blocks, comments, and test ranges;
- extract function names from function and method nodes;
- suppress direct function bodies from block findings;
- detect `#[cfg(test)] mod` and `#[test]` ranges;
- add Rust fixtures.

Fixtures:

- oversized production file;
- test file over file limit that is exempt by default;
- production file with inline `#[cfg(test)]` module where only test range is
  subtracted;
- oversized function;
- oversized nested `match` or `if` block;
- parse-error file that still reports file-size data but skips syntax findings.

## 7. Phase 5: TypeScript And TSX Support

Files:

- `crates/repoctl-inspect/src/languages/typescript.rs`

Tasks:

- select TypeScript grammar for `.ts`, `.mts`, `.cts`;
- select TSX grammar for `.tsx`;
- query function declarations, methods, constructors, function expressions,
  generator functions, and block-bodied arrow functions;
- query nested executable statement blocks;
- query comments;
- classify common test files and test callback ranges inside test paths;
- extract symbol names when possible.

Fixtures:

- `.ts` oversized function declaration;
- `.tsx` oversized component function;
- arrow function with expression body that is not treated as block-sized;
- nested `switch` or `try` block over limit;
- test file exemption for file-size rule.

## 8. Phase 6: Python Support

Files:

- `crates/repoctl-inspect/src/languages/python.rs`

Tasks:

- initialize `tree_sitter_python::LANGUAGE`;
- query functions, async functions, methods, comments, and nested suites;
- detect test paths, `test_*` functions, and `Test*` classes;
- extract symbol names;
- handle decorators as part of function span where the grammar exposes them.

Fixtures:

- oversized module;
- oversized function with decorators;
- oversized nested `try` or `match` block;
- `test_*.py` exemption for file-size rule;
- production file with `test_*` function range and production code.

## 9. Phase 7: Facade And CLI

Files:

- `crates/repoctl/src/lib.rs`
- `apps/repoctl-cli/src/main.rs`

Tasks:

- add `Repoctl::inspect_code_size`;
- add top-level `Inspect` command and `Size` subcommand;
- parse scope, language, rule, fail-on, base/head, changed-file, and
  include-transitive flags;
- render human report;
- render JSON report;
- set exit code from `fail_on` without changing report contents.

CLI examples to validate:

```bash
repoctl inspect size --scope all
repoctl inspect size --scope changed --base origin/main --head HEAD
repoctl inspect size --scope affected --base origin/main --head HEAD --include-transitive
repoctl inspect size --scope all --language rust --rule function --format json
```

## 10. Phase 8: PR And CI Integration

Files:

- `crates/repoctl-runner/src/lib.rs`
- `crates/repoctl-scaffold/src/lib.rs`
- `apps/repoctl-cli/src/main.rs`

Tasks:

- add code-size findings to PR summary when changed-file or affected-project
  inspection finds violations;
- add optional CI workflow step generated by `repoctl ci workflow`;
- keep code-size inspection separate from `graph validate`;
- document the generated CI default `--fail-on error` or `--fail-on warning`
  behavior in the workflow rendering.

## 11. Migration Notes

- Existing repos without `inspection.code_size` get defaults and no manifest
  churn.
- The first implementation should not rewrite `repo.yaml` automatically.
- If a repo wants to ratchet down existing debt, it can start with
  `--scope changed` in CI and run `--scope all` locally or periodically.

## 12. Implementation Risks

- TypeScript grammar crate size may noticeably increase compile time. Isolating
  it in `repoctl-inspect` keeps the dependency out of the core model.
- Tree-sitter node kinds can change across grammar versions. Fixtures must lock
  expected behavior for the supported versions.
- File effective LOC can be expensive if every file requires syntax comment
  ranges. The implementation should use physical-line prefiltering and still
  parse only supported files required by enabled rules.
- Test range classification is intentionally conservative. When uncertain,
  classify as production rather than accidentally hiding a production finding.
