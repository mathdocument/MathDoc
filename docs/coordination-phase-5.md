# 协调层阶段 5：切换默认入口并清理

状态：**已实现，待评审**（2026-09-29）。

## 目的

回答：能否让 MathDoc 的默认构建、部署、CI 和命令行都只用 TypeScript 后端，删除旧 Rust 应用，同时不丢失
仍在使用的功能（迁移计划 §4.4、§4.5、§8、§10 阶段 5）。

## 背景

- 阶段 2–4 让 TypeScript 后端（`coordinator/`）提供了文档接口、协作接口和 `web/` 编辑器，但 Docker 镜像、compose、
  CI 仍构建旧 Rust 应用；旧应用自带本地 Lean（elan/Lake、lean4monaco 编辑器、Infoview）。
- 维护者在本阶段前确定了四件事：旧 Rust 代码直接删除；写一个薄的 TypeScript 版 `mdc` 命令行；切换前先在 Mathlib
  基座上验收；允许重装本机的 LeanGround worker。另外批准了镜像构建所需的下载（plasTeX 3.1、pybtex 0.26.1、
  `postgres:17-bookworm` 镜像）。
- **GoalKey、ProofRequest、草图、组装** 等术语见 `docs/coordination-phase-4.md`「背景」。

## 方法

### 1. 切换前的验收（Mathlib 基座）

- **worker 重装**：运行 LeanGround 的 `scripts/install_tools.sh`。
- **Mathlib 源码统计**：新脚本 `perf/lean-block-scan.ts` 用节点扫描器（`coordinator/src/lean-source.ts`）扫描
  Mathlib 源码，按两种粒度统计能否转换：整个文件当作一个 Lean 块；每条顶层命令单独当作一个块。
- **Mathlib 端到端**：新测试 `coordinator/test/mathlib.test.ts` 在真实 LeanGround（Mathlib 基座）、TerminusDB、
  PostgreSQL 上跑一份手写文档：带参数且带 Mathlib 类参数的 `structure`、`extends`、ℝ 上的 `noncomputable def`、
  `lemma`、与证明环境一致的 `open Finset`；再把阶段 4 的完整场景测试改到 Mathlib 基座上跑一次。
- 本机没有真实的 MathDoc 文档库（colima 中没有 TerminusDB 数据卷），所以「真实文档」由上面两者代替。

### 2. 删除与迁移

| 内容 | 处理 |
|---|---|
| `src/`（Rust 后端、本地 Lean、旧 CLI）、`Cargo.*`、`tests/*.rs`、`Dockerfile.legacy`、`compose.legacy.yaml`、`legacy-check.yml`、`backend-perf.yml`、跨后端测试 | 删除 |
| `src/latex/` 的 Python 渲染器、`amsalpha.*`、`test_renderer.py`、`requirements.txt` | 移到 `renderer/`；后端默认从这里启动渲染进程（`MDC_RENDERER_DIR` 可改） |
| `web/`：`lean.html`、`lean-editor.ts`、`build/lean-socket.ts`、`LeanBlock.svelte`、`LeanProjectForm.svelte`、`lean-api.ts`、`lean-module.ts`、`features.ts` 与构建开关 `MDC_WEB_LEAN_EDITOR`、依赖 `lean4monaco` | 删除；Monaco 字体从 lean4monaco 附带的 JuliaMono 改为系统等宽字体 |
| `web/package-lock.json` | 删除，统一使用根工作区的锁文件 |
| 项目目录的 start/stop、运行状态筛选 | 删除（分支按需加载） |
| e2e 的 Rust 装置、`MDC_E2E_BACKEND` 开关、本地 Lean 用例、旧项目目录用例 | 删除 |

### 3. 新的默认入口

- `Dockerfile`：Node 镜像，构建后端与 `web/`，内含 LaTeX 渲染运行时；入口 `node coordinator/dist/main.js`，
  命令 `serve` / `worker` / `migrate`；`mdc` 在 PATH 上；镜像内没有 Lean。
- `compose.server.yaml`：`terminusdb`、`postgres`、一次性的 `migrate`、`mdc`（API）、`worker` 五个服务；
  LeanGround 由 `LEANGROUND_SERVER_URL` 指向外部。`compose.yaml`（开发）只起 TerminusDB 与 PostgreSQL。
- `config.example.env` 列出全部变量；新增 `MDC_POSTGRES_PASSWORD`（compose 用）。
- CI：`release-check.yml` 改为 Node 流水线（类型检查、单元测试、构建、文档站、渲染器测试、真实 TerminusDB 与
  PostgreSQL 上的后端测试、浏览器 e2e、容器冒烟）；`frontend-perf.yml` 改用根锁文件并兼容旧基线提交。
