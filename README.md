# repoctl

`repoctl` is a graph-aware control plane for functional monorepos. It gives a repository one validated model for humans, CI, and AI agents to share: project boundaries, workspace discovery, dependency graph, affected tasks, CI matrices, PR impact, templates, proto ownership, IaC routing, and agent context.

The project is still early, but the core workflow is already present behind a typed Rust facade and a CLI named `repoctl`.

## Why repoctl exists

Large monorepos become hard to operate when every tool sees a different shape. Package managers know language workspaces, CI knows job matrices, platform teams know deployment and IaC, and AI agents need local boundaries before they can make safe edits.

`repoctl` keeps those concerns connected through repo-level and project-level manifests:

- `repo.yaml` defines repo layout, defaults, proto roots, IaC roots, AI context paths, and global policies.
- `project.yaml` defines one functional project: app, framework, foundation service, proto root, or core infrastructure.
- The generated graph powers validation, affected analysis, task planning, CI data, PR summaries, and scoped AI context.

## Repository layout

`repoctl` manages functional monorepos. Generated repositories are organized by product and capability instead of by language:

```text
apps/<app>/
frameworks/<capability>/
foundations/<service>/
protos/
core-infra/
templates/
tools/
```

Language workspaces live inside those functional projects, for example `apps/catalog/api/Cargo.toml` or `apps/catalog/web/package.json`. A generated repository should not rely on root-level language workspaces such as a root `Cargo.toml`, `package.json`, or `pyproject.toml`.

This source repository is a Rust workspace because it builds the tool itself.

## Install from source

```bash
git clone https://github.com/tyrchen/repoctl.git
cd repoctl
cargo install --path apps/repoctl-cli
repoctl --help
```

For local development without installing:

```bash
cargo run --bin repoctl -- --help
```

## Quick start

Create a new functional monorepo:

```bash
repoctl init --name acme --repo ./acme
cd acme
```

Create projects:

```bash
repoctl new app catalog \
  --stack rust-api,bun-web \
  --iac pulumi \
  --owner @catalog

repoctl new framework runtime \
  --languages rust,typescript \
  --facade \
  --owner @platform

repoctl new foundation identity \
  --clients rust,typescript \
  --proto company.identity.v1 \
  --owner @identity
```

Validate and inspect the graph:

```bash
repoctl graph validate
repoctl graph print
repoctl explain apps.catalog
```

Plan affected work:

```bash
repoctl affected \
  --changed-file apps/catalog/api/src/lib.rs \
  --tasks check,test

repoctl run check --affected --dry-run
```

Generate CI data:

```bash
repoctl ci matrix \
  --tasks check,test,build \
  --format github-actions
```

## Main command groups

| Command | Purpose |
| --- | --- |
| `repoctl init` | Create a functional monorepo skeleton. |
| `repoctl new` | Scaffold app, framework, and foundation projects. |
| `repoctl graph` | Validate and print the repository graph. |
| `repoctl explain` | Explain a project or graph node and its edges. |
| `repoctl lint-boundaries` | Check graph and path boundary policies. |
| `repoctl affected` | Compute affected projects, tasks, workspaces, reviewers, and risk flags. |
| `repoctl run` | Plan or execute manifest-defined tasks. |
| `repoctl ci` | Build CI matrix output, including GitHub Actions output. |
| `repoctl template` | List and render built-in or local templates. |
| `repoctl codegen` | Detect direct edits to generated code. |
| `repoctl proto` | Query proto owners and consumers, and check proto policy. |
| `repoctl context` | Build project-scoped AI context. |
| `repoctl pr` | Build PR impact summaries. |
| `repoctl iac` | Plan IaC provider commands without applying changes. |
| `repoctl skills` | Check or synchronize generated agent skills. |

Most commands accept `--repo` and `--format human|json|github-actions`. Human output is for local use. JSON output is for automation. GitHub Actions output is specialized where CI needs a matrix-shaped payload.

## Documentation

- [User guide](docs/guides/user-guide.md)
- [Developer guide](docs/guides/developer-guide.md)
- [中文 README](docs/README.zh-CN.md)
- [中文使用指南](docs/guides/user-guide.zh-CN.md)
- [中文开发指南](docs/guides/developer-guide.zh-CN.md)
- [Docs index](docs/index.md)
- [Specs index](specs/index.md)

## Developing repoctl

This repository uses Rust 2024 and the pinned toolchain in `rust-toolchain.toml`.

Useful commands:

```bash
make build
make test
cargo +nightly fmt
cargo clippy -- -D warnings
```

Run the smallest meaningful check for the surface you changed. Documentation-only changes should be proofread and link-checked instead of running the full Rust gate set mechanically.

## License

This project is distributed under the terms of MIT. See [LICENSE.md](LICENSE.md).

Copyright 2025 Tyr Chen
