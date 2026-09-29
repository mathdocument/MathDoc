# 协调层阶段 1：v1 契约与实现自查

状态：**契约已冻结为 `v1`（2026-09-29）**。四个产品决定 A–D 已由维护者按推荐方案确定（§3），各自已编码进
schema 与测试。§5 自查中的「不满足」「不确定」项不因冻结而改变，属于后续阶段的实现工作（§6）。
冻结契约不等于授权阶段 2；阶段 2 另行交代。

依据：MathDoc `docs/coordination-migration-plan.md` §6.4、§7.3、§9；LeanGround 只以 `main` 的
`3965273` 中 `docs/12-LeanGround-事实层接口与验收.md` 为接口依据。本文不假设 docs/12 以外存在声明解析、
成员读取、事务回调或批量回写接口。

## 1. 交付物与版本边界

四份机器可读契约均为 JSON Schema 2020-12，版本为 `v1`（`schema_version` 形如 `mathdoc.<契约>.v1`）。修改任何
一份都要升版本，并同步 Zod、生成的 schema、fixture 与测试：

| 契约 | Schema | 固定 fixture / 测试 |
|---|---|---|
| 文档权限 | `coordinator/contracts/v1/document-permission.schema.json` | `coordinator/test/contracts/v1/contracts.fixture.json`；owner/editor/viewer/admin 的动作矩阵和 viewer 写入负例 |
| 证明环境 | `coordinator/contracts/v1/proof-environment.schema.json` | 环境身份与请求绑定独立；无关节点、根节点、依赖定义三种修改场景验证；旧环境由 `replaces_environment_id` 引用而非覆盖 |
| 声明绑定 | `coordinator/contracts/v1/declaration-binding.schema.json` | 明确覆盖 `definition_only`、`theorem`、`unsupported` 三类；源码 SHA-256、组件/mode 配对及猜测形态负例 |
| 双向回写 | `coordinator/contracts/v1/writeback-batch.schema.json` | 定义节点 + 根定理节点的同批操作、节点修订前置条件、可重放键、冲突负例 |

`coordinator/src/contracts.ts` 是 TypeScript 的运行时 Zod 镜像，并提供稳定 JSON 摘要和旧节点修订算法。
`coordinator/test/contracts.test.ts` 验证四份 fixture；`tests/test_contract_fixture.rs` 让旧 Rust 对同一个节点 fixture
计算相同修订号。声明绑定是新增能力，旧 Rust 没有对应结果可比，因此只要求同一输入在 TypeScript 中重复得到同一
解析结果，不能声称与旧 Rust 对齐。

## 2. 四份契约

### 2.1 文档权限契约

- 权限对象是 DocumentWorkspace 及其分支，不从协作 Project/ProofRequest 的成员资格反推。
- 分支明确记录是否继承工作区角色以及分支增量授权。
- 动作集合显式覆盖读、写、历史、建/删分支、导入导出、建/删数据库、成员管理、创建 ProofRequest。
- fixture 当前矩阵是评审起点：viewer 只读，editor 可写和建分支，owner/admin 承担删除、导入与成员管理，数据库
  创建/删除只允许 admin。矩阵通过评审前不是生产授权规则。
- 负向要求：无权访问与资源不存在对外都不得泄露内容；ProofRequest 创建还必须检查其绑定分支的读取权限。

### 2.2 证明环境契约

- 环境身份只取完整 BaseRef、context、options、minimum_trust（`environmentIdentity`）；environment_id 是标签，不作为哈希输入。
- ProofRequest 在 `proof_request` 中另行绑定 database、branch、根节点与依赖节点的 UUID 和 Lean 内容指纹
  `lean_content_sha256`（`leanContentSha256`：Lean 块文本的 SHA-256，不含标题、其他块和 metadata）。依赖由
  `proofRequestSnapshot` 计算：定义依赖（`relation: definition`）取传递闭包，定理前提（`relation: premise`）只取
  直接依赖；定义依赖到定理、未分类的依赖、无 Lean 块的节点都拒绝生成快照。不用 `legacyNodeRevision`，因为它
  覆盖 metadata，而协作层会把定义 ID、认证 ID 写进 metadata（见 §7 第二轮）。
