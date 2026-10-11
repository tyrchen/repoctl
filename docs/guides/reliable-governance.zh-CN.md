# 多语言仓库治理（v0.7）

repoctl 的核心定位是为开发者、CI 和 AI 提供共同的架构、归属、依赖及变更影响模型。
默认离线读取 Cargo、Node、Python 清单；仅显式的 `graph validate --mode full`
可能运行 Cargo metadata。Doctor 的各项检查共享一次发现得到的快照，下一次调用重新发现。
Node/Python 包索引和解析后的清单在每次建图内共享，下一次建图会重新读取。
Cargo 虚拟 workspace 会展开成员及通配符，遵循排除规则，并读取成员的依赖、target 表和
workspace 继承；不需要运行 Cargo。缺失成员或路径越出仓库会报错。
Python 图发现也覆盖 Poetry 的开发、测试等依赖分组。

## 任务与变更影响

```bash
repoctl affected --base origin/main --head HEAD --tasks check,test,build
repoctl run check --affected --concurrency 4
repoctl doctor --agent
```

Git 范围以 merge-base 为起点；未提供引用时包含暂存、未暂存、删除和未跟踪文件。
重命名同时选择原位置和新位置的归属。影响沿项目依赖反向传播，覆盖基础设施及内部接口，
并保留依赖证据和间接受影响项目的负责人。没有归属的文件产生警告。

任务等待所有前置任务成功后执行；同一任务的多个命令按清单顺序执行，独立任务可以并发。
失败跳过后续依赖任务，但独立分支继续。缺失依赖和循环是错误，并发范围为 1–256。
每个任务的不同前置依赖并集最多为 256，超过上限会在展开依赖之前拒绝。
默认超时 300 秒，每个输出流最多 1 MiB；通过 `timeout_seconds` 和 `max_output_bytes`
调整。超时状态为 124，输出超限为 125。程序终止并回收直接子进程；本版本不提供进程树隔离。

## Nx 思路的显式缓存契约

```yaml
tasks:
  build:
    - workspace: web
      command: npm run build
      cache:
        enabled: true
        inputs: ["apps/catalog/web/src/**", "apps/catalog/web/package.json"]
        outputs: ["apps/catalog/web/dist"]
        env: ["NODE_ENV"]
        runtime: ["node --version"]
```

路径相对于仓库。输出必须位于所属 workspace 内，不能覆盖输入、配置、仓库元数据或其他任务输出。
前缀检查比较保守，应使用具体源码模式，避免包含输出目录的整个 workspace `**` 模式。
缓存键包含命令、指定环境、运行时探测、配置、项目及前置依赖的源码闭包。标准包管理器版本
及 PATH 自动参与缓存键；其他外部环境或运行时输入应显式声明。

仅缓存可重复、输出和日志不含秘密的任务。命中恢复文件、执行权限和空目录，并清除陈旧输出。
失败和跳过的结果不会缓存；损坏条目产生警告并重新执行。存储位于 `.repoctl/cache/tasks`，
单条目上限 64 MiB、4096 个文件或目录。缺失声明输出或缓存 I/O 失败会使任务失败。
不提供远程缓存或后台 daemon；将 `.repoctl/cache/` 加入被管理仓库的 `.gitignore`。
源码不应放在自动排除的 `target`、`dist`、`build`、`node_modules`、`.venv` 等产物目录中。

若前置任务有任何命令未提供包含产物的缓存指纹，下游会报告警告并绕过缓存；这一规则沿任务链传播。

```bash
repoctl run build --no-cache
```

此选项同时绕过缓存读写。IaC 与 operations 执行不使用任务缓存。

## Buf breaking 与真实 CI

```bash
repoctl proto check --against '.git#branch=main,subdir=protos' --format json
repoctl proto check --against '.git#branch=main,subdir=protos' --execute
repoctl ci workflow --write
repoctl ci run-step --project apps.catalog --workspace web --task check --install
```

Proto 默认只规划命令；`--execute` 使用已安装的 Buf 执行 lint 和 breaking，违规使 CLI 失败。
规则由配置 proto 根目录的 `buf.yaml` 管理，比较源可以是 Git、本地基线或 Buf registry。
Registry 的认证和联网由 Buf 负责。Doctor 始终只规划这些命令。

生成的 CI 使用 graph、任务矩阵和 required 三个 job，独立安装固定版本 CLI，安装所需工具链
及依赖，并真正执行任务与前置任务。发现图不会自动安装依赖，安装需要 `--install`。
CI 矩阵仅包含原始选中的任务，前置依赖在同一个 job 内执行；工具链选择仍覆盖完整依赖链。
有锁文件时使用冻结安装。空矩阵可以跳过任务，但失败或取消不能通过 required 检查。
protoc 检测覆盖 Cargo 成员、本地依赖及实际使用的 build script。

**发布前提：**生成的 workflow 固定使用 `repoctl-cli =0.7.0`，外部仓库启用前需要按顺序发布
workspace crates。本 PR 不发布版本；集成测试使用本地构建验证相同的工作流契约。

## 有预算的上下文与图导出

```bash
repoctl context apps.catalog --changed-file apps/catalog/api/src/lib.rs --include-docs --max-bytes 65536
repoctl graph print --diagram mermaid
repoctl affected --changed-file frameworks/ui/src/index.ts --diagram mermaid
```

源码需要显式选择并属于指定项目；声明的文档可以是文件或目录。保护路径、生成代码、疑似秘密、
二进制和符号链接会排除。JSON 包含内容哈希、截断及省略原因，预算计算序列化后的 UTF-8 字节，
默认 64 KiB、最大 1 MiB，包括项目元数据。显式文件最多 256 个，总选择最多 512 个，文档目录
遍历最多 32 层。元数据本身超过预算时返回错误。Mermaid 使用稳定 ID 和转义标签，不自动发布。

迁移覆盖通用 Cargo/Node 包，保留 Cargo 别名、features、optional、default-features 和 target
配置；重名包及无法解析的 workspace 继承会失败。归类依据内容或 `--map`，任务依据实际
workspace、Node scripts/TypeScript 配置及 Python 编译检查生成。Python 依赖源重写不在本版本内。
Node 子包保留独立任务，并继承最近的锁文件及包管理器，CI 在锁文件所属目录安装依赖。
没有锁文件时不会生成虚假的锁文件声明。
Python 子包同样继承最近的 `uv.lock`，安装在锁文件所属目录执行 `uv sync --frozen`。

自动改写 Node 依赖后，旧锁文件不会被复制；计划中的 `lockfileRefreshes` 列出必须执行的
刷新命令。先执行 `adopt apply`，再在指定目录运行这些命令、提交新锁文件，最后执行
`adopt verify`。缺失待刷新的锁文件会使验证失败。迁移本身不会运行包管理器。
规划和应用前都会检查已有文件及重复目标；已有 `project.yaml` 或 `project.yml` 不会被覆盖。

Rust API 新增请求和执行字段，因此版本升级为 0.7.0；YAML 新选项均可省略。
详细契约见[设计规格](../../specs/repoctl-reliable-governance-design.md)和[英文指南](reliable-governance.md)。
