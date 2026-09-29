# MathDoc 协调层改造计划：TypeScript 后端 + 保留知识编辑界面

> 状态：**计划，已完成首轮审阅，待按阶段执行**。本文只描述要做什么；工作区中已有的
> `coordinator/`、`app/` 与入口改动是待审阅实现，不表示本计划已经交付。
>
> 日期：2026-09-28（同日修订 §6.4 契约冻结与 §7.3 声明转换；同日再按 LeanGround `3965273` 修订草图组件与
> 定义支持，见 §7.3、§11）。基线提交：`a4d61e3`（分支 `codex/mathdoc-coordination`），另有 Codex 未提交的
> `coordinator/`、`app/` 与入口改动（§4.6）。LeanGround 基线：`main` 的 `3965273`（合并了草图组件
> [#10](https://github.com/YanbiaoLab/LeanGround/pull/10) 与定义登记 [#11](https://github.com/YanbiaoLab/LeanGround/pull/11)）。
>
> 本文回答：**把 MathDoc 改造成多人与 Agent 共同补全证明图的协作应用时，哪些代码保留、改造、隔离、
> 删除；已有文档数据怎么兼容；新旧接口和状态怎么对应；默认要启动哪些服务；每一阶段怎么验收。**
>
> 相关文档：`docs/coordination.md`（Codex 对其现有实现的说明）；LeanGround 仓库的
> `docs/11`（协作模式与事实层契约，其中 §7.5 是定义节点与定义登记）、`docs/12`（事实层接口与验收，
其中「内置草图组件」「定义登记」两节是本文依赖的接口）。

---

## 0. 已确定的决定

| 决定 | 内容 |
|---|---|
| 产品定位 | MathDoc 是顶层协作应用：提出问题、组织协作、审阅成果、展示知识 |
| 后端 | **改为 TypeScript**，以 Codex 的 `coordinator/` 为基础；旧 Rust 后端在替代路径验收前继续可用 |
| 默认界面 | **保留现有完整的知识编辑界面 `web/`**，在其中新增四个协作视图；Codex 的轻量工作台 `app/` 不作为默认界面 |
| 文档存储 | 继续用 TerminusDB，**不迁移**已有数据 |
| 协作存储 | **PostgreSQL**（理由见 §3.3） |
| 认证事实与源码 | 由 LeanGround 持有；MathDoc 只引用认证 ID |
| 草图检查 | 由 LeanGround 的内置组件 `lean-worker-sketch` 完成；MathDoc 经 LeanGround `submit` 选择该组件，不直接调用任何证明组件 |
| 定义 | 使用 LeanGround 已交付的定义登记（docs/11 §7.5）：定义节点登记为定义、按定义 ID 引用；MathDoc 负责节点与定义的对应和回写（§7.3） |
| 本地 Lean 执行 | 从默认运行路径移出 |
| EvoHarness | 本次不涉及 |
| 顺序 | **先隔离、验收替代路径，再切默认入口，最后清理死代码** |

---

## 1. 现状 [源码，`HEAD a4d61e3`]

### 1.1 后端：Rust，约 9,100 行，一个 axum 进程

| 职责 | 文件 | 外部依赖 |
|---|---|---|
| 证明图与文档（TerminusDB） | `store.rs`、`service.rs`、`server.rs`、`server/projects.rs`、`core/algorithms.rs` | TerminusDB（HTTP） |
| LaTeX 预览 | `latex/api.rs`、`latex/runtime.rs`、`latex/project.rs` 驱动一个常驻 Python 进程 `latex/renderer.py`（plasTeX、pybtex），按行交换 JSON | python3 + plasTeX 虚拟环境；TikZ 只在浏览器里渲染 |
| 本地 Lean | `lean.rs`、`lean/editor.rs`、`lean/transport.rs`、`lean/artifact_status.lean` | elan、`lake serve`、`lake build`、磁盘上的认证缓存 |
| Rocq | 无（只是一种允许的块类型，状态恒为未验证） | — |
| 项目目录、服务启停、静态资源 | `server/projects.rs`、`service.rs`（按分支的守护进程与租约）、`web/assets.rs` | — |
| 命令行客户端 `mdc` | `cli.rs` | — |

- 每个分支有自己的路由，路径形如 `/p/<数据库>/<分支>/api/...`。
- **没有用户身份**，只做同源检查。

### 1.2 前端：`web/`，Svelte 5 + Vite + TypeScript

- 主视图（`App`、`NodeColumn`、`EditorPane`、`lib/api.ts`）是硬依赖。
- 按需加载：源码编辑器 `BlockEditor`（Monaco，所有块的编辑都用它）、依赖图 `DepthGraph`、TikZ、MathJax。
- `LeanBlock` 通过 iframe 打开 `lean.html`，其中运行 `lean4monaco` 与 Infoview，经 WebSocket 连接后端的 Lean 语言服务器。
- 写操作在 `If-Match` 里带节点修订号（节点 JSON 的 SHA-256），按节点串行；409/412 视为冲突。

### 1.3 数据：TerminusDB

- `Node`：`fnode`（UUID）、`title`、`module`（Lean 模块名 `Lib.N_<uuid>`）、`blocks`（一个 JSON 字符串，每种类型至多一块：text、lean、rocq、latex）、`depens`（直接依赖的节点）。
- `Project`：`lean`、`latex` 两条配置。
- 修订保护：节点级 `If-Match`，提交时带 `TerminusDB-Data-Version`。

### 1.4 Lean 块与节点依赖的关系

**节点依赖就是 Lean 模块 import。** 一个节点的 Lean 块被编成模块 `Lib.N_<uuid>`，它的 import 必须与该节点
的直接依赖**完全一致**（`lean.rs:382`）；认证沿 import 链逐层传递并缓存。这是本次改造最大的兼容问题（§7.3）。

### 1.5 启动与测试

- 启动：TerminusDB + `mdc` 进程（内含 Lean 工具链与 Python 渲染环境）。
- 测试：Rust 集成测试（多数需 TerminusDB、Lean 或 Python）、`latex/test_renderer.py`（15 项）、`web/` 的 Vitest 单元测试、
  **`web/e2e/run.mjs` 的 39 项 Playwright 端到端测试**（对真实后端二进制 + TerminusDB 运行）、性能基准。

---

## 2. 产品边界

| 模块 | 处理 |
|---|---|
| 项目、数学节点、说明、LaTeX、引用、知识依赖 | 保留 |
| 文档分支、历史、带修订保护的编辑 | 保留 |
| Lean 源码展示与编辑 | 保留；**本地检查结果与正式认证分开显示** |
| 本地 Lean/Lake 环境、编译、worker、认证缓存 | 移出默认运行路径，由外部服务承担 |
| Infoview、完整 Lean IDE 集成 | 移出默认构建，作为可选编辑扩展 |
| Rocq 执行及其他与本轮证明协作无关的执行能力 | 不提供（原本也没有）；已有内容可读可编辑 |
| 大规模源码导入、编译性能实验 | 移出默认产品入口与验收链路，代码保留 |
| 目标、拆法、任务领取、预算、事实同步、组装 | 新增 |
| 草图检查 | 经 LeanGround 的内置组件 `lean-worker-sketch` 完成 |

裁剪要落实到**入口、路由、依赖和启动过程**，不能只隐藏按钮。

---

## 3. 目标架构

```
web/（保留的完整知识编辑界面 + 四个协作视图）
  │  /p/<库>/<分支>/api/...（与旧后端相同的路径与语义）   /api/coordination/...
  ▼
TypeScript 后端（coordinator/ 扩展而来）
  ├─ 文档接口 ──────────────► TerminusDB（原有数据，不迁移）
  ├─ LaTeX 预览 ────────────► Python renderer.py 常驻进程（原样复用）
  ├─ 协作接口 + 后台作业 ───► PostgreSQL（协作数据）
  └─ 身份：令牌 → 用户 → 文档工作区角色 / 协作项目角色
外部：LeanGround（命题身份、定义登记、认证事实、推导、计划、组装；
      草图检查由其内置组件 lean-worker-sketch 完成）
```

### 3.1 三方职责

| 参与方 | 负责 | 输出 |
|---|---|---|
| MathDoc 协调层 | 哪些目标值得做、谁来做、投入多少、采用哪种拆法；文档审阅与回写 | 任务、调度决定、组装请求、文档修订 |
| LeanGround | 命题身份、定义登记、认证事实（含草图检查产生的带前提认证）、推导查询、固定计划、最终组装认证 | GoalKey、定义 ID、认证 ID、计划 ID |

协调层可以记录「某个 Agent 声称完成」，但只有读取并核对 LeanGround 的认证后，才显示数学上的进展。

### 3.2 为什么复用 `coordinator/`、不复用 `app/`

- `coordinator/` 已有协作领域模型、PostgreSQL 存储、租约、幂等命令、事件同步和 TerminusDB 适配器，方向与本计划一致。
- `app/` 用 KaTeX 做基础渲染，BibTeX、跨节点宏、TikZ、Infoview 都要回到旧应用才能用，不满足「保留知识编辑主界面」。
  它的四个协作视图的交互逻辑可以参考，但界面放进 `web/`。

### 3.3 协作存储为什么用 PostgreSQL

`coordinator/` 把 API 与后台作业拆成两个进程，并需要共享的事务存储、原子预算预留、作业租约、
`SKIP LOCKED` 领取和后续多实例部署。因此确定使用 PostgreSQL。首版可以让同一项目的状态写入串行，
但不能把这个实现宣传为已经通过千 Agent 负载验收。
**不为了协作数据迁移任何已有文档数据。**

---

## 4. 逐文件清单

处理方式：**保留**（行为不变）、**改造**（本计划内修改）、**隔离**（不进默认构建与启动，代码保留）、
**删除**（替代路径验收通过后，在最后阶段删除）。

### 4.1 Rust 后端 `src/`

| 文件 | 处理 | 去向 |
|---|---|---|
| `store.rs`、`service.rs`、`server.rs`、`server/projects.rs`、`core/*` | 隔离 → 删除 | 文档与图接口移植到 TypeScript（§6.1）；阶段 5 后删除 |
| `latex/api.rs`、`latex/runtime.rs`、`latex/project.rs`、`latex.rs` | 隔离 → 删除 | 由 TypeScript 驱动同一个 Python 进程 |
| `latex/renderer.py`、`latex/richtext.py`、`latex/requirements.txt`、`latex/amsalpha.*`、`latex/test_renderer.py` | **保留** | 移到共享位置（例如 `renderer/`），TypeScript 后端调用 |
| `lean.rs`、`lean/*` | 隔离 → 删除 | 被「提交给 LeanGround」取代（§7） |
| `web/assets.rs` | 隔离 → 删除 | TypeScript 后端提供静态资源 |
| `config.rs`、`profile.rs`、`lib.rs`、`main.rs` | 隔离 → 删除 | 配置项对照见 §8 |
| `cli.rs`（`mdc` 命令行） | 隔离 | 待定：是否提供 TypeScript 版命令行（§10） |
| `Cargo.toml`、`Cargo.lock` | 隔离 → 删除 | 旧应用构建专用 |

### 4.2 前端 `web/`

| 文件 | 处理 | 说明 |
|---|---|---|
| `App.svelte`、`components/NodeColumn`、`EditorPane`、`BlockEditor`、`AddBlockControl`、`AddDepOverlay`、`RmDepOverlay`、`NewNodeOverlay`、`SearchOverlay`、`DepthGraph`、`LatexPreview`、`LatexImports`、`LatexProjectForm`、样式文件 | **保留并做必要适配** | 文档编辑交互和主要路径保持一致；状态模型变化的消费方必须改造 |
| `Projects.svelte`、`ProjectCreate`、`ProjectSettings` | **改造** | 移除 running/stopped 与 start/stop 假设，改为工作区权限、直接打开分支及外部证明环境设置 |
| `lib/api.ts`、`lib/api-types.ts` | 改造 | 加身份令牌；新增协作接口的类型与调用 |
| `lib/latex*.ts`、`lib/monaco*.ts`、`lib/state*`、`lib/workspace*`、`lib/history*`、其余 `lib/*` | 保留 | — |
| `components/LeanBlock.svelte`、`components/FormalStatus.svelte` | 改造 | Lean 块只展示与编辑源码；状态区分「本地检查」与「正式认证」；「检查」改为「提交给 LeanGround」 |
| `components/LeanProjectForm.svelte` | 改造 | 从配置 Lake 项目改为选择 LeanGround 的 Base 与项目环境 |
| `lean.html`、`lean-editor.ts`、`build/lean-socket.ts`、依赖 `lean4monaco` | 隔离 | 可选构建开关，默认不构建、不打包 |
| 新增：证明概览、任务板、拆法视图、成果审阅 四个视图，以及登录（令牌） | 新增 | §9 |
| `e2e/run.mjs` | 改造 | 启动对象从 Rust 二进制改为 TypeScript 后端；**作为阶段 2 的对齐验收**；删去依赖本地 Lean 的用例并补上协作用例 |
| `perf/` | 保留 | 移出默认验收 |

### 4.3 测试与脚本

| 文件 | 处理 |
|---|---|
| `tests/test_service.rs`、`test_status.rs`、`test_database.rs`、`test_algorithms.rs`、`test_service_scale.rs`、`test_latex_service.rs` | 隔离 → 删除；覆盖的行为由 `web/e2e` 与 TypeScript 接口测试接替 |
| `tests/test_lean_service.rs`、`test_mathlib_bench.rs` | 隔离 → 删除（本地 Lean 执行不再在默认路径） |
| `tests/docker-smoke.py` | 改造：检查新镜像 |
| `scripts/mdc-docker` | 隔离 |
| `perf/`（报告与基准脚本） | 保留，移出默认验收 |

### 4.4 构建、部署与 CI

| 文件 | 阶段 0–4 | 阶段 5 |
|---|---|---|
| `Dockerfile`、`compose.server.yaml`、`compose.yaml`、`.dockerignore` | **恢复到基线提交 `a4d61e3` 中的对应文件**，默认仍是旧应用 | 切到 TypeScript 后端 + `web/`；旧的另存为 legacy |
| `.github/workflows/release-check.yml` | 恢复到基线提交 `a4d61e3`；新增一个并行的协作层检查工作流 | 协作层检查成为默认，旧检查改手动 |
| `backend-perf.yml`、`frontend-perf.yml`、`docs-deploy.yml` | 保留 | backend-perf 随 Rust 隔离 |

### 4.5 文档站 `docs/src/content/docs/`

阶段 5 前更新：`reference/http-api`、`reference/configuration`、`reference/work-and-compilers`、
`development/architecture`、`development/compiler-internals`、`development/setup`、`development/web-frontend`、
`concepts/source-workflow`、`concepts/web-interface`、`getting-started/*`。新增协作概念与 Agent 接口两页。

### 4.6 Codex 未提交的改动

| 内容 | 处理 |
|---|---|
| `coordinator/`（`config`、`documents`、`domain`、`http`、`main`、`remote`、`store`、`worker` 及测试） | 保留并提交；阶段 1 逐条审阅，阶段 2 起扩展 |
| `app/` | 提交存档，不作为默认界面；协作视图交互可参考 |
| 根 `package.json`、`package-lock.json`、`config.example.env` | 保留，工作区加入 `web` |
| `docs/coordination.md` | 保留，阶段 5 前与本文合并 |
| `Dockerfile`、`compose.server.yaml`、`.dockerignore`、`release-check.yml`、`README.md`、`tests/docker-smoke.py` 的改动，以及 `Dockerfile.legacy`、`compose.legacy.yaml`、`legacy-check.yml` | 在存档提交中保留；随后仅把默认入口文件恢复到基线 `a4d61e3`，新增 legacy 文件在阶段 5 前不进入默认路径 |

---

## 5. 数据兼容

- **TerminusDB 的模式与数据不变。** `Node`、`Project` 字段照旧；新后端读写同一批文档，旧后端在隔离期间仍能打开同一数据库。
- **修订保护不变**：节点修订号的计算方法、`If-Match` 语义、`TerminusDB-Data-Version` 前置条件，
  与 `store.rs` 逐字节一致，由接口测试比对两个后端对同一节点给出的修订号。
- **Lean 块内容不变**；协作相关的标记（例如回写操作 ID、绑定的认证 ID、定义节点登记得到的定义 ID）写进块的
  `metadata`，不改块的源码。
- **`Project/lean` 原样保存、原样返回**：它继续表示旧 Lake 项目，不能写入 Base/context 等新字段；旧 Rust 类型使用
  `deny_unknown_fields`，改写其含义会破坏双后端兼容。LeanGround 的 Base、context、options、minimum_trust
  存入 PostgreSQL 中独立的证明环境；ProofRequest 另存 database、branch、根节点与前提/定义依赖节点的修订快照。
  分支 data_version 只用于审计和回写前置条件，不进入环境或请求身份。旧配置到新环境的映射必须由用户显式选择，不能静默推断。
- **旧的本地认证缓存**（`CACHE/checks-v1`）不迁移、不再读取；界面上旧的「已检查」不能当作正式认证。
- **协作数据**全部在 PostgreSQL，与文档数据没有外键；删除协作项目不影响文档。

---

## 6. 新旧接口对照

### 6.1 文档与图接口（路径前缀 `/p/<库>/<分支>/api/`）

| 旧路由 | 新后端 | 说明 |
|---|---|---|
| `graph/check`、`graph/roots`、`graph/full`、`search`、`resolve` | 移植 | `graph/full` 的 Lean 状态改为协作状态（§7.1） |
| `node/:id/view`、`node/new`、`DELETE node/:id`、`node/:id/title`、`node/:id/block/:type`、`node/:id/dep/add`、`dep/rm`、`dep/candidates` | 移植 | 语义、状态码、ETag 与旧后端一致 |
| `project/latex`、`project/latex/catalog`、`node/:id/latex/context`、`node/:id/latex/preview` | 移植 | 调用同一个 Python 渲染进程 |
| `history`、`branches`、`export`、`import` | 移植 | `web/` 目前未用；协作回写与分支操作需要 |
| `node/:id/dep`、`node/:id/metric/ior` | 待定 | 只有命令行使用 |
| `project/lean` | 兼容保留 | 原样读写旧 Lake 配置；新的 LeanGround 环境使用独立协调接口和 PostgreSQL 记录 |
| `node/:id/lean/check`、`lean/goals`、`lean/session`、`lean/session/:id`、`lean/session/:id/ws` | **移除** | 被协作接口中的「提交」取代；Infoview 扩展另行接入 |

### 6.2 全局接口

| 旧路由 | 新后端 |
|---|---|
| `GET /api/status` | 移植 |
| `/api/projects`（init、remove、new_branch、delete_branch） | 移植；删除类操作需管理员身份 |
| `/api/projects` 的 start、stop，`/api/service/*` | **移除**：新后端一个进程服务所有数据库，没有按分支的守护进程 |

### 6.3 新增接口

- **身份**：所有接口都要求 `Authorization: Bearer <令牌>`，令牌映射到用户；旧的同源检查保留。
- **文档工作区权限**：数据库/分支属于一个工作区，角色为 `owner/editor/viewer`。所有
  `/p/<库>/<分支>/api/...`、历史、导入导出、分支和项目管理请求先核对工作区角色；ProofRequest 只能引用调用者
  已有读取权限的分支。协作项目成员资格不能替代文档权限，也不能因同一分支存在某个 ProofRequest 而自动授权。
- **管理员边界**：创建/删除数据库、导入整库和工作区成员管理要求管理员或工作区 owner；删除仍需独立的明确操作，
  不能由 ProofRequest 删除连带触发。
- **协作**：`/api/coordination/...`，沿用 `coordinator/` 的命令模型（幂等键、`If-Match` 修订号），覆盖项目、目标、任务、
  领取、续租、回报、拆法管理、事实导入、组装、审阅与回写。界面与 Agent 使用同一套接口。

### 6.4 阶段 1 必须冻结的四份契约

在大规模移植接口前，先形成版本化 schema、示例 fixture 与确定性合同测试：

1. **文档权限契约**：工作区、分支、角色、继承规则，以及所有读写/分支/导入导出/删除操作的授权矩阵。
2. **证明环境契约**：环境身份仅由完整 BaseRef、context、options、minimum_trust 决定；不复用 `Project/lean`。
   ProofRequest 另行绑定 database、branch、根节点的 `legacyNodeRevision`，以及前提/定义依赖节点（含传递依赖）的修订快照。
   分支 data_version 仅作审计快照和回写前置条件，不参与环境或 ProofRequest 身份。
   环境改变创建新的 ProofRequest，不能覆盖旧 Goal 状态；根节点或依赖节点修订变化才令旧请求过期，
   同分支无关节点修改不影响环境、请求身份或有效性。fixture 必须分别覆盖这三类节点修改。
3. **声明绑定契约**：一个 Node/Lean block 怎样判定为定义节点或定理节点；定理节点如何选择唯一结论声明，辅助
   声明（含证明内部定义）如何处理，支持哪些源码形态，怎样生成前提占位、`definitions` 定义 ID 列表和 GoalKey
   （§7.3）。多声明或无法判定的节点必须返回明确的 `unsupported_source`，不能猜测。
4. **双向回写契约**：最终认证的哪一份源码回写哪个 block，怎样保持原公开声明名、模块接口、依赖与 metadata；
   拆法引入的定义怎样回写为新的定义节点；多节点回写采用审阅分支上的批次 ID 和可重放操作，任何节点冲突都不把
   旧题面标记为已认证。

阶段 1 的完成条件是四份契约经过固定 fixture 验收，TypeScript 与旧 Rust 对共同字段给出相同结果；不是只有设计文档。

---

## 7. 状态与对象映射

### 7.1 节点状态

| 旧 | 新 |
|---|---|
| 节点 Lean 状态（本地检查：certified / unverified / no_code / 错误） | 拆成两个维度：**本地检查**（仅编辑辅助，可为空）与**正式认证**（来自 LeanGround：`insufficient / derivable / certified`，带查询身份、信任要求、时间、是否截断） |
| Rocq 状态（恒为未验证） | 不变 |

`insufficient` 且被截断时，不能显示成「确认没有路线」。

### 7.2 协作对象与文档对象

| 协作对象 | 绑定的文档对象 |
|---|---|
| DocumentWorkspace | 一个 TerminusDB 数据库及其分支集合；独立保存 owner/editor/viewer，不从 ProofRequest 反推权限 |
| Project / ProofRequest | 绑定 database、branch、根节点及前提/定义依赖节点的修订号；固定 BaseRef、context、options、minimum_trust。分支 data_version 只作审计快照和回写前置条件，不参与身份；无关节点修改不令请求过期 |
| Goal | 按 GoalKey 合并；可关联一个或多个经过明确声明绑定的节点别名 |
| Decomposition | 引用 LeanGround 的条件认证；审阅、暂停、退役状态只在协作层 |
| Task / Attempt | 与文档无关；租约带 `lease_epoch` |
| AssemblyRun | 固定计划 ID、尝试 ID、最终认证 ID |
| 成果回写 | 经审阅后写回绑定节点的 Lean 块，带数据版本前置条件；题面已变则拒绝覆盖 |
| 定义节点 | 一个只含定义的 Lean 块对应 LeanGround 的一次定义登记；定义 ID 写进块的 `metadata`（§7.3） |
| 拆法引入的定义 | 随草图经 `new_definitions` 一并登记；回写时新建定义节点与子目标节点，走同一批次（§6.4 第 4 条） |

### 7.3 节点依赖（Lean import）与 LeanGround 前提：**关键兼容问题**

旧模型里，节点的 Lean 块通过 import 引用依赖节点的**整个模块**。LeanGround 接受的是单文件、不含 import 的源码
（`theorem-file-v1`），依赖按两种方式表达 [源码：LeanGround `3965273`，docs/12]：

- **依赖定理**：写成「前提占位」（证明体为 `sorry` 的 `theorem`），按命题身份 GoalKey 对接；
- **依赖定义**：先把定义登记到 LeanGround，提交时在请求里给出定义 ID（`definitions: [...]`），服务端把定义
  及其依赖放进验证环境。

旧节点数据没有「定义节点 / 定理节点」类型，一个 Lean block 也可能包含多个声明。因此，阶段 1 不能仅凭节点依赖
自动生成证明请求，必须先应用 §6.4 的声明绑定契约。节点分三类：

| 节点的 Lean 块 | 例子 | 转换方式 |
|---|---|---|
| **只含定义**（`def`、`abbrev`、`noncomputable def`、`structure`、`inductive`） | 「完全数」的定义 `IsPerfect` | 调 LeanGround `define` 登记（范围为协作项目），把返回的定义 ID 写进块的 `metadata`。它依赖的定义节点先登记，按 ID 传入 |
| **明确选定一条 `theorem` / `lemma` 为结论** | 欧几里得引理 | 结论名与目标身份均可核对；依赖的定理节点转成前提占位，依赖的定义节点转成 `definitions` 里的定义 ID；本节点连同占位组成一份草图，提交给 `lean-worker-sketch` |
| 其他 | 多个结论、含 `instance` / `class`、定义与定理混在一块而无法区分 | 返回 `unsupported_source`，附具体原因 |

补充规则：

- **陈述级定义与证明内部定义。** 出现在结论或前提陈述里的定义必须是已登记的定义节点（或随提交一并登记）；
  只在证明内部使用的辅助定义可以留在 Lean 块里，不登记，不进身份。陈述引用了未登记定义时，LeanGround 以
  `definition_not_registered` 拒绝。
- **拆法引入新定义。** 拆分时临时定义的辅助对象若出现在子目标陈述里，就是陈述级定义：随草图经
  `new_definitions` 一并登记。LeanGround 在同一事务里写入定义与带前提的认证，失败则都不写入。结论引用新定义时，
  调用方算不出结论身份，可以省略 `goal`。
- **定义不可变。** 定义节点的内容一改，就是新的定义 ID，依赖它的目标都成为新的 GoalKey；旧成果保留在历史中，
  不能标记新版本已认证（与 §9 的题面修改规则一致）。LeanGround 这边不需要失效规则。
- **命题解析。** 引用定义的命题要走需要身份的 `POST /v1/facts/resolve` 并传 `definitions`；旧的
  `/v1/goals/resolve` 不接受定义。
- **可见范围。** 定义对谁可见由 LeanGround 的名字绑定决定。协作服务应把定义登记在协作项目范围内；认证的范围不能
  宽于它用到的定义，公开成果前要先公开相关定义。
- **实例与类型类。** `instance`、`class` 不能登记为定义，只能放在 ProofRequest 固定的项目文件开头（context）里。
  依赖实例的节点，要求其所需实例已在证明环境中；否则返回 `unsupported_source`。

LeanGround 已冻结并实现的部分（原先列为「独立前置项目」须先冻结的内容）：

| 项 | LeanGround 的规定 |
|---|---|
| 定义内容身份 | 定义 ID 为内容哈希，覆盖名字、源码、依赖定义 ID 与环境，不含提交者 |
| 依赖闭包 | 请求只给定义 ID；服务端展开依赖闭包并按依赖顺序放进 worker 前言 |
| GoalKey 如何绑定定义 | GoalKey 的可选字段 `definitions_hash` 只记陈述**直接**引用的定义；不引用定义时与以前完全相同 |
| 允许的声明种类与实例范围 | 见上表；`instance`、`class` 只能在固定的项目文件开头 |
| 变更后的失效规则 | 不需要：定义不可变，改内容即新 ID |

仍由 MathDoc 负责、须在阶段 1 的契约里定下的：

1. 节点 ↔ 定义 ID 的对应，以及定义节点被编辑后如何标记「需要重新登记」；
2. 定义节点、定理节点、子目标节点的**批量回写协议**（并入 §6.4 第 4 条的回写契约）；
3. 同名定义的处理（见 §11）。

**草图检查组件已交付。** 凡是有前提占位的节点，提交的都是条件证明，需要草图检查。LeanGround 的内置组件
`lean-worker-sketch` 输出声明绑定、完整前提集合、每个前提的 GoalKey 与验证证据，并经事实层准入；它另做整体
蕴含检查（前提改写为 `axiom` 后整份文件不得再依赖 `sorry`）。提交时组件与模式必须配对：草图用
`lean-worker-sketch`，完整证明与组装用 `lean-worker`；给 `lean-worker` 传 `mode: "sketch"` 会以
`component_unavailable` 拒绝。

---

## 8. 默认启动需要的服务

| | 旧（`HEAD`） | 新（阶段 5 之后） |
|---|---|---|
| 文档库 | TerminusDB | TerminusDB（同一实例） |
| 应用进程 | `mdc`（Rust） | TypeScript API 进程 + 后台作业进程 |
| LaTeX 渲染 | Python（由 `mdc` 拉起） | Python（由 API 进程拉起，同一脚本） |
| Lean | 本机 elan/Lake（容器内安装） | **不需要** |
| 协作数据库 | — | PostgreSQL |
| 外部服务 | — | LeanGround（事实层，内置草图组件与定义登记） |

LeanGround 的部署前提：`main` 不早于 `3965273`；已执行 `leanground db schema --only 10-definitions`；
worker 已按新版重新安装（旧 worker 会拒绝一切带定义的请求）；草图组件未被 `fact_sketch_enabled=false` 停用。
阶段 4 的部署检查要逐项核对。

配置项：`MDC_TERMINUS_*` 沿用；`MDC_LEAN_TIMEOUT_SECONDS`、`MDC_CACHE_DIR` 中的 Lean 部分废弃；
新增 `MDC_DATABASE_URL`、`MDC_ACTORS`、`LEANGROUND_SERVER_URL`、`LEANGROUND_FACT_TOKEN`、`LEANGROUND_ACTOR`
（与 `config.example.env` 一致）。

---

## 9. 协作领域的关键规则

对象见 §7.2。规则：

- 数学状态、任务状态、尝试结果、拆法管理状态、项目状态**分开记录**。
- 同一项目中 Goal 按 GoalKey 去重；一个 Goal 可以有多个 Task（直接证明、探索拆法、形式化草稿是不同工作）。
- 租约用 `lease_epoch` 防止迟到执行者覆盖新状态；**租约过期只撤销调度权，不否定成果**，有效的迟到证明仍可吸收。
- 预算在领取时预留、结束时结算；掉线不当作「费用未发生」；参与者自己运行的 Agent 只能按上报统计。
- 事件处理与游标推进在同一个本地事务里；通知、查询、组装由事务内写入的待办记录驱动。游标用服务端返回的 `cursor`。
- LeanGround 的查询不知道暂停、退役的拆法，所以**选路在协作层完成**：只从启用的拆法中选完整路线，交给
  LeanGround 固定计划，再组装。
- 组装失败重试用同一计划；换路线就生成新计划；响应丢失先用同一尝试号重放。
- 暂停拆法只影响调度与选路，不删除认证事实；证否关联未实现前，由维护者手动暂停。
- 题面修改后，旧成果保留在历史中，但不能标记新版本已认证。
- 项目完成的条件：根目标取得满足项目信任要求、项目成员可访问的无前提认证。公开成果是之后的独立操作。
- 协作服务身份必须是每个 LeanGround 项目的成员；个人私有成果显式共享后才进入项目。

---

## 10. 分阶段计划与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| **0 保全与回退** | 把 Codex 改动原样提交为存档提交；随后仅把 §4.4、§4.6 指定的默认入口文件恢复到基线 `a4d61e3`；根工作区加入 `web` | 存档提交可定位；旧应用照常构建、旧 CI 通过；`coordinator/` 自身测试通过；恢复提交不丢 `coordinator/` 和 `app/` |
| **1 契约冻结与实现审阅** | 对照 §9 审阅 `coordinator/`；完成 §6.4 的权限、环境、声明绑定、回写四份版本化契约和 fixture | 权限矩阵与负向测试；新旧环境互不污染；同一 fixture 的修订/声明绑定结果确定；评审记录列清修改项和外部阻塞 |
| **2 文档接口对齐** | TypeScript 后端实现 §6.1、§6.2 的移植项、DocumentWorkspace 权限与身份；驱动 Python 渲染；改造 `Projects.svelte` 和 `web/e2e` | `web/e2e` 除本地 Lean 用例外全部通过；`test_renderer.py` 通过；同一节点在两个后端上的修订号一致；两个后端交替写同一数据库不产生冲突之外的错误；直接猜测其他分支 URL 被拒绝 |
| **3 移出本地 Lean** | Lean 块改为「提交给 LeanGround」；状态分两维；Infoview 与 `lean.html` 变为可选构建 | 默认构建产物不含 `lean4monaco`、Infoview、Lean WebSocket 代码；默认启动不需要 elan/Lake |
| **4 协作视图与闭环** | `web/` 中新增证明概览、任务板、拆法视图、成果审阅；Agent 接口；实现 §7.3 的节点转换（定义节点登记、定理节点转草图）；`coordinator/` 改用带定义的解析与提交（§12）；接入真实 LeanGround 与 `lean-worker-sketch` | 见下方完整场景及各项检查；不支持的形态以明确原因拒绝，不阻塞其余节点验收 |
| **5 切换默认入口并清理** | Docker、compose、CI 切到新后端；旧应用保留一个发布周期后删除 Rust 代码；更新文档站 | 容器冒烟测试；在真实 TerminusDB 与 LeanGround 上部署验收 |

**阶段 4 的完整场景**：两位参与者分别证明子引理，共享同一个 B；一位中途掉线，一个拆法被暂停；服务重启后
恢复任务和事件，选出有效路线完成组装，再把结果安全写回原文档版本。场景中至少有一个定义节点，以及一个
引入新定义的拆法。

同时检查：

- 两人竞争时不能同时拿到同一个独占租约；
- 重复事件和重复请求不会重复创建任务或重复扣预算；
- 没有叶子证据的循环不能显示为已证明；
- 无权用户读不到项目的事实、源码或诊断；
- 无权用户也读不到文档工作区、分支、历史、导入导出结果；协作成员资格不会扩大文档权限；
- 题面变化后，旧成果不能覆盖新版本；
- 多节点回写任一点冲突时不产生“当前版本已认证”的状态；同一批次重放不会重复创建节点；
- 定义节点被编辑后，依赖它的目标不再显示为已认证；项目内同名不同内容的定义以明确原因拒绝；
- 默认启动和前端构建不再加载本地证明执行模块；
- 原有的文档编辑、历史、分支和数学展示照常可用（阶段 2 的端到端测试持续通过）。

---

## 11. 风险与待定

| 项 | 影响 | 处理 |
|---|---|---|
| **草图组件与定义登记只在最小环境验收过** | LeanGround 的测试在 `Init` 基座上进行，没有在 Mathlib 基座和真实文档上验收；带参数的 `structure`、`extends`、嵌套 `inductive` 未测 | 阶段 4 用真实文档的 fixture 在 Mathlib 基座上验收；发现问题回报 LeanGround |
| **项目内同名定义** | 旧模型每个节点是独立模块，两个互不 import 的节点可以各自定义同名的 `f`；LeanGround 在一个范围内规定名字只增不改，第二个同名不同内容的定义以 `name_taken` 拒绝，同一请求同时用到两者以 `definition_name_conflict` 拒绝 [推断，未在真实文档上统计] | 阶段 1 统计现有文档里的同名定义；声明绑定契约里规定处理方式（例如改名后再登记），不能静默选一个 |
| LeanGround 源码形态偏严：拒绝 `#` 命令、`open … in`、`@[simp]` 等属性、`private`、`protected`、`local`、`scoped`、`class`、`instance`、`deriving`、`partial`、`unsafe`、`mutual`、自定义记号与宏 | 现有 Lean 块可能大量不合规；代数结构类文档大量使用 `class` / `instance` | 阶段 1 统计现有文档里的比例；实例只能放进固定的项目文件开头；需要时推动 LeanGround 放宽 |
| LeanGround 没有读取项目成员的接口 | 协作层只能单向推送成员名单 | 可暂时接受；建议 LeanGround 补上 |
| 旧后端在内存里保存全图快照 | TypeScript 后端要么同样缓存，要么每次查询 TerminusDB | 阶段 2 用大图做性能对照 |
| `mdc` 命令行 | 旧用户与脚本依赖它 | 待定：保留旧二进制作为命令行、提供 TypeScript 版，或以 HTTP 接口取代 |
| 当前工作区有未提交实现 | 与阶段 0 的存档/恢复范围重叠 | 开始阶段 0 时先记录精确文件清单和基线 `a4d61e3`，存档提交与恢复提交分开，禁止使用笼统的“恢复 HEAD” |

---

## 12. 本计划没有覆盖的

- 当前 `coordinator/` 已有状态机、真实 PostgreSQL/HTTP 合同测试和 `app/` 浏览器测试记录，但尚未接入真实
  LeanGround、真实现有 TerminusDB 数据与保留的 `web/`；这些结果只能作为待审实现的基线，阶段 1 仍需逐条审阅。
- 没有评估 TypeScript 后端移植 `service.rs`、`store.rs` 的工作量细节。
- 没有统计现有文档中各类节点的比例：只依赖定理的、依赖定义节点的、Lean 块里含全局实例的、有同名定义的。
- `coordinator/` 尚未支持定义 [源码，工作区未提交的 `coordinator/src`]：`worker.ts` 解析命题用
  `/v1/goals/resolve`（不接受定义），提交时不传 `definitions` / `new_definitions`，`domain.ts` 允许调用方
  自由组合组件与模式。它对 LeanGround 新增字段是兼容的：身份与认证的解析用 `.passthrough()`，事件同步只处理
  `certificate` 类事件，新的 `definition_registered` 事件会被跳过。这些改动归入阶段 4。
- 千级 Agent 负载、SSO、主动通知渠道、证否关联、自动搜索策略不在本次范围内。
