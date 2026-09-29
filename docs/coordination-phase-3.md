# 协调层阶段 3：移出本地 Lean

状态：**已实现，待评审**（2026-09-29）。默认入口仍是旧 Rust 应用；阶段 5 才切换。

## 目的

回答：能否让默认构建与默认启动不再依赖本地 Lean（lean4monaco、Infoview、Lean WebSocket、elan/Lake），
同时不破坏仍是默认入口的旧应用（迁移计划 §4.2、§7.1、§10 阶段 3）。

## 背景

- 旧界面的 Lean 块是一个独立页面 `lean.html`（iframe），内含 lean4monaco 编辑器、Infoview 与到后端本地 Lean
  服务器的 WebSocket；Lake 项目在「项目设置」里配置。
- `web/` 同时被两个后端使用：旧 Rust 应用把它嵌入二进制（阶段 5 之前仍是默认），TypeScript 后端直接提供它。
- 本阶段的范围由维护者确定：**只移出本地 Lean**。「提交给 LeanGround」按钮与证明环境设置在阶段 4 随
  ProofRequest 与声明绑定一起上线，本阶段不放一个接不通的按钮。

## 方法

### 可选构建

`MDC_WEB_LEAN_EDITOR=1` 时，`vite build` 与原来完全相同（`lean.html`、Infoview、Lean WebSocket 开发插件、
原生 Lean 块）。不设时：

- 不构建 `lean.html`，不复制 Infoview，不加载 Lean WebSocket 插件；
- 编译期常量 `__MDC_LEAN_EDITOR__` 为假，原生 Lean 块（`LeanBlock.svelte`）与本地 Lean 调用
  （移到 `lib/lean-api.ts`）不会被引入；
- Lean 块改用与其他源码块相同的 Monaco 编辑器，语法用 Shiki 的 `lean4` 语法文件；
- 「项目设置」只保留 LaTeX 页；`Project/lean` 数据原样保留（计划 §5），接口不变。

旧应用的构建都设置了该变量：`Dockerfile`、`Dockerfile.legacy`、`release-check.yml`、`legacy-check.yml`。

### 状态分两维（计划 §7.1）

`formalization.lean` 保持原义：本地检查。新增 `formalization.lean_certification`：

| 值 | 含义 |
|---|---|
| `null` | 没有 Lean 代码 |
| `{status: "not_submitted"}` | 有 Lean 代码，尚未绑定到 ProofRequest（阶段 4 之前总是如此） |
| `{status: "insufficient" \| "derivable" \| "certified", goal, minimum_trust, checked_at, truncated}` | 来自 LeanGround 的查询；阶段 4 填充 |

界面按认证维度显示（「Not submitted to LeanGround」等），`insufficient` 且截断时显示「search truncated」，
不能读成「确认没有路线」。旧后端不发送该字段，界面照旧显示本地检查。

### 顺带修复阶段 2 的回归

阶段 2 的提交 `e26ca79` 让 `web/` 只适配新后端：登录页依赖旧后端没有的 `/api/me`，项目目录去掉了旧后端需要的
启动/停止；e2e 也改成只驱动新后端。由于旧应用嵌入同一份 `web/`，这会让默认入口不可用、旧 CI 失败。本阶段改为：

- 启动时请求 `/api/me`：401 显示登录，404（旧后端）直接打开，其余直接打开；
- 项目目录按后端的 `server.on_demand` 决定：新后端显示角色、没有启动/停止；旧后端恢复原样；
- e2e 用 `MDC_E2E_BACKEND` 选择后端，**默认 `rust`**：旧 CI 与迁移前完全一样地运行；Rust 模式的装置与项目
  目录测试逐字恢复。

## 结果

测试环境：macOS，TerminusDB 12.0.7（临时容器），临时 PostgreSQL，Playwright chromium。

| 验收项 | 结果 |
|---|---|
| 默认构建不含 lean4monaco、Infoview、Lean WebSocket 代码 | **通过** [实测：在 `web/dist` 中搜索 `@leanprover/infoview`、`InfoviewApi`、`LeanMonaco`、`lean.html`、`/lean/session`、`/lean/check`、Infoview iframe 标题，均为 0 个文件；同样的搜索在带标志的构建中都能找到，说明搜索有效] |
| 默认启动不需要 elan/Lake | **通过** [实测：从 `PATH` 去掉 elan 目录（`lake`、`lean`、`elan` 均不可见）后运行 TypeScript 模式 e2e] |
| 新的 Lean 块 | **通过**：新增 e2e「Lean blocks are source editors certified by LeanGround…」——语法着色、无 iframe、无「Start Lean server」、状态为「Not submitted to LeanGround」、保存只写块内容、设置里没有 Lean 页、全程没有本地 Lean 请求 |
| 旧应用不受影响（Rust 模式，带标志构建） | **25 通过**，含全部 13 个本地 Lean 用例与原项目目录测试 |
| TypeScript 模式（默认构建） | 12 通过（含新 Lean 块测试） |

两种模式都失败的：两个 LaTeX 用例（没有 plasTeX 运行时，未获准下载）。Rust 模式另有「editors and previews
pass scrolling…」失败；**[实测]** 用迁移前的 `web/`（提交 `0a36348`）在同一机器、同一后端上运行该测试，同样在
`nativeBoundary` 超时，因此是本环境下原有的失败，不是本阶段引入的。

观察到的不稳定：「Monaco source blocks retain highlighting…」在本次会话约 9 次运行中失败 1 次（撤销后仍显示
Unsaved，失败的是 text 块），单独重跑 5 次均通过；未在迁移前代码上复现，原因未定。

其余：coordinator 25/25（真实 PostgreSQL、TerminusDB、Rust 二进制，无跳过，含新的认证字段断言）；`web`
单元测试 28/28；三套类型检查 0 错误。

## 结论

默认构建与默认启动已不再依赖本地 Lean；旧应用在带标志的构建下保持原有全部 Lean 功能。可以进入阶段 4：
接入 ProofRequest、声明绑定、证明环境设置与「提交给 LeanGround」，并填充认证维度。

## 未覆盖

- 「提交给 LeanGround」与证明环境设置：按维护者决定放在阶段 4。
- 图视图的颜色仍按本地检查维度（新后端下有 Lean 代码的节点都显示为「未验证」色）；阶段 4 有认证数据后改为按认证着色。
- LaTeX 实际渲染与 `test_renderer.py`：同阶段 2，等待 plasTeX 运行时。
- JuliaMono 字体仍从 `lean4monaco` 包的字体文件打包（是字体资源，不是代码），因此默认构建仍需安装该包。
