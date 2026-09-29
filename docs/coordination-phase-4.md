# 协调层阶段 4：协作视图与闭环

状态：**已实现，待评审**（2026-09-29）。维护者决定本阶段起不再维护旧 Rust 应用的兼容（旧 e2e 与跨后端测试未运行）。

## 目的

回答：MathDoc 能否把文档里的一个 Lean 节点连同它的依赖交给 LeanGround 证明，让多人与 Agent 分工补全，
再把认证结果安全写回文档（迁移计划 §7.2、§7.3、§9、§10 阶段 4 的完整场景）。

## 背景

- **ProofRequest（证明请求）**：一个协作项目绑定文档里的一个根节点，固定证明环境（基座 BaseRef、context、
  options、信任要求），并记录根节点及依赖节点 Lean 文本的指纹，用来判断文档是否已变（契约
  `proof-environment.v1`）。
- **GoalKey**：LeanGround 给一个命题算出的身份；陈述相同的节点得到同一个 GoalKey，因此自动共享成果。
- **草图（sketch）**：证明里把依赖的定理写成证明体为 `sorry` 的「前提占位」，由 LeanGround 的内置组件
  `lean-worker-sketch` 检查「前提 ⇒ 结论」；得到的是带前提的条件认证。没有前提的完整证明用 `lean-worker`。
- **组装**：把一条完整路线上的条件认证拼成一份无前提的源码，由 `lean-worker` 从头检查，得到最终认证。
- 阶段 1 冻结了声明绑定、证明环境、回写批次三份契约（`coordinator/contracts/v1/`）和决定 A–D；阶段 3 让 Lean
  块成为普通源码编辑器，并把节点状态分成「本地检查」「正式认证」两维。

## 方法

### 实现位置

| 文件 | 内容 |
|---|---|
| `coordinator/src/lean-source.ts` | 保守的 Lean 源码扫描：按第 0 列切分顶层命令，识别 import 与 context 命令、定义、定理的陈述；决定 B 的节点分类；不认识的一律带原因拒绝 |
| `coordinator/src/proof.ts` | 按分支保存的证明环境；把根节点及其依赖转换成定义登记与草图提交；生成 `proof-environment.v1` 契约与过期判断 |
| `coordinator/src/writeback.ts` | `writeback-batch.v1` 批次：核对、构造、确定性节点 ID |
| `coordinator/src/certification.ts` | 文档视图里每个节点的「正式认证」维度 |
| `coordinator/src/worker.ts` | 新作业：绑定（在 provision 中）、`submit_node`、`writeback`；同步时刷新过期；解析一律走 `/v1/facts/resolve` |
| `coordinator/src/http.ts` | 证明环境接口、按文档创建项目、Agent 草稿接口、文档权限复核 |
| `web/src/components/Collaboration.svelte` | 四个协作视图：证明概览、任务板、拆法、成果审阅 |
| `web/src/components/LeanGroundSubmit.svelte`、`ProofEnvironmentForm.svelte` | 「提交给 LeanGround」；项目设置里的证明环境页 |

### 节点转换（§7.3）

从根节点出发沿依赖收集所有有 Lean 块的节点（没有 Lean 块的节点是非形式说明，不参与），逐个分类：

| 分类 | 处理 |
|---|---|
| 只含 `def`/`abbrev`/`noncomputable def`/`structure`/`inductive` | 按依赖顺序调 `define` 登记在项目范围内，得到定义 ID |
| 唯一一条 `theorem`/`lemma`，或 metadata `lean_conclusion` 指定结论 | 解析陈述得到 GoalKey（带其定义闭包的 ID）；依赖的定理节点写成前提占位；证明不是 `sorry` 就提交（有前提用 `lean-worker-sketch`，否则 `lean-worker`） |
| 证明体是 `sorry` | 只解析陈述，成为开放目标，自动生成证明任务 |
| 其他 | 记录在该节点上：`multiple_conclusions`、`mixed_declarations`、`unsupported_declaration`（含 `instance`/`class`、`namespace`、属性、`open … in`）、`local_context_mismatch`、`premise_name_collision`、`definition_name_taken` 等；依赖它的节点随之以 `ambiguous_binding` / `unregistered_statement_definition` 拒绝，details 写明是哪个节点 |

节点里的 `open`/`set_option`/`universe`/`variable` 必须与证明环境的 context 逐行相等，否则
`local_context_mismatch`；import 行丢弃（依赖由节点关系与定义 ID 表达）。每次提交都生成并用 schema 校验
`declaration-binding.v1` 记录，保存在节点绑定上。

### 与计划措辞不同的实现选择

