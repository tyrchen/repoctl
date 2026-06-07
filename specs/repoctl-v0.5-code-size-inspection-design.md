# repoctl v0.5 Code Size Inspection Design

Status: Draft

Related specs:

- [PRD](repoctl-v0.5-code-size-inspection-prd.md)
- [Implementation Plan](repoctl-v0.5-code-size-inspection-impl-plan.md)
- [Verification Plan](repoctl-v0.5-code-size-inspection-verification-plan.md)
- [Dependency Research](../docs/research/repoctl-v0.5-code-size-inspection-dependency-research.md)

## 1. Design Thesis

Code-size inspection should be a syntax-aware repository scan, not a language
toolchain wrapper.

The scanner should:

1. select files using repoctl's existing graph, git, and ignore rules;
2. count lines with byte-level prefilters;
3. use Tree-sitter for function and block boundaries in supported languages;
4. emit repoctl diagnostics and stable JSON;
5. keep parser dependencies out of `repoctl-core`.

## 2. Architecture

Add a new crate:

```text
crates/repoctl-inspect
```

Responsibilities:

- resolve code-size configuration into immutable scan settings;
- select source files for `all`, `changed`, and `affected` scopes;
- detect language from repo-relative path and extension;
- classify test files and syntax ranges;
- parse supported files with Tree-sitter;
- evaluate file, function, and block rules;
- return `CodeSizeInspectionReport`.

Existing crates keep their roles:

- `repoctl-core`: configuration DTOs, request/report DTOs, diagnostic types,
  validated newtypes.
- `repoctl-engine`: discovery, graph, policy evaluation, local filesystem walk.
- `repoctl-runner`: git changed-file provider and affected analysis already used
  by PR and CI flows.
- `repoctl`: public facade wiring.
- `repoctl-cli`: command parsing and rendering.

`repoctl-inspect` may depend on `repoctl-core`, `repoctl-engine`, and
`repoctl-runner`, but `repoctl-core` must not depend on Tree-sitter.

## 3. Domain Types

Add request/report DTOs in `repoctl-core`.

```rust
pub struct CodeSizeInspectionRequest {
    pub repo: Option<PathBuf>,
    pub scope: CodeSizeScope,
    pub base: Option<String>,
    pub head: Option<String>,
    pub changed_files: Vec<RepoRelativePath>,
    pub include_transitive: bool,
    pub languages: Vec<CodeLanguage>,
    pub rules: Vec<CodeSizeRuleKind>,
    pub fail_on: InspectionFailOn,
}
```

```rust
pub struct CodeSizeInspectionReport {
    pub scope: CodeSizeScope,
    pub base: Option<String>,
    pub head: Option<String>,
    pub summary: CodeSizeInspectionSummary,
    pub findings: Vec<CodeSizeFinding>,
    pub diagnostics: Vec<Diagnostic>,
}
```

```rust
pub struct CodeSizeFinding {
    pub rule: CodeSizeRuleKind,
    pub severity: Severity,
    pub path: RepoRelativePath,
    pub project: Option<ProjectName>,
    pub language: CodeLanguage,
    pub symbol: Option<String>,
    pub node_kind: Option<String>,
    pub start_line: NonZeroU32,
    pub end_line: NonZeroU32,
    pub measured_lines: NonZeroU32,
    pub limit: NonZeroU32,
    pub message: String,
}
```

Enums:

- `CodeSizeScope`: `All`, `Changed`, `Affected`.
- `CodeLanguage`: `Rust`, `TypeScript`, `Python`.
- `CodeSizeRuleKind`: `File`, `Function`, `Block`.
- `InspectionFailOn`: `Never`, `Error`, `Warning`.
- `GeneratedCodeInspectionMode`: `Skip`, `Inspect`.

All public DTOs must derive `Debug`, `Clone` where useful, `Serialize`, and
`Deserialize`. JSON fields use `camelCase`.

## 4. Manifest Configuration

Extend `RawRepoManifest` with an optional `inspection` section and convert into
a validated `RepoInspectionConfig`.

Repo-level defaults:

```rust
pub struct RepoInspectionConfig {
    pub code_size: CodeSizeConfig,
}
```

```rust
pub struct CodeSizeConfig {
    pub enabled: bool,
    pub generated_code: GeneratedCodeInspectionMode,
    pub max_files: NonZeroUsize,
    pub max_file_bytes: NonZeroUsize,
    pub rules: CodeSizeRuleConfigSet,
    pub languages: BTreeMap<CodeLanguage, CodeLanguageConfig>,
    pub excludes: Vec<RepoGlob>,
    pub overrides: Vec<CodeSizeOverride>,
}
```

Configuration is resolved in this order:

1. built-in defaults;
2. repo-level `inspection.code_size`;
3. language-specific settings;
4. first matching path override, then later matching overrides in file order.

Later matching overrides win. Report output must include enough resolved values
to explain which limit applied.

## 5. File Selection

### 5.1 Common Filtering

Every scope applies the same filter pipeline:

1. repo-relative path validates with `RepoRelativePath`;
2. path is not excluded by repoctl's hard-coded heavy directory filter;
3. path is not excluded by `inspection.code_size.excludes`;
4. extension maps to a supported language;
5. language is enabled and not filtered out by CLI;
6. file exists in the worktree;
7. file size is at most `max_file_bytes`;
8. file is not generated code when `generated_code: skip`.

Hard-coded heavy directories:

- `.git`;
- `target`;
- `node_modules`;
- `dist`;
- `.next`;
- `.turbo`;
- package-manager caches;
- build outputs already skipped by existing repoctl hygiene logic.

### 5.2 `all` Scope

Use `ignore::WalkBuilder` through the same local traversal discipline as
`LocalRepoFileSystem`. Scan all supported files under the repository root.