- `tests/docker-smoke.py` 重写：令牌、CLI、LaTeX 预览、worker 存活、重启与重建后数据仍在、镜像内无 Lean。

### 4. `mdc` 命令行

`coordinator/src/cli.ts`，通过 HTTP 接口加令牌工作（`MDC_URL`、`MDC_TOKEN`、`MDC_PROJECT` 或 `-p`）。保留旧
CLI 的文档命令（status、init、remove、branch、history、export、import、project latex、graph、search、show、new、
del、rename、edit、dep、metric）；去掉 start/stop、`lean check/goals`、Lake 项目设置；新增 `grant` 与
`proof env|submit|list|status|command`。`scripts/mdc-docker` 在 compose 部署里调用它。

### 5. 文档

`README.md`、`web/README.md`、文档站（`docs/src/content/docs/`）与迁移计划状态行。文档站由一个子代理对照代码改写
（新增「Collaboration」「Proofs with LeanGround」「Agent Interface」三页，删除本地 Lean 编译相关的三页），我抽查了
部署页与 Agent 页并用冒烟测试验证了部署页给出的 `docker compose … up -d --wait` 命令。

### 6. 核对文档时发现并修正的问题

| 问题 | 来源 | 修正 |
|---|---|---|
| 文档接口收到格式错误的请求体时返回 500 | 阶段 2 引入：请求体读取抛出协作层的 `Fault`，文档路由只处理 `DocError` | 文档路由把它映射为 400 / 413 |
| 文档接口请求体上限 2.2 MB，旧后端是 256 MiB | 阶段 2 引入：文档路由沿用了协作接口的上限，大图谱无法 `import` | 分支路由恢复 256 MiB，协作接口仍为 2.2 MB |
| 本地构建后 `coordinator/dist/cli.js` 没有可执行位 | `tsc` 不设可执行位 | `build` 脚本加 `chmod +x` |

两项接口修正由 `coordinator/test/cli.test.ts` 覆盖（格式错误的请求体得到 400；3 MB 的请求体被解析而不是 413）。

## 结果

测试环境：macOS；LeanGround `3965273`，**Mathlib 基座**（`base/mathlib` 提交 `0df444a`，Lean v4.33.1），
使用重装后的 worker；TerminusDB 12.0.7 与 PostgreSQL 17，均为临时实例。

### 部署前提检查（计划 §8）

- **[实测]** 重装后 worker 一启动就被系统以 SIGKILL 结束（退出码 137），同一二进制在仓库构建目录里能正常运行。
  **[推断]** 原因是安装脚本原地覆盖了已签名的可执行文件，macOS 按旧签名缓存拒绝运行。把
  `~/.local/share/leanground/bin` 下每个文件复制成新文件再改名覆盖后，全部可以启动。LeanGround 的
  `install_tools.sh` 应改为「写新文件再改名」，否则下次重装会复现。
- 你本机端口 8765 上运行的 `leanground-server` 需要重启才会用上新 worker；我没有动它。

### Mathlib 源码统计（`perf/lean-block-scan.ts`，n = 8,311 个文件、534,073 条顶层命令，确定性扫描，1 次）

| 粒度 | 结果 | 所以 |
|---|---|---|
| 整个文件当作节点 | 0 / 8,311 可转换（8,303 个含不支持的命令，8 个无法判定） | 把 Mathlib 这类库文件整份导入成节点，转换器一个也接不住；MathDoc 的节点需要按陈述粒度编写 |
| 每条顶层命令 | 定理 31.6%、定义 6.4% 可直接转换；上下文命令（variable、set_option、open、universe）13.3%；其余被拒，主要是属性 `@[…]`（约 18%）、section/namespace/end（11.6%）、instance（4.8%）、protected/private | 按陈述拆开后，约四成声明能直接进 LeanGround；带属性、instance、命名空间的写法要么改写，要么由证明环境的固定上下文提供（计划 §11 的风险属实） |

测量中发现两个问题并已修正：第一次统计把注释块里的文字（版权声明、`-/` 等）当成了命令，是统计脚本按原始行
切分造成的**测量伪影**，改为按扫描器屏蔽注释后的文本切分；扫描器把 Lean 新模块语法（`module` 文件头、
`public import`）当成不支持的命令，已改为按 import 处理并补了单元测试。

### Mathlib 端到端

- `mathlib.test.ts`：**[实测，n = 2 次]** 通过。三个定义节点（带参数 structure、extends、noncomputable def）登记
  成功；根节点作为草图提交；Agent 按草稿提交叶子证明；**错误证明（`le_refl 0`）被拒**，说明通过不是空检查；组装、
  写回成功，根节点显示 Certified；`lemma` 与 `open Finset` 节点作为叶子直接认证。worker 已热时整个测试约 2 秒，
  首次加载 Mathlib 约 30 秒。
