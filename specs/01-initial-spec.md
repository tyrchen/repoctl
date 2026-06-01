# repoctl v0.2 PRD

## 1. 产品定位

`repoctl` 是 monorepo 的 **control plane**。

它不是一个通用脚手架工具，也不是 CI/CD 系统，更不是 Cargo / Bun / uv / Pulumi / Terraform 的替代品。

它负责统一管理：

```text
repo 结构
project manifest
workspace discovery
project graph
affected analysis
task execution
CI matrix
PR impact summary
AI context
skills
templates
proto ownership
infra boundary
app creation
framework extraction path
```

一句话：

> `repoctl` 让 monorepo 变成一个可以被人、CI、AI agent 共同理解和安全操作的工程系统。

---

## 2. v0.2 核心变化

上一版把 `rust/`、`ts/` 作为顶层目录，并在根目录放了 Cargo workspace / pnpm workspace。这一版改掉。

新的原则是：

```text
顶层按功能划分，不按语言划分。
语言 workspace 属于具体功能模块。
app 是一等公民。
framework 是从 app 中抽取出来的复用能力。
foundation service 是公司级基础业务服务。
core infra 是公司共享基础设施。
```

所以不再有：

```text
/rust
/ts
Cargo.toml
pnpm-workspace.yaml
```

而是：

```text
/apps/<app>/api/Cargo.toml
/apps/<app>/web/package.json
/apps/<app>/jobs/pyproject.toml

/frameworks/<capability>/rust/Cargo.toml
/frameworks/<capability>/ts/package.json
/frameworks/<capability>/python/pyproject.toml

/foundations/<service>/service/Cargo.toml
/foundations/<service>/clients/ts/package.json
/foundations/<service>/clients/python/pyproject.toml
```

根目录可以有：

```text
target/
```

但它只是构建产物和 cache 的集中位置，不是语言 workspace。

---

## 3. 产品目标

## 3.1 第一目标：让 app 好维护

每个 app 应该是一个局部完整的工程单元。

一个 app 内部包含：

```text
API
web
workers
jobs
local packages
tests
IaC
deploy
docs
project.yaml
AGENTS.md
```

开发者进入 `apps/billing`，应该能理解这个 app 的完整边界。

AI agent 进入 `apps/billing`，也应该知道：

```text
可以改哪里
不能改哪里
本地怎么测试
依赖哪些 framework facade
消费哪些 proto
IaC 在哪里
review 风险是什么
```

---

## 3.2 第二目标：让新 app 好创建

创建一个新 app 应该是一条命令：

```bash
repoctl new app apps/billing \
  --stack rust-api,bun-web,uv-jobs \
  --iac pulumi
```

它应该生成：

```text
apps/billing/
  project.yaml
  AGENTS.md
  README.md
  api/
  web/
  jobs/
  iac/
  deploy/
  docs/
  tests/
```

而且一开始就接好：

```text
framework facade path dependencies
proto generation
task definitions
CI affected rules
AI context
review checklist
IaC boundary
```

---

## 3.3 第三目标：让核心能力容易抽取

不要一开始把所有东西都抽成 framework。

更好的路径是：

```text
先 app-local 实现
两个 app 重复后抽到 framework
多个 app 依赖后收敛为 facade
稳定后沉淀成标准 template
```

`repoctl` 要鼓励这种演进方式。

例如：

```text
apps/app-a/api/crates/local-auth
apps/app-b/api/crates/local-auth
```

后续可以抽取为：

```text
frameworks/auth-client/rust/crates/auth-client
frameworks/auth-client/ts/packages/auth-client
frameworks/auth-client/python/packages/auth-client
```

然后 app 只依赖 facade。

---

## 3.4 第四目标：让代码库朝好的方向发展

`repoctl` 要通过 graph、policy 和 template 把 repo 往正确方向推。

它要防止：

```text
app 之间互相 import
framework 依赖 app
foundation service 随意依赖业务 app
generated code 被手改
proto ownership 混乱
core infra 和 app infra 混在一起
CI 全量跑
AI agent 跨边界乱改
```

它要鼓励：

```text
app 边界清晰
framework facade 稳定
proto source 集中
IaC ownership 清晰
task 可复用
PR 影响范围可见
AI context 精准
```

---

# 4. 顶层目录结构

推荐标准 layout：

```text
.
├── README.md
├── AGENTS.md
├── repo.yaml
├── justfile
├── .gitignore
├── .gitattributes
├── .editorconfig
│
├── .repoctl/
│   ├── init-state.json
│   ├── cache/
│   ├── templates/
│   └── schemas/
│
├── .agents/
│   └── skills/
│       ├── repoctl/
│       ├── monorepo-boundaries/
│       ├── proto-change/
│       ├── pr-review/
│       └── app-creation/
│
├── .claude/
│   └── skills/
│       ├── repoctl/
│       ├── monorepo-boundaries/
│       ├── proto-change/
│       ├── pr-review/
│       └── app-creation/
│
├── .github/
│   ├── CODEOWNERS
│   ├── pull_request_template.md
│   └── workflows/
│       ├── ci.yml
│       ├── proto.yml
│       ├── iac.yml
│       ├── pr-summary.yml
│       └── nightly.yml
│
├── target/
│   ├── rust/
│   ├── bun/
│   ├── uv/
│   └── repoctl/
│
├── protos/
│   ├── buf.yaml
│   ├── buf.gen.yaml
│   ├── project.yaml
│   └── acme/
│       ├── identity/v1/
│       ├── authz/v1/
│       ├── experiment/v1/
│       ├── analytics/v1/
│       └── app_a/v1/
│
├── apps/
│   ├── app-a/
│   │   ├── project.yaml
│   │   ├── AGENTS.md
│   │   ├── README.md
│   │   ├── api/
│   │   │   ├── Cargo.toml
│   │   │   ├── Cargo.lock
│   │   │   └── crates/
│   │   ├── web/
│   │   │   ├── package.json
│   │   │   ├── bun.lock
│   │   │   └── src/
│   │   ├── jobs/
│   │   │   ├── pyproject.toml
│   │   │   ├── uv.lock
│   │   │   └── src/
│   │   ├── iac/
│   │   │   ├── Pulumi.yaml
│   │   │   └── stacks/
│   │   ├── deploy/
│   │   ├── docs/
│   │   └── tests/
│   └── app-b/
│
├── foundations/
│   ├── identity/
│   │   ├── project.yaml
│   │   ├── AGENTS.md
│   │   ├── service/
│   │   │   ├── Cargo.toml
│   │   │   └── crates/
│   │   ├── clients/
│   │   │   ├── rust/
│   │   │   ├── ts/
│   │   │   └── python/
│   │   ├── iac/
│   │   ├── deploy/
│   │   ├── docs/
│   │   └── runbooks/
│   ├── authz/
│   ├── experiment/
│   ├── analytics/
│   └── billing/
│
├── frameworks/
│   ├── service-runtime/
│   │   ├── project.yaml
│   │   ├── AGENTS.md
│   │   ├── rust/
│   │   │   ├── Cargo.toml
│   │   │   ├── Cargo.lock
│   │   │   └── crates/
│   │   │       ├── service-runtime/
│   │   │       ├── service-runtime-facade/
│   │   │       └── service-runtime-internal/
│   │   ├── ts/
│   │   │   ├── package.json
│   │   │   ├── bun.lock
│   │   │   └── packages/
│   │   ├── python/
│   │   │   ├── pyproject.toml
│   │   │   ├── uv.lock
│   │   │   └── packages/
│   │   └── docs/
│   ├── observability/
│   ├── config/
│   ├── auth-client/
│   ├── experiment-client/
│   ├── web-runtime/
│   └── testing/
│
├── core-infra/
│   ├── project.yaml
│   ├── AGENTS.md
│   ├── org/
│   ├── projects/
│   ├── iam/
│   ├── network/
│   ├── dns/
│   ├── certs/
│   ├── secrets/
│   ├── observability/
│   ├── artifact-registry/
│   ├── policy/
│   ├── modules/
│   └── environments/
│       ├── dev/
│       ├── staging/
│       └── prod/
│
├── templates/
│   ├── app-fullstack/
│   ├── rust-api/
│   ├── bun-web/
│   ├── uv-job/
│   ├── foundation-service/
│   ├── framework-capability/
│   └── core-infra-module/
│
├── tools/
│   ├── repoctl/
│   │   ├── Cargo.toml
│   │   └── crates/
│   ├── ci/
│   └── dev/
│
└── docs/
    ├── adr/
    ├── architecture/
    ├── engineering/
    ├── security/
    └── runbooks/
```

