# repoctl

`repoctl` 是给功能型 monorepo 用的工程控制面。它关心的不是某一种语言的 workspace，而是整个代码库的工程边界：哪些目录是 app，哪些能力应该放在 framework，哪些服务属于 foundation，proto 归谁维护，IaC 应该在哪儿跑，CI 该测哪些东西，AI agent 能安全改到什么范围。

一句话说，它把仓库整理成一张可验证的项目图。人、CI 和自动化工具都围着这张图工作，少靠约定，多靠检查。

## 它解决什么问题

很多 monorepo 后期会变成这样：Rust、TypeScript、Python、IaC、proto、CI 各管各的，目录结构能跑，但边界说不清。新同学不知道该在哪里加代码，CI 经常全量跑，PR 影响面靠人猜，AI 工具也容易跨目录乱改。

`repoctl` 用两个核心清单把这些信息收回来：

- `repo.yaml`：仓库级规则，包含默认 owner、proto 根目录、IaC 根目录、AI context 输出位置和全局策略。
- `project.yaml`：项目级规则，描述 app、framework、foundation service、proto root 或 core infra 的 owner、workspace、任务、依赖、proto、IaC 和可编辑区域。

有了这些清单，`repoctl` 可以做图校验、边界检查、affected 分析、CI matrix、PR 摘要、模板渲染、proto owner 查询和 AI context 生成。

## 推荐目录

生成出来的仓库按功能分层，不按语言分层：

```text
apps/<app>/
frameworks/<capability>/
foundations/<service>/
protos/
core-infra/
templates/
tools/
```

语言 workspace 放在具体项目里面，例如 `apps/catalog/api/Cargo.toml` 或 `apps/catalog/web/package.json`。根目录不应该再成为某一种语言的全局 workspace。

当前这个仓库本身是 Rust workspace，因为它是在开发 `repoctl` 这个工具，这是实现细节，不代表它初始化出来的业务仓库也要这样组织。

## 本地安装

```bash
git clone https://github.com/tyrchen/repoctl.git
cd repoctl
cargo install --path apps/repoctl-cli
repoctl --help
```

开发时也可以直接运行：

```bash
cargo run --bin repoctl -- --help
```

## 快速上手

初始化一个新仓库：

```bash
repoctl init --name acme --repo ./acme
cd acme
```

创建项目：

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

常用检查：

```bash
repoctl graph validate
repoctl affected --changed-file apps/catalog/api/src/lib.rs --tasks check,test
repoctl ci matrix --tasks check,test,build --format github-actions
```

更多日常用法见 [中文使用指南](guides/user-guide.zh-CN.md)。参与开发这个工具本身，见 [中文开发指南](guides/developer-guide.zh-CN.md)。

## 许可证

MIT，详见 [LICENSE.md](../LICENSE.md)。