| 选择 | 原因 |
|---|---|
| 节点 ↔ 定义 ID 的对应保存在协作项目（PostgreSQL），登记时**不**写块 metadata；只有回写新建的定义节点带 `definition_id` | 登记时写 metadata 会改变节点修订号，正在编辑该节点的人会遇到 412 冲突。定义 ID 按内容寻址，重新登记得到同一 ID，所以不需要把它存进文档才能找回 |
| 回写直接写入绑定分支，由项目所有者在「成果审阅」里接受；契约的 `review_branch` 即绑定分支 | 另开审阅分支需要分支合并流程，docs/12 与 TerminusDB 适配都没有；原子性与冲突由单次提交保证（下一节） |
| 证明环境按分支保存，只有 owner/admin 可改（沿用 `member_manage` 的角色集合） | 环境决定之后每个请求的含义；改环境只影响新请求，旧请求保留原环境（`replaces_environment_id` 记历史） |
| 项目成员都必须对绑定分支有读权限；每个项目接口都重新核对调用者的文档读权限 | 计划 §6.3：协作成员资格不能扩大文档权限。成员能读认证源码，而源码就是文档内容 |

### 回写（§6.4 第 4 条，决定 C）

接受一次已认证的组装后，作业读取路线上每条认证的源码，构造一个批次：

- 路线上的目标对应已有节点：先核对该节点的 Lean 文本仍是转换时的文本，否则整批 `aborted`
  （`document_conflict`）；由节点原文转换来的认证只写 metadata，Agent 提交的证明替换声明部分并保留节点原有
  的 import/context 行；结论名必须保留（否则 `public_declaration_renamed`）。
- 拆法引入、没有对应节点的子目标：新建目标节点；认证用到、文档里没有节点的定义：新建定义节点。新节点 ID 由
  批次 ID 与 GoalKey/定义 ID 派生，重放不会重复创建。
- 根节点 metadata 写入最终认证 ID；所有写入节点带 `certification_id`、`certified_lean_sha256` 与批次 ID。
- 整批一次 TerminusDB 提交，带读取时的数据版本；中途有人写入则整批重试并重新核对。已提交的批次由根节点上的
  批次 ID 识别，响应丢失后重放直接记为已提交。

### 过期（§9「题面修改」）

- 请求级：同步时按契约 `proofRequestIsStale` 比较根节点、直接前提、定义闭包的 Lean 文本；过期后拒绝接受
  （`proof_request_stale`），旧成果保留。
- 节点级（文档视图）：节点自己或它陈述用到的定义节点的 Lean 文本与提交时不同，就显示「Changed since
  submission」，不再显示已认证。回写把请求自己写入的文本记为新的基准，不会被自己的回写打成过期。

### Agent 接口

人与 Agent 用同一组接口（`Authorization: Bearer <令牌>`；命令带 `If-Match: "<项目修订号>"` 与
`Idempotency-Key`）：

| 接口 | 用途 |
|---|---|
| `GET/PUT /api/coordination/environments/<库>/<分支>` | 读取 / 设置证明环境（`base_key`、`context`、`options`、`minimum_trust`） |
| `POST /api/coordination/projects` `{document:{database,branch,node}, members, budget}` | 以文档节点创建证明请求 |
| `GET /api/coordination/projects?database=&branch=&node=` | 查某分支或某节点所在的项目 |
| `GET /api/coordination/projects/<id>/nodes/<节点>/draft` | 该节点的提交草稿：前提占位 + 节点声明、定义 ID、GoalKey、组件与模式 |
| `POST …/commands` | `claim`/`heartbeat`/`finish`（租约）、`submit`（可带 `definitions`、`new_definitions`）、`resolve`（可带 `definitions`）、`submit_node`、`decomposition`、`assemble`、`retry`、`accept` |
| `GET …/facts/<认证 ID>` | 读认证（含源码与前提绑定名） |

典型流程：领取任务 → 取草稿 → 把结论的 `sorry` 换成证明（或写成新的草图、用 `new_definitions` 引入定义）→
`submit` → `finish`。拆法产生的新前提只有 GoalKey，先按认证里的前提名写出命题，用 `resolve` 取得身份，再提交。

## 结果

测试环境（2026-09-29，本机 macOS）：LeanGround `main` `3965273`，`Init` 基座，worker 为仓库内
`.lake/build/bin`；TerminusDB 12.0.7（`compose.yaml` 固定的镜像，临时容器）；临时 PostgreSQL 17；Playwright chromium。

### 完整场景（`coordinator/test/scenario.test.ts`，真实 LeanGround + TerminusDB + PostgreSQL）

**[实测，n=1 次完整运行，约 34 秒，其中 31 秒是等待租约过期]** 文档：定义节点 `double`；开放引理
`double_eq`（B，证明为 `sorry`）；两个开放子引理 `double_two`、`double_three` 都依赖 B；根 `sum_doubles`
依赖两个子引理与定义。逐项结果：