### 5.3 `changed` Scope

Use explicit `--changed-file` values when present. Otherwise use the existing
git provider with `git diff --name-only --diff-filter=ACMR <base> <head>`.

Deleted files are skipped with a bounded skip reason.

### 5.4 `affected` Scope

Resolve changed files as in `changed` scope, compute `AffectedReport`, then scan
all supported files under directly affected project paths. If
`--include-transitive` is set, include transitively affected projects.

This gives maintainers a project-level refactor signal without scanning the
entire repository.

## 6. Language Detection

Initial mapping:

| Language | Extensions |
| --- | --- |
| Rust | `.rs` |
| TypeScript | `.ts`, `.tsx`, `.mts`, `.cts` |
| Python | `.py`, `.pyi` |

TypeScript `.tsx` files use the TSX grammar. Other TypeScript extensions use
the TypeScript grammar.

JavaScript is intentionally not enabled by the TypeScript rule in v0.5. It can
be added later with a separate JavaScript language mapping.

## 7. Parsing Pipeline

Per file:

1. read bytes with bounded size;
2. reject binary-looking files by checking for NUL bytes in the first bounded
   prefix;
3. build a line index using `memchr` newline search;
4. resolve language and test-path classification;
5. parse bytes with a worker-owned Tree-sitter parser;
6. if parse returns an error tree, still report file-size findings but add a
   syntax diagnostic and skip function/block findings for that file;
7. run language-specific queries for function, block, comment, and test ranges;
8. compute rule findings;
9. sort findings by path, start line, rule rank, symbol, and node kind.

Tree-sitter parser instances are not shared across threads. Each worker owns a
parser cache keyed by `CodeLanguage`.

## 8. Queries

Each language has a small query module:

```text
rust_queries.rs
typescript_queries.rs
python_queries.rs
```

Each module exposes:

- function query;
- block query;
- comment query;
- test-range query;
- symbol-name extraction rules.

Captures use common names:

- `@function.outer`;
- `@function.name`;
- `@block.outer`;
- `@comment`;
- `@test.outer`.

The query layer returns neutral `SyntaxSpan` values:

```rust
pub struct SyntaxSpan {
    pub kind: SyntaxSpanKind,
    pub node_kind: String,
    pub symbol: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: NonZeroU32,
    pub end_line: NonZeroU32,
}
```

This prevents rule evaluation from depending directly on Tree-sitter node
lifetimes.

## 9. Line Counting

File rule:

- physical lines: count newline bytes and account for a final non-newline line;
- effective lines: count physical lines that contain non-whitespace source
  outside comment and excluded test ranges.

Function and block rules:

- span lines: count from the first syntactic line to the last syntactic line;
- if a node ends at column 0, use the previous physical line as the last line;
- report one-based line numbers.

All arithmetic uses checked or saturating operations. File line counts are
bounded by `u32::MAX`; files that exceed representable line counts are skipped
with an error diagnostic rather than truncated silently.

## 10. Test Handling

The scanner classifies tests in two layers:

1. path-level test classification;
2. syntax-level test ranges.

The file rule with `include_tests: false` subtracts syntax test ranges from
effective LOC when available. A path-level test file is fully exempt from the
file rule.

Function and block rules check whether the span is contained in a test range or
test path. They skip the finding only when that rule has `include_tests: false`.

## 11. Generated Code

Generated code is skipped by default.

Sources of generated-code classification:

- existing repo-level generated policy;
- project `ai.do_not_edit` patterns;
- default `**/generated/**`;
- common generated filenames such as `*.pb.rs`, `*.pb.go`, generated TypeScript
  clients, and generated OpenAPI artifacts only when configured in a later
  implementation pass.

When generated code is skipped, the report includes a bounded skipped-file count
and reason.

## 12. Diagnostics

Finding diagnostic codes:

- `inspect.code_size.file_too_large`;
- `inspect.code_size.function_too_large`;
- `inspect.code_size.block_too_large`.

Operational diagnostic codes:

- `inspect.code_size.no_base_head`;
- `inspect.code_size.no_changed_files`;
- `inspect.code_size.unsupported_language`;
- `inspect.code_size.file_too_large_to_scan`;
- `inspect.code_size.binary_file_skipped`;
- `inspect.code_size.syntax_error`;
- `inspect.code_size.too_many_files`;
- `inspect.code_size.config_invalid`.

Findings use configured severity. Operational parse and IO issues are warnings
unless they make the scan impossible, in which case they are errors.

## 13. Performance Model

The scanner is designed as a bounded parallel map over selected files.

Fast paths:

- unsupported extensions are rejected before file reads;
- file bytes are bounded before full read;
- newline counting uses byte search;
- query compilation is cached per language;
- parser instances are reused inside a worker;
- changed-file scope avoids repository walking;
- affected scope walks only selected project roots.

Default worker count should be `min(available_parallelism, 8)` unless configured
or overridden later. Results are sorted after collection for deterministic
output.

## 14. Security And Resource Limits

- Validate all config values at manifest parse time.
- Reject path traversal through existing `RepoRelativePath` validation.
- Do not execute source code or language-specific tools.
- Cap file count, file byte length, glob count, and override count.
- Treat file bytes as untrusted input. Avoid panics on invalid UTF-8 by using
  byte ranges and lossy display only for snippets if snippets are ever added.
- Avoid `unwrap()` and `expect()` in production scanning code.
- Do not log source snippets by default.

## 15. Open Design Decisions

- Whether the affected-project scan should include transitive projects by
  default. The initial design keeps it explicit with `--include-transitive`.
- Whether JavaScript should be added as a separate v0.6 language. TypeScript
  support in v0.5 does not imply JavaScript support.
- Whether class size should become a fourth rule. v0.5 keeps the requested
  three rules only.