---

# 5. 关键目录解释

## 5.1 apps/

`apps/` 是产品或业务应用。

每个 app 是独立维护单元。

一个 app 可以有多种语言和多个 workspace：

```text
api/      Rust workspace
web/      Bun workspace
jobs/     uv workspace
iac/      app-local IaC
deploy/   app-local deploy intent
docs/     app-local docs
tests/    app-level tests
```

app 可以依赖：

```text
framework facade
foundation service client
protos
core infra module outputs
```

app 不应该依赖：

```text
另一个 app 的代码
framework internal crate
foundation service 内部实现
core-infra 私有模块
```

---

## 5.2 foundations/

`foundations/` 放公司级基础业务服务。

例如：

```text
identity
authz
experiment
analytics
billing
notification
audit-log
scheduler
```

foundation service 和 framework 不同。

foundation service 是独立运行的服务，有 API、数据、部署、SLO、runbook。

framework 是被 app 编译或 import 的库 / package / SDK。

---

## 5.3 frameworks/

`frameworks/` 放跨 app 复用的工程能力。

例如：

```text
service-runtime
web-runtime
observability
config
auth-client
experiment-client
testing
data-access
workflow
```

framework 可以内部有多语言实现，但顶层按能力组织，不按语言组织。

例如：

```text
frameworks/service-runtime/
  rust/
  ts/
  python/
```

而不是：

```text
rust/frameworks/service-runtime
ts/frameworks/service-runtime
python/frameworks/service-runtime
```

---

## 5.4 core-infra/

`core-infra/` 放公司共享基础设施。

适合放在这里的内容：

```text
org / folder / project bootstrap
IAM baseline
network
DNS
certs
secret policy
artifact registry
observability baseline
security policy
shared infra modules
environment baseline
```

不适合放在这里的内容：

```text
某个 app 的 Cloud Run / GKE / DB / topic / bucket
某个 app 的 deploy config
某个 app 的 runtime env vars
某个 app 的 app-specific permission
```

这些应该放到：

```text
apps/<app>/iac/
foundations/<service>/iac/
```

---

## 5.5 protos/

全系统 proto source 集中在：

```text
protos/
```

这是系统 API contract 的 source of truth。

推荐结构：

```text
protos/
  buf.yaml
  buf.gen.yaml
  project.yaml
  acme/
    identity/v1/identity.proto
    authz/v1/authz.proto
    experiment/v1/experiment.proto
    analytics/v1/events.proto
    app_a/v1/app_a.proto
```

source proto 集中放置，但 ownership 可以细分。

例如：

```yaml
# protos/project.yaml
schema: company.project/v1
name: protos
kind: proto-root
path: protos
owners:
  - "@api-review"

proto_packages:
  - path: acme/identity/**
    owner: "@identity"
    consumers:
      - foundations.identity
      - apps.app-a
  - path: acme/experiment/**
    owner: "@experiment"
    consumers:
      - foundations.experiment
      - apps.app-a
      - apps.app-b
```

---

# 6. Workspace 策略

## 6.1 根目录不放全局语言 workspace

根目录不放：

```text
Cargo.toml
package.json
pnpm-workspace.yaml
pyproject.toml
uv.lock
```

原因：

```text
全局 workspace 容易把所有东西耦合在一起。
所有改动都会影响全局 lock。
根 workspace 会让 CI 粒度变粗。
app 的 ownership 被稀释。
framework 内部实验会影响业务 app。
```

每个功能模块自己拥有 workspace。

---

## 6.2 Rust workspace

每个 Rust-bearing project 自己有 `Cargo.toml`。

例如 app API：

```text
apps/catalog/api/
  Cargo.toml
  Cargo.lock
  crates/
    catalog-api/
    catalog-domain/
    catalog-testkit/
```

示例：

```toml
[workspace]
resolver = "3"
members = [
  "crates/catalog-api",
  "crates/catalog-domain",
  "crates/catalog-testkit",
]

[workspace.package]
edition = "2024"
rust-version = "1.85"

[workspace.dependencies]
service-runtime = { path = "../../../frameworks/service-runtime/rust/crates/service-runtime-facade" }
observability = { path = "../../../frameworks/observability/rust/crates/observability-facade" }
identity-client = { path = "../../../foundations/identity/clients/rust/crates/identity-client" }
```

关键规则：

```text
app 可以 path 引用 framework facade crate
app 不可以 path 引用 framework internal crate
foundation service client 应该通过 client crate 暴露
framework facade crate 是稳定边界
framework internal crate 可以快速迭代
```

---

## 6.3 Rust target 目录

虽然没有根 `Cargo.toml`，但可以把 build target 放在根目录：

```text
target/rust/
```

`repoctl` 在执行 Rust task 时注入：

