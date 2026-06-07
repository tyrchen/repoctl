# repoctl v0.5 Code Size Inspection Dependency Research

Date: 2026-06-07

This note records dependency and tooling checks for the v0.5 code-size
inspection specs:

- [PRD](../../specs/repoctl-v0.5-code-size-inspection-prd.md)
- [Design](../../specs/repoctl-v0.5-code-size-inspection-design.md)
- [Implementation plan](../../specs/repoctl-v0.5-code-size-inspection-impl-plan.md)
- [Verification plan](../../specs/repoctl-v0.5-code-size-inspection-verification-plan.md)

## Inputs Checked

- `tree-sitter` latest docs show 0.26.9, with Rust bindings to the
  Tree-sitter parsing library. Source: <https://docs.rs/crate/tree-sitter/latest>.
- `tree-sitter-rust` latest docs show 0.24.2, with a Rust grammar crate and the
  newer `tree-sitter-language` bridge dependency. Source:
  <https://docs.rs/crate/tree-sitter-rust/latest>.
- `tree-sitter-python` latest docs show 0.25.0, with a Python grammar crate and
  `tree-sitter-language` bridge dependency. Source:
  <https://docs.rs/crate/tree-sitter-python/latest>.
- `tree-sitter-typescript` latest docs show 0.23.2 and provides distinct
  TypeScript and TSX grammars. It is older than the core parser crate and has a
  larger package footprint, so the implementation should isolate it behind an
  inspector crate and keep it out of `repoctl-core`. Source:
  <https://docs.rs/crate/tree-sitter-typescript/latest>.
- `tree-sitter-language` latest docs show 0.1.7. Its documented purpose is to
  let grammar crates create language instances without tight coupling to the
  exact `tree-sitter` crate version. Source:
  <https://docs.rs/crate/tree-sitter-language/latest>.
- Tree-sitter query docs describe S-expression query patterns, captures,
  alternations, fields, wildcard nodes, and predicates. This supports
  language-specific function and block capture queries instead of hand-written
  AST traversal for every syntax kind. Sources:
  <https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html>,
  <https://tree-sitter.github.io/tree-sitter/using-parsers/queries/2-operators.html>,
  <https://tree-sitter.github.io/tree-sitter/using-parsers/queries/3-predicates-and-directives.html>.
- Tree-sitter advanced parsing docs describe incremental parsing and cheap tree
  copying for concurrency, but individual trees are not thread-safe. The initial
  implementation should use independent parser instances per worker and avoid
  sharing mutable `Tree` instances across threads. Source:
  <https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html>.
- `ignore` latest docs show 0.4.26 and describe a fast recursive iterator that
  respects `.gitignore` and related filters. repoctl already uses `ignore`, so
  code-size scans should reuse the same traversal discipline and update the
  workspace dependency when implementation begins. Source:
  <https://docs.rs/crate/ignore/latest>.
- `memchr` latest docs show 2.8.1 and describe SIMD-accelerated byte search over
  `&[u8]`. It is appropriate for newline counting and cheap prefilters before
  syntax queries. Source: <https://docs.rs/crate/memchr/latest>.
- `rayon` latest docs show 1.12.0 and describe data-parallel iterators with
  data-race freedom. It is a reasonable implementation dependency for
  CPU-bound per-file parsing if a scoped standard-thread worker pool is not
  sufficient. Source: <https://docs.rs/crate/rayon/latest>.

## Design Consequences

- Use Tree-sitter for function and block detection in Rust, TypeScript/TSX, and
  Python. Regex-based function detection should not be used for supported
  languages.
- Keep the parsing implementation in a new inspector crate, not `repoctl-core`.
  `repoctl-core` should only hold validated configuration and report DTOs.
- Use `ignore` for whole-repository and project-root walking, and the existing
  git-range provider for changed-file discovery.
- Use `memchr` for newline counting, byte offsets, cheap file-size prefilters,
  and line-index construction.
- Prefer a bounded parallel worker model. Each worker owns its `Parser`
  instances; shared data should be immutable query/config data.
- Add Tree-sitter grammar dependencies through `[workspace.dependencies]` only
  when the implementation starts. That dependency change must run `cargo audit`
  and `cargo deny check`.