- `proofRequestIdentity` 取环境身份和上述节点绑定，依赖按 UUID 排序；`data_version` 仅作审计快照和回写前置条件，不进入两个身份。
- `Project/lean` 只能标记为 `preserved_unmodified`；不得把 Base、context 或 options 写入旧字段。
- BaseRef、context、options、minimum_trust 改变才换环境，形成新的 ProofRequest；用 `replaces_environment_id` 建历史边。
- 根节点或快照中依赖节点的 Lean 文本变化、节点缺失或失去 Lean 块，令原请求过期，保留旧成果；无关节点、metadata、
  说明文字、标题、定理前提自己的依赖变化都不令其过期。已知取舍：定理前提只改证明时仍判为过期。
- context 仅允许 raw、local_options、universes、opens、namespaces、local_notation、local_attrs、variables，值均为字符串数组；namespaces 只能缺省或空数组，禁止 definitions 与所有未知键。
- BaseRef 完整字段与 `3965273` 的既有 GET 基座接口所用 `BaseRef.to_dict` 对齐：base_id、base_key、root_module、lean_version、lean_githash、package_revision、package_manifest_hash、module_list_hash、build_flags_hash、platform、promotion_generation、schema_version。示例为合成数据，不是可访问的真实基座。

### 2.3 声明绑定契约

顶层 `classification` 必须恰为以下三类之一：

1. `definition_only`：仅含 docs/12 允许的 `def`、`abbrev`、`noncomputable def`、`structure`、`inductive`；记录每个
   名字、种类、依赖定义 ID 和登记后定义 ID。
2. `theorem`：明确记录唯一结论的名字与 `theorem/lemma` 种类；依赖定理变成 premise placeholder，依赖定义只以
   `definitions` ID 出现；拆法引入且进入陈述的定义列在 `new_definitions`。草图只能配
   `lean-worker-sketch + sketch`，完整证明只能配 `lean-worker + leaf`。
3. `unsupported`：必须给机器可读 reason 和人可读 details；多结论、混合且无法区分、instance/class、陈述引用未登记
   定义、缺少固定环境实例或其他歧义都不能猜测。

证明内部定义只有在不出现在结论或前提类型时才留在源码；出现在陈述里的定义必须登记。`instance` / `class` 不登记，
只能由固定证明环境提供。定义编辑后产生新内容身份，原绑定只保留为历史。

冻结前补充约束：sketch 必须至少有一个 premise placeholder，leaf 必须为空；只有 new_definitions 非空时 conclusion.goal 才能省略。
每类绑定显式携带 required_context。节点的 open/variable/set_option/namespace 等命令不能留在提交正文；应由未来适配器提取并核对固定 context，绝不能为某节点修改共享 context。
比较采用保守字段子集：required_context 可以省略未使用字段；出现的每个字段必须与固定 context 的整个数组逐字符串、逐顺序完全相等（缺省视作空数组）。不做 trim、重排、去重或语义等价推断；不匹配返回 unsupported / local_context_mismatch。namespace 非空直接违反 context 形状。
两个前提同名、或前提与结论同名，均返回 unsupported / premise_name_collision；不自动改名。绑定中的名字要求为已确定的 Lean 声明名，词法别名消歧留给未来适配器，无法确定时按 ambiguous_binding 拒绝。

### 2.4 双向回写契约

- 批次固定 `batch_id`、`replay_key`、审阅分支及其 data version、来源 ProofRequest 和有序操作。
- 更新已有 Lean block 必须带节点修订号、源码哈希与最终认证 ID；必须保留原公开声明名。
- 拆法定义和子目标用 `create_definition_node` / `create_goal_node` 显式建新节点；定义节点带 definition ID。
- 只有整个批次提交后才能暴露“当前版本已认证”；重复 `replay_key` 不得重复建节点。
- 冲突处理仍待 C 决定；已按评审明确禁止强制覆盖，删除 certified_batch_wins。保留 abort_entire_batch / reviewer_merge_required；fixture 使用推荐候选不代表批准 C。

## 3. 产品决定（2026-09-29 已定）

维护者决定：A–D 全部按推荐方案。下面保留各选项的代价作为决策记录；每节末尾写明决定及其在契约中的编码。

### 3.1 项目内同名定义

| 选项 | 代价 |
|---|---|
| A. 登记前要求作者在审阅分支显式改名；冲突返回 `name_taken` | 最符合 LeanGround 名字只增不改；会增加人工修改，且需同步改引用 |
| B. 协调层自动生成稳定别名并重写源码 | 自动化高，但公开名字和原文档不一致；重写规则、诊断和回写复杂，容易造成不可见语义漂移 |
| C. 每个冲突节点拆到独立 LeanGround project/scope | 少改源码，但跨节点组装、共享和公开极复杂；同一请求仍不能同时使用同名不同 ID |