- 阶段 4 完整场景（`scenario.test.ts`）在 Mathlib 基座上：**[实测，n = 1 次，约 35 秒]** 通过。

### 切换后的检查

| 检查 | 结果 |
|---|---|
| 后端测试（真实 TerminusDB、PostgreSQL、Mathlib LeanGround） | 33 项，无跳过（含新增的 CLI 测试与 Mathlib 测试）。全套 4 次（场景测试改为不限定基座之后）：3 次全部通过；1 次文档权限测试失败，单独重跑 4 次均通过。**[推断]** 是并行运行的测试文件共用同一个 TerminusDB 造成的干扰，未查实 |
| `web` 单元测试 | 27/27（删去了只服务于本地 Lean 的 `lean-module` 测试） |
| 类型检查（`web`、`coordinator`、`app`） | 0 错误、0 警告 |
| 默认构建 | 不含 Infoview、lean4monaco、JuliaMono |
| 浏览器 e2e（全套） | 第一次 15 项中 11 通过：两个 LaTeX 用例失败（当时本机没有渲染运行时），项目目录用例失败（我改了删除确认文案，已恢复，之后 5/5 通过），Monaco 用例失败（见下）。修正后再跑全套 1 次：**15/15 通过** |
| 两个 LaTeX e2e 用例 | 装好渲染运行时后通过。**自阶段 2 起首次实际验证 LaTeX 渲染**。修掉了一个一直被运行时缺失掩盖的测试装置问题：写令牌的初始化脚本也在沙箱化的预览 iframe 里执行并抛错 |
| `renderer/test_renderer.py` | 15/15 |
| 容器冒烟（`tests/docker-smoke.py`，2 次） | 均通过：非 root 运行、无令牌 401、容器内 CLI、LaTeX 预览、worker 存活、重启与重建后数据仍在、镜像内无 Lean；第二次使用文档给出的不带服务名的 `up -d --wait`（含一次性的 `migrate`） |
| 文档站 | `npm run check` 0 错误；`npm run build` 25 页；子代理检查了站内链接与锚点 |

**Monaco 用例的不稳定**：**[实测]** 切换后共运行 16 次，失败 2 次，两次症状不同（一次是文本块首屏无语法着色，
一次超时），其余 14 次通过。阶段 3 在字体改动之前观察到约 1/9。样本太少，不能判断去掉字体加载等待是否让它变差；
原因未定，列为未解决的不稳定用例。

## 结论

默认构建、部署、CI 与命令行已切到 TypeScript 后端，旧 Rust 应用与本地 Lean 已删除；新镜像在容器冒烟测试中通过，
协作闭环在 Mathlib 基座上通过，LaTeX 渲染首次得到实测。按计划可以视为阶段 5 完成，待评审；仍需在真实部署
环境（真实用户数据与生产 LeanGround）上做一次部署验收。

## 未覆盖

- **真实文档库**：本机没有现存的 MathDoc 文档数据；Mathlib 统计与手写文档是替代。已有文档迁到新后端后仍需按
  `perf/lean-block-scan.ts` 统计并处理被拒节点（旧库还需要管理员用 `set_owner` 认领，见阶段 2）。
- **真实部署验收**（计划 §10 阶段 5 的第二项）：没有在生产 TerminusDB 与生产 LeanGround 上部署。
- **CI 未在 GitHub 上运行**：新的 `release-check.yml` 只在本机按相同步骤分别验证过；依赖 LeanGround 的测试在 CI
  中会跳过（CI 没有 LeanGround）。
- Monaco 用例的不稳定（见上）；「editors and previews pass scrolling…」随本地 Lean 用例一起删除，其中与 Lean
  无关的滚动检查从此没有覆盖。
- 编辑器字体换成系统等宽字体后没有做截图比对；Lean 的 Unicode 符号显示依赖系统字体。
- 文档站里的 curl 与 CLI 示例按代码写成，没有逐条对真实服务执行（部署页的 compose 命令除外）。
- `web/perf/baseline.json` 的体积基线仍是移除本地 Lean 之前的数值，前端性能对比会显示体积大幅下降，需要重新记录基线。
- LeanGround 安装脚本原地覆盖二进制的问题只在本机绕过，没有改 LeanGround 仓库。
- `app/`（Codex 的协作原型）仍保留为存档，只做了类型检查。
- 本机为跑冒烟测试做过两处环境处理：`brew install docker-compose`（已安装，无变化）并把
  `~/.docker/cli-plugins/docker-compose` 从已卸载的 Docker Desktop 改指向 Homebrew 的插件；测试时用一个临时
  Docker 配置绕过失效的 Docker Desktop 凭据助手，没有改 `~/.docker/config.json`。
