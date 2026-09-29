# 协调层阶段 2：文档接口对齐

状态：**已实现，待评审**（2026-09-29）。默认入口仍是旧 Rust 应用；阶段 5 才切换。

## 目的

回答：TypeScript 后端能否替代旧 Rust 后端提供文档与图接口，使保留的 `web/` 知识编辑界面照常工作，并与旧后端
共用同一个 TerminusDB 数据库（迁移计划 §6.1、§6.2、§10 阶段 2）。

## 背景

- 旧后端（`src/store.rs`、`src/service.rs`、`src/server.rs`、`src/server/projects.rs`、`src/latex/*` @ `a4d61e3`）
  每个分支是一个需要手动启动的服务，只做同源检查，没有用户身份。
- 阶段 1 冻结了四份 `v1` 契约，其中文档权限契约（决定 D 的授权矩阵）在本阶段首次实现。
- 验收要求（计划 §10）：`web/e2e` 除本地 Lean 用例外全部通过；`test_renderer.py` 通过；同一节点在两个后端上的
  修订号一致；两个后端交替写同一数据库不产生冲突之外的错误；直接猜测其他分支 URL 被拒绝。

## 方法

### 实现位置

| 文件 | 内容 | 对应旧代码 |
|---|---|---|
| `coordinator/src/docs/names.ts` | 名字、UUID、Lean 模块名、按 Rust 规则的规范 JSON | `config.rs`、`store.rs` |
| `coordinator/src/docs/toml.ts` | Lake 配置用的 TOML 子集读取器（只拒不猜） | `toml` crate |
| `coordinator/src/docs/model.ts` | 节点、块、Lean/LaTeX 项目、校验、TerminusDB 文档形式、修订号 | `store.rs`、`latex/project.rs` |
| `coordinator/src/docs/graph.ts` | 拓扑深度、弱连通分量、环检测、分支快照 | `core/algorithms.rs`、`store.rs` |
| `coordinator/src/docs/terminus.ts` | TerminusDB 读写，每次写入带 `TerminusDB-Data-Version` | `store.rs` |
| `coordinator/src/docs/service.ts` | 每个分支的全部文档接口 | `service.rs`、`latex/api.rs` |
| `coordinator/src/docs/latex.ts` | LaTeX 预览进程（同一个 `renderer.py`） | `latex/runtime.rs` |
| `coordinator/src/docs/workspace.ts` | 文档工作区权限（`v1` 契约 + 决定 D 的矩阵） | 新增 |
| `coordinator/src/docs/routes.ts` | `/api/status`、`/api/projects`、`/p/<库>/<分支>/…`、静态资源 | `server.rs`、`web/assets.rs` |

`http.ts` 先交给文档路由处理；协作接口移到 `/api/coordination/...`（计划 §6.3），`/api/me` 保持全局。
`app/` 原型与集成测试已改用新前缀。

### 与旧后端相同的部分

路由、请求体字段（拒绝未知字段）、状态码（428 缺 If-Match、412 修订不符、409 数据版本冲突、404、422）、
错误文字、ETag、节点修订号算法、导入/导出形式、LaTeX 上下文键、SPA 资源规则，以及写入前的全部校验
（UUID、标题、模块名、块类型、依赖、环、模块文件冲突、Lake 项目规则、LaTeX 项目规则）。

### 有意改变的部分

| 变化 | 原因 |
|---|---|
| 所有 API 要求 `Authorization: Bearer <令牌>`；同源检查保留 | 计划 §6.3。旧的「管理请求必须带 Origin」由令牌取代 |
| 分支按需加载，没有 start/stop；`start`/`stop` 动作返回 422 | 计划 §6.2 |
| 每个库是一个工作区；无权读取与不存在一律 404，有读权无写权为 403 | `v1` 权限契约 §2.1 |
| 没有工作区记录的旧库只有管理员可见；管理员用 `set_owner` 认领 | 不从任何其他来源推断权限（只拒不猜） |
| 新分支复制来源分支的授权规则；删分支同时删规则 | 契约没有规定，取最保守且可预期的做法 |
| TerminusDB 提交的作者是操作者名，而不是固定的 `mdc` | 多人协作需要可追溯；历史接口返回原样 |
| 写入 Lake 配置时用 TOML 子集校验，超出子集的语法以 422 拒绝；读取不校验 | 没有批准下载 TOML 库。只拒不猜：可能拒绝旧后端接受的写法，但不会接受旧后端拒绝的；已有数据永远可读 |
| `web/` 登录页与令牌（存于浏览器 `localStorage`）；项目目录去掉运行状态与 start/stop，显示角色 | 计划 §4.2 |