**已决定 A**：明确拒绝并在审阅分支改名后重试；不静默选择某个定义，也不自动改公开接口。编码：声明绑定
`unsupported` 新增 reason `definition_name_taken`（对应 LeanGround 的 `name_taken` 与 `definition_name_conflict`）。

### 3.2 怎样判定定义节点与定理节点

| 选项 | 代价 |
|---|---|
| A. MathDoc 保守语法扫描；只接受明显单类节点，歧义要求人工选择结论 | 不假设新接口，能尽早报错；需要跟进 Lean 语法，不能代替 elaboration |
| B. 全部由作者在 metadata 中显式标注分类和唯一结论 | 最可审计、实现简单；迁移负担和误标风险高，仍需后端核对源码 |
| C. 要求 LeanGround 新增“解析/分类”接口 | 语义来源最集中；docs/12 没有该接口，形成外部阻塞，当前不能使用 |

**已决定 A + B**：保守语法扫描只处理明显的单类节点；多声明、混合或无法判定的节点必须在 Lean 块 metadata 里
显式标注，仍不清楚就返回 `unsupported`；最终以 LeanGround 的检查结果为准（例如它会拒绝陈述里用到却未登记的定义）。
编码：
- `definition_only` 与 `theorem` 绑定必须带 `classification_source`，取值 `syntax_scan` 或 `explicit_metadata`；
- 显式标注使用 Lean 块 metadata 键 `lean_role`（`definition` / `theorem`）与 `lean_conclusion`（结论声明名），
  与现有的 `certification_id`、`coordination_operation` 同一命名风格；常量见 `contracts.ts` 的
  `LEAN_ROLE_METADATA_KEY`、`LEAN_CONCLUSION_METADATA_KEY`。扫描器与标注界面属于后续阶段，契约只固定字段。

### 3.3 多节点回写冲突谁优先

| 选项 | 代价 |
|---|---|
| A. 当前文档优先：任一点冲突就终止整个批次，在审阅分支重新生成 | 不会覆盖人工编辑，也不会产生部分认证；需要重新审阅和重放 |
| B. 已认证批次优先：强制覆盖冲突节点 | 自动完成率高；可能丢失用户编辑，且认证针对的题面未必还是当前题面 |
| C. 进入 reviewer merge，合并后重新绑定并重新认证 | 最灵活；需要新的合并 UI、状态机和再次认证流程，不能复用原认证直接标记完成 |

**已决定 A**：当前文档优先、整批终止；禁止 B。编码：`conflict_resolution` 固定为 `abort_entire_batch`。
reviewer merge 不是冲突策略：合并后重新绑定、重新认证，作为一个**新的**回写批次提交。

### 3.4 文档权限矩阵

**已决定**：采用 fixture 中的矩阵作为 v1 授权规则：

| 动作 | 允许的角色 |
|---|---|
| read、history、export | admin、owner、editor、viewer |
| write、branch_create、proof_request_create | admin、owner、editor |
| branch_delete、import、member_manage | admin、owner |
| database_create、database_delete | admin |

分支默认继承工作区角色（`inherit_workspace_role: true`）。编码：测试「decision D」逐行断言这张矩阵，改动任何一行
都必须同时改本节与测试。

## 4. LeanGround `3965273` / docs/12 的接口上限

允许依赖的事实操作仅为 `POST /v1/facts/{submit,define,resolve,read,query,plan,assemble,project,widen,events}`；基座读取
为既有 `GET /v1/bases/{key}`。不含 definitions 时可使用既有 `/v1/goals/resolve`；带 definitions 时必须使用有身份的
`/v1/facts/resolve`。定义只通过 ID 传递；`new_definitions` 只用于随 submit 同事务登记。草图使用
`lean-worker-sketch`，完整证明/组装使用 `lean-worker`。

docs/12 没有以下接口，因此本阶段不假设：声明自动分类、项目成员读取、MathDoc 节点批量写回、回写事务回调、
TerminusDB 权限、冲突自动合并。

## 5. `coordinator/` 对照 §9 的逐条自查

以下只是代码自查状态，**交人评审，不在此判定阶段 1 通过**。

