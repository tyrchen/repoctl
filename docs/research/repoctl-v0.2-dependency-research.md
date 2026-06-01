# repoctl v0.2 Dependency Research

Date: 2026-05-31

This note records the dependency and tooling checks used while deriving the v0.2 design, implementation plan, and verification plan from [the initial PRD](../../specs/01-initial-spec.md).

## Inputs Checked

- Rust latest stable is 1.96.0, released on 2026-05-28. Pinning this in `rust-toolchain.toml` satisfies the project rule to use Rust 2024 on latest stable. Source: <https://blog.rust-lang.org/2026/05/28/Rust-1.96.0/>.
- `clap` latest docs show 4.6.1 and support the derive API used for structured subcommands. Source: <https://docs.rs/crate/clap/latest>.
- `cliclack` latest docs show 0.5.4 and provide the prompt primitives needed for interactive CLI flows: text input, select, multi-select, confirm, and intro/outro rendering. Source: <https://docs.rs/cliclack/latest/cliclack/>.
- `minijinja` latest docs show 2.20.0. It remains the right default for the PRD's MiniJinja template system because it is Rust-native and supports sandboxed rendering patterns. Source: <https://docs.rs/crate/minijinja/latest>.
- `serde_yaml` is deprecated and no longer maintained. `serde_yml` has a RustSec advisory for unsoundness and unmaintained status. `serde_norway` is the practical maintained fork to evaluate for YAML manifest parsing, with latest docs showing 0.9.42. Sources: <https://docs.rs/crate/serde_yaml/latest>, <https://rustsec.org/advisories/RUSTSEC-2025-0068>, <https://docs.rs/serde_norway>.
- `camino` latest docs show 1.2.2. It is a good fit for repo-relative UTF-8 path modeling and serde/clap integration. Source: <https://docs.rs/crate/camino/latest>.
- `ignore` latest docs show 0.4.25 and provide a recursive walker that respects `.gitignore`. It should back project and template discovery. Source: <https://docs.rs/crate/ignore/latest>.
- `globset` latest docs show 0.4.18 and is appropriate for many-pattern path matching such as editable areas, generated code policies, and affected rules. Source: <https://docs.rs/crate/globset/latest>.
- `cargo_metadata` latest docs show 0.23.1 and is the direct source for Cargo workspace/package metadata. Source: <https://docs.rs/crate/cargo_metadata/latest>.
- `guppy` latest docs show 0.17.25 and provides higher-level Cargo dependency graph queries over `cargo metadata`. It should be evaluated once Rust workspace graph edges need feature/platform accuracy. Source: <https://docs.rs/guppy/latest/guppy/>.
- `petgraph` latest docs show 0.8.3 and remains a viable general-purpose graph representation for repoctl's cross-language project graph. Source: <https://docs.rs/crate/petgraph/latest>.
- `schemars` latest docs show 1.2.1 and `jsonschema` latest docs show 0.46.5. These should be used together for generated schemas and schema validation when repoctl emits or verifies manifest schemas. Sources: <https://docs.rs/crate/schemars/latest>, <https://docs.rs/crate/jsonschema/latest>.
- `winnow` latest docs show 1.0.3 and is appropriate where repoctl needs a small grammar parser, such as project names, workspace selectors, and maybe task command tokenization if a simpler argv form is not enough. Source: <https://docs.rs/winnow/latest/winnow/>.
- `typed-builder` latest docs show 0.23.2. Use it selectively for configuration structs with many fields, not for every domain type. Source: <https://docs.rs/crate/typed-builder/latest>.
- `validator` latest docs show 0.20.0. Prefer domain newtypes with fallible constructors first, and use `validator` where serde-bound struct validation adds clarity. Source: <https://docs.rs/crate/validator/latest>.

## Design Consequences

- Use `serde_norway`, not `serde_yaml` or `serde_yml`, for YAML parsing unless a later audit finds a better maintained YAML serde option.
- Keep project identifiers, owner handles, task names, workspace names, and repo-relative paths as validated newtypes. Do not leave validation to downstream command handlers.
- Execute repo-defined tasks with argv-form process spawning. The PRD shows command strings for readability, but the implementation should parse those into argv and reject shell-only constructs in v0.2 instead of invoking `sh -c`.
- Use `ignore` and `globset` for all file traversal and policy matching, with explicit caps on file size, manifest count, pattern count, and traversal roots.
- Model the repo graph as a domain graph first. Use Cargo-specific tooling only inside Rust workspace adapters; the cross-language graph cannot be Cargo-shaped.
- Builtin and local templates are enough for v0.2. External Git templates should stay behind a later trust model that includes pinned refs and checksums.