```bash
CARGO_TARGET_DIR=$REPO_ROOT/target/rust
```

这样可以：

```text
保留每个 workspace 的独立性
减少重复构建缓存
避免根 Cargo workspace 带来的耦合
让 CI cache 更稳定
```

如果未来遇到 target 污染或锁竞争，可以切换成：

```text
target/rust/<workspace-hash>/
```

这个策略由 `repo.yaml` 控制。

---

## 6.4 TypeScript 使用 Bun

每个 TS-bearing project 自己有 Bun workspace。

例如：

```text
apps/catalog/web/
  package.json
  bun.lock
  src/
  packages/
    catalog-ui/
    catalog-client/
```

示例：

```json
{
  "name": "@acme/catalog-web-workspace",
  "private": true,
  "workspaces": [
    "packages/*"
  ],
  "scripts": {
    "dev": "bun run src/main.tsx",
    "check": "bunx tsc --noEmit",
    "test": "bun test",
    "build": "bun run build.ts"
  },
  "dependencies": {
    "@acme/web-runtime": "workspace:*",
    "@acme/experiment-client": "workspace:*"
  }
}
```

对跨 project 的共享 TS package，不建议用一个全局 TS workspace 统一管理。

更推荐：

```text
frameworks/web-runtime/ts/package.json
frameworks/experiment-client/ts/package.json
foundations/identity/clients/ts/package.json
apps/catalog/web/package.json
```

`repoctl` 根据 project graph 决定执行哪个 Bun workspace 的任务。

---

## 6.5 Python 使用 uv

每个 Python-bearing project 自己有 uv project 或 uv workspace。

例如：

```text
apps/catalog/jobs/
  pyproject.toml
  uv.lock
  src/
  packages/
```

示例：

```toml
[project]
name = "catalog-jobs"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = [
  "acme-observability",
  "acme-identity-client",
]

[tool.uv.workspace]
members = [
  "packages/*"
]

[tool.uv.sources]
acme-observability = { path = "../../../frameworks/observability/python/packages/acme-observability" }
acme-identity-client = { path = "../../../foundations/identity/clients/python/packages/acme-identity-client" }
```

`repoctl` 执行 Python task 时注入：

```bash
UV_CACHE_DIR=$REPO_ROOT/target/uv/cache
```

---

# 7. repo.yaml

根目录只有 repo 级配置，不承担语言 workspace 职责。

示例：

```yaml
schema: company.repo/v1
name: acme
layout: functional

defaults:
  owner: "@platform"
  visibility: internal

languages:
  rust:
    enabled: true
    target_dir: "{repo_root}/target/rust"
    edition: "2024"
    resolver: "3"

  typescript:
    enabled: true
    toolchain: bun
    cache_dir: "{repo_root}/target/bun"

  python:
    enabled: true
    toolchain: uv
    cache_dir: "{repo_root}/target/uv"

protos:
  root: protos
  toolchain: buf
  generated_code_policy: consumer-local

ci:
  provider: github-actions
  required_check: repoctl-required
  merge_queue: true

iac:
  core_infra_root: core-infra
  app_iac_mode: colocated
  supported:
    - pulumi
    - terraform
    - opentofu

templates:
  engine: minijinja
  builtin_root: internal
  external_sources:
    enabled: true
    allowed:
      - git

ai:
  agent_skills_root: .agents/skills
  claude_skills_root: .claude/skills
  context_output: target/repoctl/context-packs

policies:
  cross_app_dependency:
    mode: deny

  framework_internal_dependency:
    mode: deny

  generated_code:
    direct_edit: deny

  prod_change:
    required_owners:
      - "@platform"
      - "@security"
```

---

# 8. project.yaml

## 8.1 App project manifest

```yaml
schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
owners:
  - "@catalog"
visibility: internal

workspaces:
  - name: api
    language: rust
    root: api
    manifest: api/Cargo.toml
    lockfile: api/Cargo.lock
    target_dir: "{repo_root}/target/rust"

  - name: web
    language: typescript
    toolchain: bun
    root: web
    manifest: web/package.json
    lockfile: web/bun.lock
    cache_dir: "{repo_root}/target/bun"

  - name: jobs
    language: python
    toolchain: uv
    root: jobs
    manifest: jobs/pyproject.toml
    lockfile: jobs/uv.lock
    cache_dir: "{repo_root}/target/uv"

depends_on:
  - frameworks.service-runtime
  - frameworks.observability
  - frameworks.experiment-client
  - foundations.identity.client
  - protos.acme.catalog.v1

tasks:
  check:
    - workspace: api
      command: cargo check --workspace
    - workspace: web
      command: bun run check
    - workspace: jobs
      command: uv run ruff check .

  test:
    - workspace: api
      command: cargo test --workspace
    - workspace: web
      command: bun test
    - workspace: jobs
      command: uv run pytest

  build:
    - workspace: api
      command: cargo build --workspace --release
    - workspace: web
      command: bun run build

iac:
  root: iac
  provider: pulumi
  stacks:
    - dev
    - staging
    - prod

deploy:
  root: deploy
  environments:
    - dev
    - staging
    - prod

protos:
  owns:
    - protos/acme/catalog/**
  consumes:
    - protos/acme/identity/**
    - protos/acme/experiment/**

ai:
  editable:
    - api/crates/**
    - web/src/**
    - jobs/src/**
    - iac/**
    - deploy/**
    - docs/**

  do_not_edit:
    - "**/generated/**"
    - deploy/prod/**
    - iac/stacks/prod/**

  docs:
    - README.md
    - docs/architecture.md
```

---

## 8.2 Framework manifest

```yaml
schema: company.project/v1
name: frameworks.service-runtime
kind: framework
path: frameworks/service-runtime
owners:
  - "@platform"

workspaces:
  - name: rust
    language: rust
    root: rust
    manifest: rust/Cargo.toml
    lockfile: rust/Cargo.lock
    target_dir: "{repo_root}/target/rust"

  - name: ts
    language: typescript
    toolchain: bun
    root: ts
    manifest: ts/package.json
    lockfile: ts/bun.lock

public_facades:
  rust:
    - rust/crates/service-runtime-facade
  typescript:
    - ts/packages/service-runtime

internal:
  rust:
    - rust/crates/service-runtime-internal
  typescript:
    - ts/packages/service-runtime-internal

tasks:
  check:
    - workspace: rust
      command: cargo check --workspace
    - workspace: ts
      command: bun run check

  test:
    - workspace: rust
      command: cargo test --workspace
    - workspace: ts
      command: bun test

policies:
  allow_app_dependency_on_facade_only: true
```

---

## 8.3 Foundation service manifest