| §9 规则 | 状态 | 代码位置与理由 |
|---|---|---|
| 数学、任务、尝试、拆法、项目状态分开 | 已满足 | `domain.ts` `Goal/Fact/Task/Attempt/Run/Board`（54–130）；状态字段分开保存 |
| Goal 按 GoalKey 去重；同 Goal 可有多类 Task | 已满足 | `domain.ts` 171–196 按 key 去重；347–352、453–472 允许 prove/decompose/formalize，禁止同 kind 重复 |
| `lease_epoch` 防迟到覆盖；过期只撤调度权，证明仍可吸收 | 已满足 | `domain.ts` 312–324、474–524；事实吸收走独立 `ingest`（211–244）；`domain.test.ts` 35–101 |
| 预算领取预留、结束结算、掉线保守计费、自报统计 | 已满足 | `domain.ts` 312–318、480–485、487–520；测试 `domain.test.ts` 35–101 |
| 事件处理与游标同事务；后续动作由事务内待办驱动 | 不确定 | `worker.ts` 126–183 在一次 `checkpoint` 中应用事件和 cursor；`store.ts` 99–139 同事务保存命令和 job。尚无覆盖所有通知/查询/组装待办及崩溃点的证明，事件也未单独持久化 sequence 去重表 |
| 协调层只从启用拆法选路，再交固定计划组装 | 已满足 | `domain.ts` 265–310 仅选 active/available；`worker.ts` 186–256 在 plan 前复核 active 并固定 plan ID |
| 同计划重试；换路线新计划；响应丢失按同尝试重放 | 已满足 | `domain.ts` 565–586 继承 plan；`worker.ts` 219–230 以 run ID 为 attempt ID；`store.ts` 203–231 对可重试 job 原 ID 重跑。仍建议评审远端已成功但本地超时场景 |
| 暂停只影响调度/选路，不删除事实；证否未接入时人工暂停 | 已满足 | `domain.ts` 526–530 只改 state；`recompute` 246–263 保留事实；`selectRoute` 280–285 才过滤 active |
| 题面修改保留旧成果，但不能标记新版本认证 | 不满足 | `contracts.ts` 的 environmentIdentity / proofRequestIdentity / proofRequestSnapshot / proofRequestIsStale 已定义按 Lean 内容指纹的过期规则并通过 11 个场景的 fixture；`documents.ts` 176–199 能拒绝修订冲突，但 Board/worker 尚未接入依赖快照和自动 stale 传播，仍可能展示旧 Goal 的 certified。契约测试通过不能视为运行链路满足 |
| 项目完成需可访问、满足信任、无前提根认证；公开另操作 | 不满足 | `worker.ts` 236–246 对最终认证检查 root/无前提且 `ingest` 检查权限/信任；但没有显式项目 completed 状态，也没有独立公开操作 |
| 服务身份是 LeanGround 项目成员；私有成果显式共享后吸收 | 不确定 | `worker.ts` 70–74 把服务身份和成员推给 project；`domain.ts` 211–229 拒绝 private/错误 project。当前没有显式“个人私有成果共享”工作流，docs/12 也没有项目成员读取接口可反查 |

## 6. 评审重点与明确未完成项

1. A–D 已决定，契约已冻结为 `v1`（§3）。
2. 文档权限 schema 已有矩阵，但当前 `http.ts` 仍把文档入口主要绑在协作 Project 成员和 owner 上；这不是权限实现完成。
3. 当前 `worker.ts` 仍调用 `/v1/goals/resolve`，submit 也未传 `definitions/new_definitions`；按迁移计划属于阶段 4，
   本阶段不提前修改。
4. 当前回写只支持单节点 `Documents.accept`；批量回写 schema 是契约，不是实现。
5. 当前声明绑定 fixture 是三类边界的合同样例，不是 Lean 源码解析器；不得把它宣传为已支持真实文档转换。

## 7. 2026-09-29 评审意见处理记录

