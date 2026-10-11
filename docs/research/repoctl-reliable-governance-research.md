# Reliable governance dependency and product research

Checked 2026-10-10 after reading the existing v0.2/v0.5 research notes.

## Product decisions

- [Nx caching](https://nx.dev/docs/concepts/how-caching-works) describes hashing
  inputs and restoring outputs/logs. repoctl adopts explicit task prerequisites and
  input/output contracts with a conservative local cache, rather than adding Nx
  as a required dependency or building remote caching in this change.
- [Buf breaking](https://buf.build/docs/breaking/usage/) compares a schema with an
  explicit source. repoctl delegates rule semantics to Buf and adds bounded argv
  planning/execution with ownership/consumer information.
- Multilingual governance and agent boundaries remain the product center. Package
  managers execute tasks; repoctl derives one model used by local commands and CI.

## Implementation dependencies and maintained resources

- [BLAKE3 1.8.7](https://docs.rs/blake3/1.8.7/blake3/) supplies streaming deterministic
  content hashes. The safe Rust API is used without project unsafe code. Its license
  and dependency tree are covered by `cargo deny check` and `cargo audit`.
- [Tokio process](https://docs.rs/tokio/1.53.2/tokio/process/struct.Child.html)
  documents child waiting, killing and kill-on-drop. Explicit process/io-util/time
  features support bounded capture; scoped standard workers serve the synchronous
  facade. No blocking filesystem work runs inside an async request server.
- [Rust 1.99](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/) is the current
  stable release; pin 1.99.0 and retain edition 2024.
- [checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1) and
  [setup-node v7.1.0](https://github.com/actions/setup-node/releases/tag/v7.1.0)
  are current major releases. Generated workflows use v7; the source workflow's
  checkout is updated too.
- [setup-uv v10.3.0](https://github.com/astral-sh/setup-uv/releases/tag/v10.3.0)
  supplies Python/uv setup; generated workflows use v10.
- [Buf 1.73.0](https://github.com/bufbuild/buf/releases/tag/v1.73.0) is tested against
  real local schema fixtures. The old buf-setup-action is archived; use maintained
  [buf-action](https://github.com/bufbuild/buf-action) with `setup_only: true` so
  installing the CLI does not publish schemas or post PR comments.
- [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)
  supports explicit job result aggregation. Structural tests and shell execution
  verify empty/success/failed/cancelled result handling.

## Supply-chain fixes

Audit found existing affected versions of anyhow and crossbeam-epoch. Resolve
anyhow 1.0.104 and crossbeam-epoch 0.9.21, without changing the advisory policy:

- [RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190.html): anyhow
  downcast_mut unsoundness, fixed from 1.0.103.
- [RUSTSEC-2026-0204](https://rustsec.org/advisories/RUSTSEC-2026-0204.html): invalid
  pointer formatting in crossbeam-epoch, fixed from 0.9.20.