```yaml
schema: company.project/v1
name: foundations.identity
kind: foundation-service
path: foundations/identity
owners:
  - "@identity"

workspaces:
  - name: service
    language: rust
    root: service
    manifest: service/Cargo.toml
    lockfile: service/Cargo.lock

  - name: clients-ts
    language: typescript
    toolchain: bun
    root: clients/ts
    manifest: clients/ts/package.json
    lockfile: clients/ts/bun.lock

  - name: clients-python
    language: python
    toolchain: uv
    root: clients/python
    manifest: clients/python/pyproject.toml
    lockfile: clients/python/uv.lock

protos:
  owns:
    - protos/acme/identity/**

public_clients:
  rust:
    - clients/rust/crates/identity-client
  typescript:
    - clients/ts/packages/identity-client
  python:
    - clients/python/packages/identity-client

iac:
  root: iac
  provider: pulumi

deploy:
  root: deploy
```

---

## 8.4 Core infra manifest

```yaml
schema: company.project/v1
name: core-infra
kind: core-infra
path: core-infra
owners:
  - "@platform"
  - "@security"

iac:
  provider: pulumi
  root: .
  stacks:
    - dev
    - staging
    - prod

modules:
  public:
    - modules/cloud-run-service
    - modules/gke-cluster
    - modules/service-account
    - modules/pubsub-topic
    - modules/cloud-sql
  internal:
    - modules/org-bootstrap
    - modules/security-baseline

policies:
  high_risk: true
  prod_change_requires:
    - "@platform"
    - "@security"
```

---

# 9. Dependency Boundary Rules

## 9.1 App dependency rules

允许：

```text
app -> framework facade
app -> foundation public client
app -> protos
app -> core-infra public module
app -> app-local packages
```

禁止：

```text
app -> another app
app -> framework internal
app -> foundation service internals
app -> core-infra internal module
```

---

## 9.2 Framework dependency rules

允许：

```text
framework -> lower-level framework
framework -> protos
framework -> external dependencies
```

禁止：

```text
framework -> app
framework -> foundation service runtime implementation
framework -> app iac
```

---

## 9.3 Foundation dependency rules

允许：

```text
foundation service -> framework facade
foundation service -> protos
foundation service -> core-infra public module
```

谨慎允许：

```text
foundation service -> another foundation public client
```

禁止：

```text
foundation service -> app
foundation service -> app-local package
```

---

# 10. Core infra vs app IaC

## 10.1 结论

采用 hybrid 模式：

```text
core-infra 放共享基础设施
app IaC 跟 app 放在一起
foundation service IaC 跟 foundation service 放在一起
```

这是比所有 IaC 集中在 `infra/` 更好的长期结构。

---

## 10.2 为什么 core infra 要独立

core infra 通常包括：

```text
org bootstrap
project bootstrap
network baseline
IAM baseline
DNS
certs
artifact registry
observability
security policy
shared modules
```

这些东西：

```text
变化慢
风险高
owner 少
需要严格 review
blast radius 大
```

所以应该集中在：

```text
core-infra/
```

并由 platform / security owner 控制。

---

## 10.3 为什么 app IaC 要 colocated

app IaC 通常包括：

```text
service runtime
database
queue
bucket
scheduler
permissions
runtime config
app-specific deploy resources
```

这些东西：

```text
变化频繁
跟 app code 同步演进
由 app owner 最了解
需要跟 feature PR 一起 review
不应该每次都打扰 platform owner
```

所以应该放在：

```text
apps/<app>/iac/
```

foundation service 同理：

```text
foundations/<service>/iac/
```

---

## 10.4 共享模块在哪里

共享 IaC module 放在：

```text
core-infra/modules/
```

app 使用这些 module。

例如：

```text
apps/catalog/iac/
  Pulumi.yaml
  stacks/dev.yaml
  stacks/staging.yaml
  stacks/prod.yaml
```

其中引用：

```text
core-infra/modules/cloud-run-service
core-infra/modules/cloud-sql
core-infra/modules/service-account
```

这样可以同时满足：

```text
共享能力统一维护
app IaC 保持局部清晰
review owner 不混乱
blast radius 可控
```

---

# 11. repoctl init

## 11.1 init 目标

`repoctl init` 初始化一个功能分层的 monorepo。

它不创建根语言 workspace。

它创建：

```text
repo.yaml
AGENTS.md
.agents/skills
.claude/skills
.github workflows
CODEOWNERS
protos root
apps root
foundations root
frameworks root
core-infra root
templates root
target root
docs root
```

---

## 11.2 init 命令

```bash
repoctl init
```

完整参数：

```bash
repoctl init \
  --name acme \
  --profile startup \
  --layout functional \
  --languages rust,typescript,python \
  --typescript bun \
  --python uv \
  --iac pulumi \
  --ci github-actions \
  --protos buf
```

非交互：

```bash
repoctl init --answer-file repoctl-init.yaml --yes
```

预览：

```bash
repoctl init --dry-run
```

接管已有 repo：

```bash
repoctl init --adopt
```

---

## 11.3 init answer file

```yaml
name: acme
profile: startup
layout: functional

languages:
  rust:
    enabled: true
    target_dir: target/rust

  typescript:
    enabled: true
    toolchain: bun

  python:
    enabled: true
    toolchain: uv

protos:
  enabled: true
  toolchain: buf
  root: protos

iac:
  provider: pulumi
  core_infra: core-infra
  app_iac: colocated

ci:
  provider: github-actions
  merge_queue: true

skills:
  agent: true
  claude: true

templates:
  engine: minijinja
```

---

## 11.4 init 生成内容

```text
repo.yaml
AGENTS.md
.repoctl/
.agents/skills/
.claude/skills/
.github/CODEOWNERS
.github/pull_request_template.md
.github/workflows/ci.yml
.github/workflows/proto.yml
.github/workflows/iac.yml
.github/workflows/pr-summary.yml
.github/workflows/nightly.yml
target/
protos/
apps/
foundations/
frameworks/
core-infra/
templates/
tools/
docs/
```

不生成：

```text
Cargo.toml
package.json
pnpm-workspace.yaml
pyproject.toml
```

根目录不会成为语言 workspace。

---

## 11.5 init 后的下一步

初始化完成后输出：

```text
Initialized acme monorepo.

Created:
  repo.yaml
  AGENTS.md
  .agents/skills/
  .claude/skills/
  .github/workflows/
  protos/
  apps/
  foundations/
  frameworks/
  core-infra/
  templates/
  target/

No root language workspace was created.

Next steps:
  1. Commit the initialized structure.
  2. Create your first app:
     repoctl new app apps/catalog --stack rust-api,bun-web,uv-jobs --iac pulumi
  3. Create your first framework:
     repoctl new framework frameworks/service-runtime --languages rust,typescript
  4. Validate:
     repoctl graph validate
```

