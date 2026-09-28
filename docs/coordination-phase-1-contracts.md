# 协调层阶段 1：v1-draft 契约与实现自查

状态：**交人评审，未判定通过**。本文件只完成阶段 1 的契约草案、固定 fixture、确定性合同测试和源码自查；
三个产品决定仍待评审。它不授权开始阶段 2。

依据：MathDoc `docs/coordination-migration-plan.md` §6.4、§7.3、§9；LeanGround 只以 `main` 的
`3965273` 中 `docs/12-LeanGround-事实层接口与验收.md` 为接口依据。本文不假设 docs/12 以外存在声明解析、
成员读取、事务回调或批量回写接口。

## 1. 交付物与版本边界

四份机器可读契约均为 JSON Schema 2020-12，版本刻意写成 `v1-draft`，避免把待评审决定伪装为已冻结：

| 契约 | Schema | 固定 fixture / 测试 |
|---|---|---|
| 文档权限 | `coordinator/contracts/v1/document-permission.schema.json` | `coordinator/test/contracts/v1/contracts.fixture.json`；owner/editor/viewer/admin 的动作矩阵和 viewer 写入负例 |
| 证明环境 | `coordinator/contracts/v1/proof-environment.schema.json` | 同一文档分支从 `env-v1` 改到 `env-v2` 时产生不同合同摘要，旧环境由 `replaces_environment_id` 引用而非覆盖 |
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

- 独立保存完整 BaseRef、context、options、最低信任要求和 TerminusDB 的 `database/branch/data_version`。
- `Project/lean` 只能标记为 `preserved_unmodified`；不得把 Base、context 或 options 写入旧字段。
- 任何环境字段或文档 data version 改变都形成新的 environment/ProofRequest；用 `replaces_environment_id` 建历史边，
  不覆盖旧 Goal 或旧认证状态。
- 环境身份用合同的稳定摘要参与幂等与审计；fixture 验证两个环境摘要不同。

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

### 2.4 双向回写契约

- 批次固定 `batch_id`、`replay_key`、审阅分支及其 data version、来源 ProofRequest 和有序操作。
- 更新已有 Lean block 必须带节点修订号、源码哈希与最终认证 ID；必须保留原公开声明名。
- 拆法定义和子目标用 `create_definition_node` / `create_goal_node` 显式建新节点；定义节点带 definition ID。
- 只有整个批次提交后才能暴露“当前版本已认证”；重复 `replay_key` 不得重复建节点。
- 冲突处理仍是待决策字段，schema 接受三种候选值；fixture 使用推荐候选仅用于可执行测试，不代表已批准。

## 3. 待人决定：选项、代价与推荐

### 3.1 项目内同名定义

| 选项 | 代价 |
|---|---|
| A. 登记前要求作者在审阅分支显式改名；冲突返回 `name_taken` | 最符合 LeanGround 名字只增不改；会增加人工修改，且需同步改引用 |
| B. 协调层自动生成稳定别名并重写源码 | 自动化高，但公开名字和原文档不一致；重写规则、诊断和回写复杂，容易造成不可见语义漂移 |
| C. 每个冲突节点拆到独立 LeanGround project/scope | 少改源码，但跨节点组装、共享和公开极复杂；同一请求仍不能同时使用同名不同 ID |

**推荐 A，待评审**：明确拒绝并在审阅分支改名后重试；不静默选择某个定义，也不自动改公开接口。

### 3.2 怎样判定定义节点与定理节点

| 选项 | 代价 |
|---|---|
| A. MathDoc 保守语法扫描；只接受明显单类节点，歧义要求人工选择结论 | 不假设新接口，能尽早报错；需要跟进 Lean 语法，不能代替 elaboration |
| B. 全部由作者在 metadata 中显式标注分类和唯一结论 | 最可审计、实现简单；迁移负担和误标风险高，仍需后端核对源码 |
| C. 要求 LeanGround 新增“解析/分类”接口 | 语义来源最集中；docs/12 没有该接口，形成外部阻塞，当前不能使用 |

**推荐 A + B，待评审**：保守扫描只生成候选，明显的单类节点可确认；多声明或混合节点必须有显式 metadata 并再次
核对，仍不清楚就返回 `unsupported_source`。阶段 1 不实现此推荐，以免先行拍板。

### 3.3 多节点回写冲突谁优先

| 选项 | 代价 |
|---|---|
| A. 当前文档优先：任一点冲突就终止整个批次，在审阅分支重新生成 | 不会覆盖人工编辑，也不会产生部分认证；需要重新审阅和重放 |
| B. 已认证批次优先：强制覆盖冲突节点 | 自动完成率高；可能丢失用户编辑，且认证针对的题面未必还是当前题面 |
| C. 进入 reviewer merge，合并后重新绑定并重新认证 | 最灵活；需要新的合并 UI、状态机和再次认证流程，不能复用原认证直接标记完成 |

**推荐 A，待评审**：当前文档优先、整批终止；如需合并则显式进入 C，并在合并后重新认证。禁止 B。

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
| 题面修改保留旧成果，但不能标记新版本认证 | 不满足 | `documents.ts` 176–199 能以固定 revision 拒绝覆盖，但 Board 不会在外部题面变化后自动标 stale，仍可能展示旧 Goal 的 certified |
| 项目完成需可访问、满足信任、无前提根认证；公开另操作 | 不满足 | `worker.ts` 236–246 对最终认证检查 root/无前提且 `ingest` 检查权限/信任；但没有显式项目 completed 状态，也没有独立公开操作 |
| 服务身份是 LeanGround 项目成员；私有成果显式共享后吸收 | 不确定 | `worker.ts` 70–74 把服务身份和成员推给 project；`domain.ts` 211–229 拒绝 private/错误 project。当前没有显式“个人私有成果共享”工作流，docs/12 也没有项目成员读取接口可反查 |

## 6. 评审重点与明确未完成项

1. 批准或修改 §3 的三个决定，然后才能把 `v1-draft` 提升为冻结 `v1`。
2. 文档权限 schema 已有矩阵，但当前 `http.ts` 仍把文档入口主要绑在协作 Project 成员和 owner 上；这不是权限实现完成。
3. 当前 `worker.ts` 仍调用 `/v1/goals/resolve`，submit 也未传 `definitions/new_definitions`；按迁移计划属于阶段 4，
   本阶段不提前修改。
4. 当前回写只支持单节点 `Documents.accept`；批量回写 schema 是契约，不是实现。
5. 当前声明绑定 fixture 是三类边界的合同样例，不是 Lean 源码解析器；不得把它宣传为已支持真实文档转换。
