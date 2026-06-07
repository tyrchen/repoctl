# repoctl 使用指南

这份指南面向在业务仓库里使用 `repoctl` 的开发者。你不需要理解内部 Rust 实现，只要知道它如何帮你创建项目、检查边界、判断 PR 影响面、生成 CI matrix、规划运维变更，以及给 AI agent 准备合适的上下文。

## 先理解项目类型

`repoctl` 看仓库时，不会先问“这是 Rust 还是 TypeScript 项目”，而是先问“这个目录在业务上是什么”。

常见项目类型有：

- app：面向产品或业务场景的应用，通常有 API、web、jobs、测试、部署配置和 app 自己的 IaC。
- framework：从多个 app 中沉淀出来的复用能力。对外暴露 facade，内部实现不鼓励被其他项目直接引用。
- foundation service：公司级基础服务，通常会拥有 proto、服务实现和多语言 client。
- proto root：源 proto 的归属地。生成代码是产物，不应该手改。
- core infra：共享基础设施，和 app 自己的 IaC 分开管理。

这些信息写在 `repo.yaml` 和各个 `project.yaml` 里。`repoctl` 读取它们后构建项目图，再基于这张图做校验、分析和生成。

## 初始化仓库

```bash
repoctl init --name acme --repo ./acme
cd acme
```

想先看会生成哪些文件，可以加 `--dry-run`：

```bash
repoctl init --name acme --repo ./acme --dry-run
```

目前初始化支持 functional layout，也就是顶层按功能目录划分：

```text
apps/
frameworks/
foundations/
protos/
core-infra/
templates/
tools/
```

## 创建项目

创建 app：

```bash
repoctl new app catalog \
  --stack rust-api,bun-web \
  --iac pulumi \
  --owner @catalog
```

这里的 `catalog` 会被放到 `apps/catalog`。如果你想写完整路径也可以：

```bash
repoctl new app apps/catalog --owner @catalog
```

创建 framework：

```bash
repoctl new framework runtime \
  --languages rust,typescript \
  --facade \
  --owner @platform
```

创建 foundation service：

```bash
repoctl new foundation identity \
  --clients rust,typescript \
  --proto company.identity.v1 \
  --owner @identity
```

人在终端里使用时，缺少项目名会进入交互式提示。CI 或脚本里不要依赖交互，参数要写完整。

## 每次改结构后先校验

改了 `repo.yaml`、`project.yaml`、项目依赖、proto 归属、生成代码策略或目录结构后，先跑：

```bash
repoctl graph validate
```

如果只想针对某些文件检查：

```bash
repoctl graph validate \
  --changed-file apps/catalog/api/src/lib.rs \
  --changed-file protos/company/identity/v1/user.proto
```

查看整张图：

```bash
repoctl graph print
```

查看某个项目为什么出现在图里、它连着哪些边：

```bash
repoctl explain apps.catalog
```

## 查 PR 影响面

最常见的用法是拿当前分支和主干比较：

```bash
repoctl affected \
  --base origin/main \
  --head HEAD \
  --tasks check,test
```

也可以直接指定文件，适合调试或在 CI 中接入：

```bash
repoctl affected \
  --changed-file apps/catalog/api/src/lib.rs \
  --tasks check,test
```

输出里会包含直接受影响的项目、传递影响的项目、workspace、任务、建议 reviewer、触发原因、风险标记和诊断信息。

## 跑任务前先看计划

项目任务来自 `project.yaml`。不确定会跑什么时，先 dry-run：

```bash
repoctl run check --affected --dry-run
```

只跑某个项目：

```bash
repoctl run test --project apps.catalog
```

只跑某个 workspace：

```bash
repoctl run build --workspace apps.catalog:api
```

任务比较重时限制并发：

```bash
repoctl run test --affected --concurrency 4
```

## 给 CI 准备 matrix

本地看结果：

```bash
repoctl ci matrix --tasks check,test,build
```

GitHub Actions 中通常需要专门的 matrix JSON：

```bash
repoctl ci matrix \
  --base origin/main \
  --head HEAD \
  --tasks check,test,build \
  --format github-actions
```

## proto、生成代码和 IaC

查 proto owner：

```bash
repoctl proto owners company.identity.v1
```

查谁消费了某个 proto：

```bash
repoctl proto consumers protos/company/identity/v1/user.proto
```

检查生成代码有没有被直接修改：

```bash
repoctl codegen check --base origin/main --head HEAD
repoctl proto check --base origin/main --head HEAD
```

规划 IaC 命令：

```bash
repoctl iac plan --affected --env staging
repoctl iac preview --affected --env staging
repoctl iac plan --project apps.catalog --env prod
repoctl iac plan --core --env prod
```

`repoctl iac plan` 只做计划和风险提示，不执行 apply。`repoctl iac preview` 是同一条预览规划路径的别名。

## 规划运维变更

如果一个改动同时影响 IaC、DNS、CDN、共享 framework、应用栈和上线验证，用 `repoctl ops plan` 先生成计划：

```bash
repoctl ops plan \
  --base origin/main \
  --head HEAD \
  --env staging \
  --tasks check,test,build \
  --output target/repoctl/ops-plan.json
```

ops plan 会列出 affected 项目、去重后的任务 dry-run、IaC preview 顺序、DNS/CDN 检查、provider 能力诊断、HTTP 探针、运行时依赖探针、手工状态清理项，以及需要的环境变量名字。它不会默认执行 apply。

根据保存的计划生成非破坏性验证命令：

```bash
repoctl ops verify --plan target/repoctl/ops-plan.json
```

长时间排障或上线时，可以把证据写进本地 journal：

```bash
repoctl ops journal start --name staging-dns-cutover
repoctl ops journal add-command --session staging-dns-cutover -- repoctl graph validate
repoctl ops journal add-note --session staging-dns-cutover --kind finding --message "Cloudflare records are DNS-only"
repoctl ops journal summary --session staging-dns-cutover
```

journal 放在 `target/repoctl/sessions/`，看起来像 token、cookie、Authorization header、API key 或 secret 的内容会被脱敏。

检查 provider 能力，避免为了一个字段盲目升级大版本：

```bash
repoctl provider capabilities --workspace frameworks.operon:infra
```

## 给 AI agent 准备上下文

```bash
repoctl context apps.catalog --for ai --format json
```

这类上下文应该是项目级的，不是把整个仓库一股脑塞进去。边界越清楚，自动化工具越不容易改错地方。

生成 PR 摘要：

```bash
repoctl pr summary --base origin/main --head HEAD
```

摘要会把影响项目、风险、reviewer、建议检查项和运维影响面集中起来，适合贴到 PR 描述或 CI 注释里。

## 输出格式怎么选

日常终端使用：

```bash
repoctl graph validate --format human
```

脚本读取：

```bash
repoctl affected --format json
```

GitHub Actions matrix：

```bash
repoctl ci matrix --tasks check,test --format github-actions
```

## 一个实用流程

普通 PR 可以按这个顺序走：

1. 在正确的 app、framework 或 foundation 里修改。
2. 跑 `repoctl graph validate`。
3. 跑 `repoctl affected --base origin/main --head HEAD --tasks check,test`。
4. 用 `repoctl run <task> --affected` 跑必要任务。
5. 如果涉及 IaC、DNS、CDN 或上线验证，再跑 `repoctl ops plan --base origin/main --head HEAD --env staging --tasks check,test,build`。
6. 用 `repoctl pr summary` 整理影响面。

如果前面的图校验失败，先修图和清单。后面的 affected、CI、PR 摘要都依赖一张正确的项目图。