---

# 12. .agents/skills 和 .claude/skills

## 12.1 目标

AI agent 不应该只读 `AGENTS.md`。

它还应该有可复用的 task-specific skills。

`repoctl init` 默认生成：

```text
.agents/skills/
.claude/skills/
```

两个目录内容基本一致。

原因：

```text
.agents/skills 面向通用 agent runtime
.claude/skills 面向 Claude Code / Claude agent 工作流
```

---

## 12.2 生成的 skills

```text
.agents/skills/repoctl/SKILL.md
.agents/skills/monorepo-boundaries/SKILL.md
.agents/skills/proto-change/SKILL.md
.agents/skills/pr-review/SKILL.md
.agents/skills/app-creation/SKILL.md

.claude/skills/repoctl/SKILL.md
.claude/skills/monorepo-boundaries/SKILL.md
.claude/skills/proto-change/SKILL.md
.claude/skills/pr-review/SKILL.md
.claude/skills/app-creation/SKILL.md
```

---

## 12.3 repoctl skill

````md
# repoctl Skill

Use repoctl as the source of truth for this repository.

## Before Editing

Run:

```bash
repoctl explain <project>
repoctl affected --base origin/main --head HEAD
````

## Rules

* Do not edit generated files directly.
* Do not introduce cross-app dependencies.
* Do not depend on framework internal crates.
* Use framework facade crates or packages.
* Update protos before generated clients.
* Do not edit prod IaC unless explicitly requested.

## Common Commands

```bash
repoctl graph validate
repoctl run test --affected
repoctl pr summary
repoctl context <project> --for ai
```

````

---

## 12.4 monorepo-boundaries skill

```md
# Monorepo Boundaries Skill

## Allowed Dependencies

- app -> framework facade
- app -> foundation public client
- app -> protos
- app -> app-local code

## Forbidden Dependencies

- app -> app
- app -> framework internal
- framework -> app
- foundation -> app
- app -> core-infra internal module

## When unsure

Run:

```bash
repoctl lint-boundaries
repoctl explain <project>
````

````

---

## 12.5 proto-change skill

```md
# Proto Change Skill

All proto source files live under `protos/`.

## Before Changing Proto

Run:

```bash
repoctl proto owners <path>
repoctl proto consumers <path>
````

## After Changing Proto

Run:

```bash
repoctl proto check
repoctl run test --affected
```

## Rules

* Do not scatter proto source into apps.
* Do not edit generated code directly.
* Keep backward compatibility unless explicitly breaking.

````

---

## 12.6 skills sync

后续可以支持：

```bash
repoctl skills sync
````

用途：

```text
从 repo.yaml 重新生成 skills
保留用户自定义 block
升级 repoctl-managed block
同步 .agents 和 .claude
```

---

# 13. Template System

## 13.1 目标

`repoctl` 内置模板使用 MiniJinja。

原因：

```text
Rust 原生
语法清晰
适合生成文件树
支持条件、循环、过滤器
容易做 sandbox
适合内置模板和外部模板统一抽象
```

---

## 13.2 内置模板

内置模板编译进 `repoctl` binary，或者随 repoctl package 安装。

例如：

```text
builtin:repo/functional
builtin:app/fullstack
builtin:app/rust-api
builtin:app/bun-web
builtin:app/uv-jobs
builtin:foundation/service
builtin:framework/capability
builtin:core-infra/module
builtin:skills/default
```

使用：

```bash
repoctl new app apps/catalog --template builtin:app/fullstack
repoctl new framework frameworks/service-runtime --template builtin:framework/capability
```

---

## 13.3 Template 目录结构

```text
templates/app-fullstack/
  template.yaml
  files/
    project.yaml.j2
    AGENTS.md.j2
    README.md.j2
    api/Cargo.toml.j2
    api/crates/{{ app.crate_name }}/Cargo.toml.j2
    web/package.json.j2
    web/src/main.tsx.j2
    jobs/pyproject.toml.j2
    iac/Pulumi.yaml.j2
    deploy/README.md.j2
```

---

## 13.4 template.yaml

```yaml
schema: repoctl.template/v1
name: app-fullstack
kind: app
engine: minijinja

inputs:
  - name: app_name
    type: string
    required: true

  - name: slug
    type: string
    required: true

  - name: owner
    type: string
    default: "@app-owners"

  - name: stack
    type: list
    default:
      - rust-api
      - bun-web

files:
  - source: files/project.yaml.j2
    target: "{{ app.path }}/project.yaml"
    mode: managed

  - source: files/AGENTS.md.j2
    target: "{{ app.path }}/AGENTS.md"
    mode: block

  - source: files/api/Cargo.toml.j2
    target: "{{ app.path }}/api/Cargo.toml"
    when: "'rust-api' in app.stack"

  - source: files/web/package.json.j2
    target: "{{ app.path }}/web/package.json"
    when: "'bun-web' in app.stack"

post_render:
  validate:
    - repoctl graph validate
```

---

## 13.5 Template rendering context

MiniJinja context：

```yaml
repo:
  name: acme
  root: /path/to/repo

app:
  name: Catalog
  slug: catalog
  path: apps/catalog
  owner: "@catalog"
  stack:
    - rust-api
    - bun-web
    - uv-jobs

language:
  rust:
    edition: "2024"
    target_dir: "{repo_root}/target/rust"
  typescript:
    toolchain: bun
  python:
    toolchain: uv

frameworks:
  service_runtime:
    rust_facade_path: ../../../frameworks/service-runtime/rust/crates/service-runtime-facade
    ts_package: "@acme/service-runtime"
```

---

## 13.6 外部 Git template

未来支持：

```bash
repoctl template add company-app \
  --git https://github.com/acme/repoctl-templates.git \
  --ref v1.2.0

repoctl new app apps/catalog --template company-app:app-fullstack
```

外部 template 规则：

```text
必须 pin ref
必须有 template.yaml
默认禁止执行任意脚本
默认只允许 MiniJinja render
可配置 checksum
可配置 allowlist
可配置 trusted source
```

不建议第一版支持 arbitrary hook。

第一版只支持：

```text
render
copy
condition
managed block
post-render repoctl validation
```

---

# 14. repoctl CLI v0.2

## 14.1 保留命令

```text
repoctl init
repoctl new
repoctl graph validate
repoctl graph print
repoctl explain
repoctl affected
repoctl run
repoctl pr summary
repoctl context
repoctl lint-boundaries
repoctl codegen check
repoctl proto check
repoctl proto owners
repoctl proto consumers
repoctl iac plan
repoctl skills sync
repoctl template list
repoctl template add
repoctl template render
repoctl ci matrix
repoctl ci summarize
```