| 评审项 | 处理与验证 |
|---|---|
| 一.1 环境 / 请求身份 | §2.2 修正；staleness_cases 分别修改无关、根、依赖定义节点；TS 从真实节点内容重新算修订，验证身份与过期结果；plan 措辞另提交 |
| 一.2 BaseRef / context | 严格对象、完整 BaseRef、仅八个 context 字段；两套 validator 均拒绝未知键、字符串代替数组、非空 namespaces、definitions、缺失任一 BaseRef 字段 |
| 一.3 声明绑定 | sketch/leaf/goal 条件由 JSON Schema 与 Zod 同例测试；固定 context 比较及同名前提由语义测试验证，见 §2.3 |
| 二 修订号对齐 | legacy_revision 共四节点，包括 Unicode、LaTeX 反斜杠与引号、控制字符及多块；Rust 与 TS 校验同一固定 SHA-256 |
| 二 回写冲突 | 删除已被明确否决的 certified_batch_wins；C 未填写，其他候选暂保留 |
| 三 自查表 | 仅重新评估题面修改项，仍为不满足：运行态尚未接线；其余判断维持 |

JSON Schema 由 `coordinator/scripts/contract-schemas.ts` 从 Zod 生成并补足条件约束，测试比较提交文件与生成结果。
跨数组名称唯一性和跨文档 context 相等属于语义规则，标准 JSON Schema 无法表达此类投影/外部比较；schema 中显式注记，调用者必须继续执行 `bindingRejection`，不能只验证 JSON 形状。
本阶段新增的身份、过期与声明检查均为合同参考函数，未接入 worker/HTTP 状态机。

### 第二轮（2026-09-29，评审方直接修改）

上一轮评审说明规定用整节点 `legacyNodeRevision` 判断过期，这是评审说明的错误，不是实现错误：它覆盖 metadata、
标题和全部块，而 §5 要求把定义 ID、认证 ID 写进 metadata。实测只改 metadata、只改 LaTeX 块、只改标题，
`legacyNodeRevision` 都会变，依赖请求因此会被自己的回写打成过期。修改：

| 项 | 处理 |
|---|---|
| 快照内容 | `revision` 改为 `lean_content_sha256`（Lean 块文本的 SHA-256）；依赖增加 `relation`（definition / premise）；根与依赖不得重复 |
| 快照范围 | 新增参考函数 `proofRequestSnapshot`：定义依赖取传递闭包，定理前提只取直接依赖 |
| 过期判断 | `proofRequestIsStale` 比较 Lean 内容指纹；节点缺失或失去 Lean 块视为过期 |
| fixture | `staleness_cases` 改为专用图（两级定义、带自身依赖的定理前提、根、无关节点）与 11 个场景：无关节点、写 metadata、改说明文字、改标题、改定理前提自己的依赖 5 个不过期；改根、改直接定义、改传递定义、改前提、前提缺失、定义失去 Lean 块 6 个过期。快照由 Python 独立计算，与 TS 参考函数比对 |
| JSON Schema | 由生成脚本重新生成 `proof-environment.schema.json`，其余三份不变；无法用 JSON Schema 表达的去重规则写入 `$comment` |

### 第三轮（2026-09-29）：产品决定与冻结 v1

维护者决定 A–D 全部按推荐方案。编码与验证：

| 决定 | 契约中的编码 | 测试 |
|---|---|---|
| A 同名定义 | reason `definition_name_taken` | Zod 与 JSON Schema 都接受该 reason |
| B 节点分类 | 必填 `classification_source`；metadata 键 `lean_role`、`lean_conclusion` | 缺字段、非法取值两边都拒绝；`explicit_metadata` 两边都接受 |
| C 回写冲突 | `conflict_resolution` 固定为 `abort_entire_batch` | `certified_batch_wins`、`reviewer_merge_required` 两边都拒绝 |
| D 权限矩阵 | fixture 矩阵即 v1 规则（§3.4） | 逐行断言矩阵与分支继承 |

四份 schema 的 `schema_version` 由 `…v1-draft` 改为 `…v1`，由生成脚本重新生成；diff 只含版本号与上表三处编码。

验证记录（2026-09-29）：`npm run check` 通过，0 errors / 0 warnings；设置 `MDC_TEST_DATABASE_URL` 的 `npm test`：15 通过、0 跳过，三个 PostgreSQL 集成测试实际执行；`cargo test --test test_contract_fixture`：1 通过，循环核对全部四个节点。沙箱内 Node IPC 被拒后在获准环境重跑通过。

验证记录（第三轮，2026-09-29，本机 macOS，临时 PostgreSQL）：`npm run check` 0 errors；设置 `MDC_TEST_DATABASE_URL`
的 `npm test` 16 通过、0 跳过（三个 PostgreSQL 集成测试实际执行）；`cargo test --test test_contract_fixture` 1 通过；
Prettier 与 `git diff --check` 通过。
