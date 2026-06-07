# repoctl 开发指南

这份文档写给要修改 `repoctl` 本身的人。重点不是介绍每个命令怎么用，而是说明代码放在哪里、边界怎么守、改完以后该怎么验证。

## 代码结构

```text
apps/repoctl-cli/          命令行入口
crates/repoctl/            对外 facade API
crates/core/               领域类型、诊断、manifest parser、端口 trait
crates/repoctl-engine/     仓库发现、图构建、策略检查
crates/repoctl-scaffold/   init、新项目、模板、skill 同步
crates/repoctl-runner/     affected、任务、CI、proto、IaC、ops、PR 摘要
specs/                     产品、设计、实现、验证规格
docs/                      用户文档和开发文档
```

依赖方向要保持简单：

```text
CLI -> facade -> capability crates -> core ports/domain -> adapters
```

CLI 只负责参数解析、调用 facade、渲染输出、处理退出码。不要把 manifest 解析、图构建、策略判断、脚手架生成或任务执行塞进 CLI。

## 工具链

仓库使用 Rust 2024，版本写在 `rust-toolchain.toml`。

```bash
rustup toolchain install 1.96.0
cargo --version
```

格式化使用 nightly，CI 也运行同一个格式化命令：

```bash
cargo +nightly fmt
```

不要运行 `cargo clean`。

## 常用命令

```bash
make build
make test
cargo clippy -- -D warnings
cargo clippy -- -D warnings -W clippy::pedantic
```

`make test` 调的是 `cargo nextest run --all-features`。如果本机没有 `cargo-nextest`，先按自己的 Rust 工具链习惯安装好。

改依赖、lockfile、license 策略、deny 配置或发布打包相关内容时，还要跑：

```bash
cargo audit
cargo deny check
```

## 怎么选验证范围

不要机械全跑，也不要少跑到看不出问题。按改动面来：

- 改 Rust 源码、公开 API、测试、示例、build script、feature 或 Cargo manifest：跑完整 Rust gate。
- 改依赖和供应链相关文件：加跑 `cargo audit` 和 `cargo deny check`。
- 只改文档：检查 Markdown、链接和示例命令是否正确，不需要跑 Rust 编译。
- 改 AGENTS、CLAUDE 或 skill：跑 `make check-agent-sync`。

行为很局部时，先跑窄一点的测试：

```bash
cargo test -p repoctl-core manifest
cargo test -p repoctl-engine graph
cargo run --bin repoctl -- graph validate --repo <fixture>
```

发现风险扩大，再补更宽的检查。

## 各层应该放什么

`crates/repoctl` 是 facade。它暴露 typed request 和 typed report，供 CLI 或未来其他前端使用。这里不应该返回预先拼好的终端字符串。

几个 capability crate 的职责：

- `repoctl-engine`：发现仓库、构建 graph snapshot、执行策略检查。
- `repoctl-scaffold`：规划文件操作，覆盖 init、新项目、模板、skill。
- `repoctl-runner`：计算 affected、规划或执行任务、生成 CI 数据、检查 codegen、解析 proto owner、生成 PR 摘要、规划 IaC、生成 ops plan、管理 session journal、检查 provider 能力。
- `repoctl-core`：放验证过的领域类型、manifest 解析、诊断模型、DTO 和端口 trait。

新增能力时，先判断这个概念属于哪一层。为了少写几行代码就把逻辑放进 CLI，后面会很难维护。

## manifest 和领域类型

外部输入进入领域模型前必须先验证：

- YAML 先解析到 raw serde struct。
- raw struct 再转成 private-field 的领域类型。
- 名称、owner、路径、glob、命令、集合大小和 byte 长度都在边界处检查。
- 错误尽量带上 source path、稳定 code 和可执行的 help。

当前主要 schema：

- `company.repo/v1`
- `company.project/v1`
- `repoctl.template/v1`

YAML 字段默认严格处理。除非是明确的兼容需求，否则不要悄悄接受未知字段。

`company.project/v1` 里的 ops 相关字段也要在边界处验证。DNS/CDN 意图、HTTP 探针、运行时依赖和手工状态记录都不能保存 secret 值；计划和 journal 里只能出现环境变量名、资源 ID、命令和脱敏后的证据。

## CLI 约定

命令行输出要同时照顾人和自动化：

- 仓库相关命令保留 `--repo`。
- 对外输出优先支持 `--format human|json|github-actions`。
- human 输出要短、清楚、能直接读。
- json 输出序列化 typed report，不要塞终端文本。
- github-actions 只用于确实需要 CI 形状的输出，比如 matrix。

失败时返回诊断，不要 panic。生产代码里不要用 `unwrap()` 或 `expect()`。

## 测试习惯

能贴近实现就写单元测试：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_should_reject_invalid_project_path() {
        // ...
    }
}
```

测试名用 `test_should_` 开头，直接说清楚行为。验证、路径、策略、生成文件计划、错误分支都要有覆盖。跨 facade 或 CLI 的行为，再写更高层的测试。

## 文档和规格

用户文档放 `docs/`，规格放 `specs/`。

新增文档时：

1. 使用指南放到 `docs/guides/`。
2. 调研记录放到 `docs/research/`。
3. 更新 `docs/index.md`。
4. 新规格按 `{feature-name}-{type}.md` 命名，并更新 `specs/index.md`。

中文文档不要照着英文逐句翻译。中文读者更关心实际工作流，就按中文技术文档的习惯重写。

## 发布相关

`CHANGELOG.md` 由 `git cliff` 生成，正常不要手改生成区块。

发布前要确认 workspace 内部依赖既有本地 `path`，也有 crates.io `version`。Cargo 发布时会移除 `path`，只留下版本约束；只有 path 没有 version 的依赖不能发布。

发布 dry-run 按依赖顺序跑：

```bash
cargo publish -p repoctl-core --dry-run --allow-dirty
cargo publish -p repoctl-engine --dry-run --allow-dirty
cargo publish -p repoctl-scaffold --dry-run --allow-dirty
cargo publish -p repoctl-runner --dry-run --allow-dirty
cargo publish -p repoctl-inspect --dry-run --allow-dirty
cargo publish -p repoctl --dry-run --allow-dirty
cargo publish -p repoctl-cli --dry-run --allow-dirty
```

发布命令在 Makefile 里：

```bash
make release
```

只有在明确要发版时才使用。