---

## 14.2 移除命令

v0.2 不提供 Docker build 相关命令。

移除：

```text
repoctl docker plan
repoctl docker bake
repoctl docker build
```

原因：

```text
repoctl 不应该过早绑定 Docker。
不同 app 可以部署到 Cloud Run、GKE、WASM runtime、serverless、static hosting。
构建系统可以由 CI 自己决定。
repoctl 只负责告诉 CI 哪些 project / task 受影响。
```

如果未来要恢复，也应该做成 generic artifact abstraction，而不是 Docker-first。

---

## 14.3 通用 task runner

替代上一版 `repoctl test` / `repoctl build` 的做法：

```bash
repoctl run check --affected
repoctl run test --affected
repoctl run build --affected
repoctl run test --project apps.catalog
repoctl run check --workspace apps.catalog:api
```

`repoctl` 不理解所有语言细节。

它只做：

```text
读取 project.yaml 的 task
设置 cwd
注入环境变量
按 graph 排序
并发执行
汇总结果
```

---

# 15. CI Design

## 15.1 CI 原则

```text
CI 始终触发
repoctl 计算 affected
根据 affected 生成 matrix
required check 名字稳定
不使用 path filter 作为 required workflow 的主机制
不包含 Docker build command
```

---

## 15.2 ci.yml

```yaml
name: ci

on:
  pull_request:
  merge_group:
  push:
    branches:
      - main

concurrency:
  group: ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

permissions:
  contents: read

jobs:
  detect:
    runs-on: ubuntu-latest
    outputs:
      check_matrix: ${{ steps.matrix.outputs.check_matrix }}
      test_matrix: ${{ steps.matrix.outputs.test_matrix }}
      build_matrix: ${{ steps.matrix.outputs.build_matrix }}
      has_check: ${{ steps.matrix.outputs.has_check }}
      has_test: ${{ steps.matrix.outputs.has_test }}
      has_build: ${{ steps.matrix.outputs.has_build }}
    steps:
      - uses: actions/checkout@<pin-full-sha>
        with:
          fetch-depth: 0

      - name: Install repoctl
        run: tools/ci/install-repoctl.sh

      - name: Validate graph
        run: repoctl graph validate

      - name: Compute CI matrix
        id: matrix
        run: repoctl ci matrix --tasks check,test,build --format github-actions

  hygiene:
    runs-on: ubuntu-latest
    needs: detect
    steps:
      - uses: actions/checkout@<pin-full-sha>
      - run: tools/ci/install-repoctl.sh
      - run: repoctl lint-boundaries
      - run: repoctl codegen check
      - run: repoctl proto check

  check:
    runs-on: ubuntu-latest
    needs: detect
    if: needs.detect.outputs.has_check == 'true'
    strategy:
      fail-fast: false
      matrix: ${{ fromJson(needs.detect.outputs.check_matrix) }}
    steps:
      - uses: actions/checkout@<pin-full-sha>
      - run: tools/ci/install-repoctl.sh
      - run: repoctl run check --project ${{ matrix.project }} --workspace ${{ matrix.workspace }}

  test:
    runs-on: ubuntu-latest
    needs: detect
    if: needs.detect.outputs.has_test == 'true'
    strategy:
      fail-fast: false
      matrix: ${{ fromJson(needs.detect.outputs.test_matrix) }}
    steps:
      - uses: actions/checkout@<pin-full-sha>
      - run: tools/ci/install-repoctl.sh
      - run: repoctl run test --project ${{ matrix.project }} --workspace ${{ matrix.workspace }}

  build:
    runs-on: ubuntu-latest
    needs: detect
    if: needs.detect.outputs.has_build == 'true'
    strategy:
      fail-fast: false
      matrix: ${{ fromJson(needs.detect.outputs.build_matrix) }}
    steps:
      - uses: actions/checkout@<pin-full-sha>
      - run: tools/ci/install-repoctl.sh
      - run: repoctl run build --project ${{ matrix.project }} --workspace ${{ matrix.workspace }}

  repoctl-required:
    runs-on: ubuntu-latest
    needs:
      - hygiene
      - check
      - test
      - build
    if: always()
    steps:
      - uses: actions/checkout@<pin-full-sha>
      - run: tools/ci/install-repoctl.sh
      - run: repoctl ci summarize
```

---

# 16. Affected Analysis

## 16.1 输入

```text
base commit
head commit
changed files
repo.yaml
project.yaml files
workspace specs
proto ownership
iac ownership
policy rules
```

---

## 16.2 输出

```text
directly affected projects
transitively affected projects
affected workspaces
affected tasks
risk flags
reason tree
suggested reviewers
```

---

## 16.3 特殊文件规则

```text
repo.yaml:
  affects repo-wide validation

.agents/skills/**:
  affects AI workflow validation

.claude/skills/**:
  affects AI workflow validation

protos/**:
  affects owning proto package and consumers

core-infra/**:
  affects core infra plan and high-risk review

apps/<app>/iac/**:
  affects only that app's iac plan

foundations/<svc>/iac/**:
  affects that foundation service's iac plan

frameworks/<capability>/**:
  affects framework and downstream app/foundation consumers

apps/<app>/**:
  affects that app only, plus consumers if it owns public proto/client

templates/**:
  affects template validation

tools/repoctl/**:
  affects repoctl validation and all repoctl-driven CI

target/**:
  ignored
```

---

## 16.4 Affected 传播规则

对于 `check/test/build`：

```text
changed project
+ reverse dependencies
+ proto consumers
+ framework facade consumers
```

对于 `iac-plan`：

```text
core-infra change:
  affected core infra environments

app iac change:
  affected app environments only

foundation iac change:
  affected foundation environments only
```

对于 `proto-check`：

```text
proto owner
+ generated clients
+ consumers
+ compatibility tests
```

对于 `ai-context`：

```text
changed project
+ directly related dependencies
+ docs
+ skills
```

---

# 17. Proto Design

## 17.1 Source proto 集中

所有 source proto 集中在：

```text
protos/
```

不允许：

```text
apps/app-a/protos/
foundations/identity/protos/
frameworks/foo/protos/
```

app 可以拥有某个 proto package，但 source 仍然放在 root `protos/`。

---

## 17.2 Proto ownership

```bash
repoctl proto owners protos/acme/identity/v1/identity.proto
```

输出：

```text
Owner:
  foundations.identity

Reviewers:
  @identity
  @api-review

Consumers:
  apps.catalog
  apps.admin
  foundations.authz
```

---

## 17.3 Proto consumers

```bash
repoctl proto consumers protos/acme/identity/v1/identity.proto
```