| 计划 §10 的要求 | 结果 |
|---|---|
| 两位参与者分别证明子引理，共享同一个 B | 通过：两个子引理的草图前提都是 B 的同一个 GoalKey，B 只有一个目标、一个任务 |
| 一位中途掉线 | 通过：租约过期后心跳被拒（`stale_lease`），按预留额计费，任务回到可领取；他迟到的有效证明仍被吸收 |
| 一个拆法被暂停 | 通过：B 的循环拆法（前提就是 B 自身）被记录但不使 B 成立，暂停后组装路线不含它 |
| 引入新定义的拆法 | 通过：B 的另一拆法用 `new_definitions` 登记 `twice`，两个新子目标以叶子证明补全 |
| 服务重启后恢复任务和事件 | 通过：换一套 API/worker 实例继续，事件游标不回退 |
| 选出有效路线完成组装 | 通过：根目标 `certified`，文档视图显示 Certified |
| 结果安全写回 | 通过：先改动 B 的文本 → 整批 `aborted`、文档节点数与子引理内容不变；恢复文本后同一批次提交，新增 `double_twice`、`twice_eq`、`twice` 三个节点，B 与子引理的 `sorry` 被替换；再次接受不新增节点 |
| 两人竞争同一独占租约 | 通过：并发领取只有一个成功；同一幂等键重放返回同一结果，预留不重复 |
| 没有叶子证据的循环不显示为已证明 | 通过（见「一个拆法被暂停」） |
| 无权用户读不到项目、事实、源码、草稿与文档 | 通过：无角色者 404；只读者建请求 403；把无读权者加为成员被拒 `member_lacks_document_access` |
| 定义节点被编辑后不再显示已认证 | 通过：改 `double` 后根节点显示 stale，同步后请求过期，接受被拒 `proof_request_stale` |
| 同名不同内容的定义、不支持的形态 | 通过：另一请求里第二个 `double` 为 `definition_name_taken`，`instance` 节点为 `unsupported_declaration`，根节点被拒并在 details 写明原因；第一个请求不受影响 |

### 其余

- coordinator 测试：30 通过、1 跳过（旧 Rust 跨后端测试，按维护者决定不再运行）。新增扫描器单元测试 5 项。
- `web/e2e`（TypeScript 模式，默认构建，n=1 次）：13 通过，其中新增「a Lean node is submitted to LeanGround,
  assembled and written back from the collaboration views」在浏览器里完成：设置证明环境 → 提交节点 → 概览显示
  derivable → 拆法视图显示条件认证 → 组装 → 写回 → 节点显示 Certified。2 项失败为两个 LaTeX 用例（plasTeX
  运行时未获准下载，阶段 2 起即如此），16 项因本地 Lean 或旧后端语义跳过。
- `web` 单元测试 28/28；`web`、`app`、`coordinator` 类型检查 0 错误。

### 部署检查发现的问题

- **[实测]** 本机按配置安装的 worker（`~/.local/share/leanground/bin`，9 月 24 日构建）早于定义登记：`define`
  返回 `definition_is_proposition`，与 LeanGround docs/12 描述的旧 worker 行为一致。测试用 `LEANGROUND_BIN`
  指向仓库新构建的 worker。部署前必须重新运行 LeanGround 的 `scripts/install_tools.sh`（计划 §8 的部署检查项）。
- **[实测，测量伪影]** 场景测试最初几次失败：项目 ID 由（操作者, 幂等键）派生，而 LeanGround 数据在测试之间
  保留，复用同一幂等键让新一轮测试吸收了上一轮的认证。已改为每轮用不同的键。这不是协调层的错误，但说明
  **[推断]** 两套 MathDoc 部署共用一个 LeanGround 且同一操作者用同一幂等键建项目时，会落进同一个 LeanGround
  项目范围。

## 结论

阶段 4 的完整场景在真实 LeanGround（`Init` 基座）、TerminusDB 与 PostgreSQL 上通过：文档节点可以转换成定义
登记与草图，多人与 Agent 通过同一接口分工，组装得到最终认证，结果以不产生部分认证的批次写回；题面或定义
改动后不再显示已认证。可以进入阶段 5（切换默认入口），但 Mathlib 基座上的验收（下节第一项）应在切换前补上。

## 未覆盖

- **Mathlib 基座与真实文档**：只在 `Init` 基座、手写的小文档上验收。计划 §11 的风险（带参数的 `structure`、
  `extends`、`class`/`instance` 大量出现、现有文档中不合规源码的比例）仍未统计。
- 查询截断：节点状态由协作层对项目可见事实的推导得出，没有调用 LeanGround `query`，因此 `truncated` 恒为假；
  截断显示的界面代码有，但没有被真实数据触发过。
- 拆法引入的新子目标只有 GoalKey：LeanGround 没有按 GoalKey 读陈述的接口，Agent 需要自己写出命题再 `resolve`。
  回写新建的目标节点以结论名为标题。
- 依赖 `instance` 节点的定理报 `ambiguous_binding`（details 写明原因），没有细分为契约里的
  `missing_environment_instance`。
- 扫描器接受含 `'` 的声明名，LeanGround 拒绝（`unsupported_source`），在提交时才报出。
- 租约过期只在下一次状态变化（命令或周期同步）时记录。
- 项目 ID 与 LeanGround 项目范围在多部署共用 LeanGround 时可能相撞（见上）。
- 界面：没有工作区成员管理界面；协作视图每 5 秒轮询，没有推送；只有桌面宽度下做过浏览器验证，未做截图审阅。
- 旧 Rust 应用：按维护者决定不再维护，本阶段没有运行旧后端 e2e 与跨后端修订号测试。