### 顺带修复

阶段 1 的 `legacyNodeRevision`（`contracts.ts`）用 JavaScript 对象排序 metadata：整数形式的键（`"9"`、`"10"`）
被 JavaScript 按数值排序，默认排序又比较 UTF-16 码元，而 Rust 的 `BTreeMap` 按 UTF-8 字节排序。已改为共用
`docs/model.ts` 的规范序列化。**[实测]** 新增 fixture `legacy_revision_key_order`（整数键、含逗号的键、BMP 与
非 BMP 字符），旧实现算出的值与 Rust 不同，新实现与 Rust 测试一致。

## 结果

测试环境：macOS，Node 25，TerminusDB 12.0.7（`compose.yaml` 固定的镜像，临时容器），临时 PostgreSQL，
Playwright chromium。

| 验收项 | 结果 |
|---|---|
| `web/e2e` 除本地 Lean 外 | **12 通过**：项目目录（改写）、大列表、对话框、删除节点、外部修改拒绝过期保存、保存中导航、前进后退、并发反向边、Monaco、Lean 静态首屏、视图切换，以及它们所在的套件 |
| 同上，LaTeX 两项 | **未验证**：「LaTeX macros…」「LaTeX tables…TikZ」停在页面显示的 `start LaTeX renderer`。原因是 plasTeX 运行时没有获准下载，不是移植错误 |
| `test_renderer.py` | **未运行**，同上 |
| 同一节点两个后端修订号一致 | **通过** [实测，`coordinator/test/cross-backend.test.ts`，3 个节点，含特殊 metadata 键、反斜杠与引号标题、控制字符] |
| 两个后端交替写同一库 | **通过**：TS 改名 → Rust 改块 → TS 加依赖 → Rust 删节点（TS 看到解除引用）→ TS 改 LaTeX 项目（Rust 读到）；过期修订两边都是 412 |
| 猜测其他分支 URL 被拒 | **通过**：无角色者对分支 API 与目录都得到 404，无令牌 401，外域 Origin 403 |
| 权限矩阵 | **通过** [实测，`docs.test.ts`，真实 TerminusDB，5 个身份覆盖 admin/owner/editor/viewer/无角色] |
| 数据版本冲突 | **通过**：读写之间有其他提交时写入为 409，服务随后重新加载 |

自动化测试：coordinator 25/25（含真实 PostgreSQL、TerminusDB 与 Rust 二进制，无跳过）；Rust
`test_contract_fixture` 通过；`web` 单元测试 28/28；`web`、`app`、`coordinator` 类型检查 0 错误。

跳过的 e2e（14 项）：全部依赖本地 Lean（Lean 服务器、Infoview、Lean 检查、按分支的 Lean 服务），按计划
§4.2 属于阶段 3 移除的范围，用 `skip` 标明原因，未删除。其中「editors and previews pass scrolling…」以启动
Lean 服务器开始，因此其中与 Lean 无关的滚动检查也暂时没有覆盖。

## 结论

文档与图接口已可由 TypeScript 后端提供，且与旧后端在同一数据库上逐字节一致地计算修订号、互相可见地交替写入。
可以进入阶段 3。LaTeX 预览的实际渲染尚未验证，需要获准安装 plasTeX 3.1 与 pybtex 0.26.1 后补跑两个 LaTeX e2e
与 `test_renderer.py`。

## 未覆盖

- LaTeX 实际渲染（见上）。桥接协议本身已用替身渲染器验证（按键只发送一次项目配置、错误透出、进程退出后重启）。
- 旧后端的内存图在大库上的性能对照（计划 §11）：本阶段每次请求检查一次数据版本，只在版本变化时重新加载，
  与旧后端相同，但没有做大图基准。
- `node/:id/dep`、`node/:id/metric/ior` 已移植，但只有命令行使用，没有端到端测试。
- `mdc` 命令行的去留（计划 §11）；e2e 用一个 HTTP 版的最小替身代替。
- 工作区成员管理只有 API（`grant`、`set_owner`），没有界面。
- 旧库在新后端里只有管理员可见，需要管理员认领；是否要批量迁移旧库的所有者，待定。
- Docker、compose、CI 仍指向旧应用，按计划在阶段 5 切换。