输出：

```text
Consumers:
  foundations.identity.clients.rust
  foundations.identity.clients.ts
  foundations.identity.clients.python
  apps.catalog.api
  apps.catalog.web
```

---

## 17.4 Generated code policy

默认策略：

```text
source proto 集中
generated code consumer-local
generated code 不允许手改
```

consumer-local 的意思：

```text
foundation client SDK 生成到 foundation clients 下
app-local generated code 生成到 app workspace 下
framework generated code 生成到 framework workspace 下
```

示例：

```text
foundations/identity/clients/rust/crates/identity-client/src/generated/
foundations/identity/clients/ts/packages/identity-client/src/generated/
apps/catalog/api/crates/catalog-api/src/generated/
```

---

# 18. AI Context Design

## 18.1 repoctl context

```bash
repoctl context apps.catalog --for ai
```

输出：

````md
# Project Context: apps.catalog

## Kind

app

## Owner

@catalog

## Workspaces

- api: Rust workspace at apps/catalog/api
- web: Bun workspace at apps/catalog/web
- jobs: uv workspace at apps/catalog/jobs

## Allowed Edits

- api/crates/**
- web/src/**
- jobs/src/**
- iac/**
- deploy/**
- docs/**

## Do Not Edit

- **/generated/**
- iac/stacks/prod/**
- deploy/prod/**

## Dependencies

- frameworks.service-runtime
- frameworks.observability
- foundations.identity.client
- protos.acme.catalog.v1

## Commands

```bash
repoctl run check --project apps.catalog
repoctl run test --project apps.catalog
repoctl iac plan --project apps.catalog --env dev
````

## Boundary Rules

* Do not import another app.
* Use framework facade crates/packages only.
* Do not import framework internal crates.
* Do not edit generated code directly.

````

---

# 19. PR Summary

```bash
repoctl pr summary --base origin/main --head HEAD
````

生成：

````md
## Monorepo Impact

### Changed Projects

- `apps.catalog`
- `frameworks.observability`

### Affected Workspaces

- `apps.catalog:api`
- `apps.catalog:web`
- `frameworks.observability:rust`

### Affected Tasks

- `check`
- `test`
- `build`

### Proto Impact

No proto changes.

### IaC Impact

- `apps.catalog/iac`
- env: dev, staging

### Risk Flags

- Framework facade changed.
- No prod IaC change.
- No generated code direct edit.

### Suggested Reviewers

- @catalog
- @platform

### Suggested Commands

```bash
repoctl run check --affected
repoctl run test --affected
repoctl iac plan --affected
````

````

---

# 20. repoctl Design Spec

## 20.1 Rust crate structure

```text
tools/repoctl/
├── Cargo.toml
├── crates/
│   ├── repoctl-cli/
│   ├── repoctl-core/
│   ├── repoctl-manifest/
│   ├── repoctl-workspace/
│   ├── repoctl-graph/
│   ├── repoctl-git/
│   ├── repoctl-init/
│   ├── repoctl-template/
│   ├── repoctl-task/
│   ├── repoctl-ci/
│   ├── repoctl-pr/
│   ├── repoctl-ai/
│   ├── repoctl-skills/
│   ├── repoctl-proto/
│   ├── repoctl-iac/
│   └── repoctl-policy/
└── templates/
````

注意：

```text
tools/repoctl/Cargo.toml 是 repoctl 自己的 workspace。
不是 repo 根 workspace。
```

---

## 20.2 repoctl-manifest

负责：

```text
解析 repo.yaml
解析 project.yaml
解析 proto ownership
解析 workspace specs
解析 task specs
填充默认值
schema validation
```

核心类型：

```rust
pub struct RepoManifest {
    pub schema: String,
    pub name: String,
    pub layout: RepoLayout,
    pub defaults: Defaults,
    pub languages: LanguageDefaults,
    pub protos: ProtoConfig,
    pub ci: CiConfig,
    pub iac: IacConfig,
    pub templates: TemplateConfig,
    pub ai: AiConfig,
    pub policies: PolicyConfig,
}

pub struct ProjectManifest {
    pub schema: String,
    pub name: ProjectName,
    pub kind: ProjectKind,
    pub path: PathBuf,
    pub owners: Vec<Owner>,
    pub visibility: Visibility,
    pub workspaces: Vec<WorkspaceSpec>,
    pub depends_on: Vec<ProjectName>,
    pub tasks: BTreeMap<TaskName, Vec<TaskSpec>>,
    pub iac: Option<IacSpec>,
    pub deploy: Option<DeploySpec>,
    pub protos: Option<ProjectProtoSpec>,
    pub ai: Option<ProjectAiSpec>,
}
```

---

## 20.3 repoctl-workspace

负责发现和执行 workspace。

```rust
pub enum WorkspaceLanguage {
    Rust,
    TypeScript,
    Python,
    Proto,
    Iac,
    None,
}

pub enum Toolchain {
    Cargo,
    Bun,
    Uv,
    Buf,
    Pulumi,
    Terraform,
    OpenTofu,
    Custom(String),
}

pub struct WorkspaceSpec {
    pub name: WorkspaceName,
    pub language: WorkspaceLanguage,
    pub toolchain: Toolchain,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub lockfile: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}
```

执行 task 时：

```text
repoctl 找到 workspace root
设置 cwd
设置 toolchain env
执行 command
收集 stdout/stderr/status
```

---

## 20.4 repoctl-graph

负责：

```text
project graph
workspace graph
proto consumer graph
iac ownership graph
facade/internal boundary graph
reverse dependency graph
```

核心 edge：

```rust
pub enum EdgeKind {
    DependsOnProject,
    ConsumesProto,
    OwnsProto,
    UsesFrameworkFacade,
    UsesFoundationClient,
    UsesCoreInfraModule,
    OwnsIac,
}
```

---

## 20.5 repoctl-init

负责：

```text
初始化 functional layout
渲染 MiniJinja templates
生成 skills
生成 CI skeleton
生成 repo.yaml
生成 protos root
生成 core-infra skeleton
生成 app/foundation/framework roots
写入 init-state
```

init 仍然先生成 plan，再执行。

```rust
pub struct InitPlan {
    pub repo_root: PathBuf,
    pub operations: Vec<FileOperation>,
    pub warnings: Vec<InitWarning>,
    pub next_steps: Vec<String>,
}
```

---

## 20.6 repoctl-template

使用 MiniJinja。

```rust
pub enum TemplateSource {
    Builtin {
        name: String,
    },
    Local {
        path: PathBuf,
    },
    Git {
        url: String,
        reference: String,
        subdir: Option<PathBuf>,
        checksum: Option<String>,
    },
}
```

第一版支持：

```text
Builtin
Local
```

后续支持：

```text
Git
```

Template engine：

```rust
pub trait TemplateEngine {
    fn render_file(&self, template: &str, context: &TemplateContext) -> Result<String>;
}
```

MiniJinja 实现：

```rust
pub struct MiniJinjaEngine {
    env: minijinja::Environment<'static>,
}
```

---

## 20.7 repoctl-skills

负责：

```text
生成 .agents/skills
生成 .claude/skills
同步 managed blocks
验证 skills 是否过期
根据 repo.yaml 和 project.yaml 生成 project-specific instructions
```

命令：

```bash
repoctl skills sync
repoctl skills check
```

---

## 20.8 repoctl-proto

负责：

```text
proto ownership
proto consumers
proto breaking check
proto generated code check
proto context for AI
```

命令：

```bash
repoctl proto check
repoctl proto owners <path>
repoctl proto consumers <path>
repoctl proto affected
repoctl proto generate --project apps.catalog
```

---

## 20.9 repoctl-iac

负责：

```text
core-infra plan
app iac plan
foundation iac plan
affected iac plan
prod risk detection
```

命令：

```bash
repoctl iac plan --affected
repoctl iac plan --project apps.catalog --env dev
repoctl iac plan --core --env staging
```

它不自动 apply。

第一版不做自动 apply。

---

## 20.10 repoctl-task

负责：

```text
执行 check/test/build/codegen 之类通用 task
按 workspace 执行
按 graph 排序
并发调度
汇总输出
```

命令：

```bash
repoctl run check --affected
repoctl run test --affected
repoctl run build --project apps.catalog
```

---

# 21. Policy Design

## 21.1 no cross-app dependency

```yaml
rules:
  - name: no-cross-app-dependency
    from:
      kind: app
    to:
      kind: app
    action: deny
```

---

## 21.2 framework facade only

```yaml
rules:
  - name: app-must-use-framework-facade
    from:
      kind: app
    to:
      kind: framework
      area: internal
    action: deny
```

---

## 21.3 foundation public client only

```yaml
rules:
  - name: app-must-use-foundation-client
    from:
      kind: app
    to:
      kind: foundation-service
      area: internal
    action: deny
```

---

## 21.4 generated code readonly

```yaml
rules:
  - name: generated-code-readonly
    paths:
      - "**/generated/**"
    action: deny-direct-edit
```

---

## 21.5 high-risk IaC

```yaml
high_risk_paths:
  - path: core-infra/**
    reviewers:
      - "@platform"
      - "@security"

  - path: apps/*/iac/stacks/prod/**
    reviewers:
      - "@app-owner"
      - "@platform"

  - path: foundations/*/iac/stacks/prod/**
    reviewers:
      - "@foundation-owner"
      - "@platform"
```

---

# 22. MVP Milestones

## Milestone 0: Functional Init

交付：

```text
repoctl init
functional layout
repo.yaml
AGENTS.md
.agents/skills
.claude/skills
protos root
apps/foundations/frameworks/core-infra roots
GitHub Actions skeleton
```

验收：

```bash
repoctl init --name acme --profile startup --layout functional
repoctl graph validate
repoctl skills check
```

---

## Milestone 1: Project + Workspace Graph

交付：

```text
project.yaml schema
workspace specs
graph validate
explain
boundary lint
```

验收：

```bash
repoctl graph validate
repoctl explain apps.catalog
repoctl lint-boundaries
```

---

## Milestone 2: App Creation

交付：

```text
repoctl new app
MiniJinja builtin templates
Rust API template
Bun web template
uv jobs template
app-local IaC template
```

验收：

```bash
repoctl new app apps/catalog --stack rust-api,bun-web,uv-jobs --iac pulumi
repoctl graph validate
repoctl run check --project apps.catalog
```

---

## Milestone 3: Affected + CI Matrix

交付：

```text
repoctl affected
repoctl ci matrix
repoctl run <task> --affected
repoctl ci summarize
```

验收：

```bash
repoctl affected --base origin/main --head HEAD
repoctl ci matrix --tasks check,test,build
repoctl run test --affected
```

---

## Milestone 4: Proto System

交付：

```text
protos/project.yaml
proto ownership
proto consumers
proto check
consumer-local generated code policy
```

验收：

```bash
repoctl proto owners protos/acme/identity/v1/identity.proto
repoctl proto consumers protos/acme/identity/v1/identity.proto
repoctl proto check
```

---

## Milestone 5: AI Context + Skills

交付：

```text
repoctl context
repoctl skills sync
repoctl pr summary
AI-safe editable/do_not_edit areas
```

验收：

```bash
repoctl context apps.catalog --for ai
repoctl pr summary
repoctl skills check
```

---

## Milestone 6: IaC Plan

交付：

```text
core-infra plan
app-local iac plan
foundation-local iac plan
affected iac plan
prod risk flags
```

验收：

```bash
repoctl iac plan --affected
repoctl iac plan --project apps.catalog --env dev
repoctl iac plan --core --env staging
```

---

# 23. 最终日常工作流

## 23.1 初始化 repo

```bash
repoctl init \
  --name acme \
  --profile startup \
  --layout functional \
  --languages rust,typescript,python \
  --typescript bun \
  --python uv \
  --iac pulumi \
  --protos buf
```

---

## 23.2 创建 app

```bash
repoctl new app apps/catalog \
  --stack rust-api,bun-web,uv-jobs \
  --iac pulumi
```

---

## 23.3 创建 framework

```bash
repoctl new framework frameworks/service-runtime \
  --languages rust,typescript \
  --facade true
```

---

## 23.4 创建 foundation service

```bash
repoctl new foundation foundations/identity \
  --service rust \
  --clients rust,typescript,python \
  --iac pulumi \
  --proto acme.identity.v1
```

---

## 23.5 日常开发

```bash
repoctl explain apps.catalog
repoctl run check --project apps.catalog
repoctl run test --project apps.catalog
repoctl affected --base origin/main --head HEAD
```

---

## 23.6 PR 前

```bash
repoctl graph validate
repoctl lint-boundaries
repoctl proto check
repoctl run test --affected
repoctl iac plan --affected
repoctl pr summary
```

---

# 24. 一句话总结

更新后的 `repoctl` 不再围绕语言组织 monorepo，而是围绕 **app、framework、foundation service、core infra、protos** 组织工程系统。

它的目标不是把 repo 变成一个巨大的 workspace，而是让每个 app 独立、清晰、好维护，同时让核心能力可以自然抽取到 framework，让共享基础设施沉淀到 core infra，让 protos 统一管理，让 AI agent 和 CI 都围绕同一张 project graph 工作。
