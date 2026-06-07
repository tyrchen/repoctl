# repoctl Developer Guide

This guide is for people changing `repoctl` itself. It explains the repository shape, crate responsibilities, development commands, validation expectations, and documentation conventions.

## Workspace structure

```text
apps/repoctl-cli/          CLI frontend
crates/repoctl/            public facade API
crates/core/               domain model, diagnostics, manifest parser, ports
crates/repoctl-engine/     discovery, graph construction, policy evaluation
crates/repoctl-scaffold/   init, project scaffolding, templates, skill sync
crates/repoctl-runner/     affected analysis, task planning, CI, proto, IaC, ops
specs/                     product, design, implementation, verification specs
docs/                      user-facing and contributor documentation
```

Dependency direction should stay one way:

```text
CLI -> facade -> capability crates -> core ports/domain -> adapters
```

The CLI parses flags, calls the facade, renders reports, and maps diagnostics to exit codes. It should not grow manifest parsing, graph building, policy logic, scaffolding, or task execution logic.

## Rust toolchain

The project uses Rust 2024 and pins the stable toolchain in `rust-toolchain.toml`.

```bash
rustup toolchain install 1.96.0
cargo --version
```

Use the pinned toolchain by default. Formatting is run with nightly because the repository policy requires `cargo +nightly fmt`.

## Common commands

```bash
make build
make test
cargo +nightly fmt
cargo clippy -- -D warnings
cargo clippy -- -D warnings -W clippy::pedantic
```

`make test` runs `cargo nextest run --all-features`. If `cargo-nextest` is missing, install it with your normal Rust tooling before relying on the target.

Use supply-chain checks when dependency or release packaging files change:

```bash
cargo audit
cargo deny check
```

Do not run `cargo clean`.

## Scoped verification

Choose checks based on the files you touched:

- Rust source, public Rust APIs, examples, tests, build scripts, feature flags, or Cargo manifests: run the full Rust gate set.
- Dependency, lockfile, license policy, deny policy, or release packaging changes: also run `cargo audit` and `cargo deny check`.
- Documentation-only changes: proofread the rendered Markdown shape, check touched links, and skip Rust builds unless the docs include generated code or doctested examples.
- Agent instruction or skill changes: run `make check-agent-sync`.

Start narrow when a behavior question is localized:

```bash
cargo test -p repoctl-core manifest
cargo test -p repoctl-engine graph
cargo run --bin repoctl -- graph validate --repo <fixture>
```

Broaden only when the result suggests wider risk.

## Facade and service boundaries

`crates/repoctl` is the public API consumed by the CLI and future frontends. It should return typed domain reports instead of preformatted strings.

Capability crates own behavior:

- `repoctl-engine` discovers repositories, builds graph snapshots, and evaluates policies.
- `repoctl-scaffold` plans file operations for init, new project, templates, and skills.
- `repoctl-runner` computes affected work, plans or runs tasks, builds CI data, checks codegen policy, resolves proto ownership, builds PR summaries, plans IaC commands, builds operations plans, manages session journals, and inspects provider capabilities.
- `repoctl-core` owns validated primitives, manifest parsing, diagnostics, request/response DTOs, and traits.

When adding behavior, put it at the lowest layer that owns the concept. Avoid pushing business logic into the CLI for convenience.

## Domain and manifest rules

External input should be validated before it enters the domain model:

- Parse YAML into raw serde structs.
- Convert raw values into private-field domain types through fallible constructors.
- Enforce byte limits, allowed characters, path traversal prevention, collection limits, and numeric ranges at the boundary.
- Return `Diagnostic` or `RepoctlError` with source path and actionable help where possible.

Manifest schemas currently center on:

- `company.repo/v1`
- `company.project/v1`
- `repoctl.template/v1`

Keep YAML field names stable and strict. Unknown fields should remain errors unless backward compatibility requires an alias or default.

Operations metadata is part of `company.project/v1`. DNS, CDN, probes, runtime dependencies, and manual-state records are parsed as typed domain values and should remain non-secret. Store only environment variable names, resource identifiers, commands, and selected evidence; never store token or cookie values in plans or journals.

## CLI behavior

Every command should support predictable automation:

- Keep `--repo` available for repository-scoped commands.
- Prefer `--format human|json|github-actions` where output is consumed externally.
- Human output should be readable and concise.
- JSON output should serialize typed reports, not terminal text.
- GitHub Actions output should be used only when the command has a CI-specific payload.

For fallible operations, return diagnostics instead of panicking. Production code must not use `unwrap()` or `expect()`.

## Tests

Use unit tests close to the behavior when possible:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_should_reject_invalid_project_path() {
        // ...
    }
}
```

Test names should start with `test_should_` and describe the behavior. Cover error cases explicitly, especially validation, path handling, policy violations, and generated operations.

Use integration-style tests when a facade or CLI path is the behavior under test. Keep fixtures small and focused.

## Documentation and specs

Project documentation lives under `docs/`; specs live under `specs/`.

When adding docs:

1. Put user-facing guides under `docs/guides/`.
2. Put research notes under `docs/research/`.
3. Update `docs/index.md`.
4. For specs, use `{feature-name}-{type}.md` and update `specs/index.md`.

For Chinese docs, write naturally for Chinese readers. Do not produce sentence-by-sentence translations of the English files.

## Release notes

`CHANGELOG.md` is generated by `git cliff`. Do not hand-edit generated release sections unless release tooling requires a temporary correction.

Before publishing, verify that publishable workspace dependencies include both local `path` and crates.io `version` requirements. A local path-only dependency will fail packaging because Cargo strips path information for crates.io.

Run package dry-runs in dependency order when changing publish metadata:

```bash
cargo publish -p repoctl-core --dry-run --allow-dirty
cargo publish -p repoctl-engine --dry-run --allow-dirty
cargo publish -p repoctl-scaffold --dry-run --allow-dirty
cargo publish -p repoctl-runner --dry-run --allow-dirty
cargo publish -p repoctl --dry-run --allow-dirty
cargo publish -p repoctl-cli --dry-run --allow-dirty
```

The repository root also supports the default package dry-run:

```bash
cargo publish --dry-run --allow-dirty
```

Release automation is exposed through the Makefile:

```bash
make release
```

Use it only when intentionally cutting a release.
