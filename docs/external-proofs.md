# 外部证明的导入与项目核验（方案）

状态：2026-10-14 方案；同日按 Codex 审阅修订一次（§8），阶段 0 完成（§5.1），10 份配置的第一版实现完成（§5.2）。涉及 MathDoc（分支 `codex/mathdoc-coordination`）与
LeanGround（分支 `imo-bench-regression`，`3b8a793`）。来源标注沿用 `docs/mathdoc.md` §1：`[实测]` 附条件，`[推断]`，`[未验证]`。

## 1. 目的

回答三个问题：

1. 把 OpenAI `math` 仓库这类**外部 Lean 证明**导入 MathDoc 并由 LeanGround 判定，能不能不改上游源码；
2. 现在的 LeanGround 认证接口为什么做不到；
3. 第一版要做哪些事、不做哪些事。

结论：

- 不改源码可行。做法是在 LeanGround 里加一个**项目核验**作业接口，与现有的**协作认证**并列。两者的区别在输入与产出：
  项目核验检查一个完整项目里配置指定的目标；协作认证为可组合的证明生成认证。判定标准都不降低。
- 项目核验直接采用 Comparator，「通过」按 Comparator 实际保证的范围定义（§3.2）。
- 在 MathDoc 里，一份 Comparator 配置对应一个**导入项目**，下面保存多个核验目标及结果，不建协作看板。
- 第一版只做：导入资料、阅读陈述、提交核验、展示结果。语法放宽、模块提升、多运行时服务化、MathDoc 数据库级模式切换都不在主线（§4）。

## 2. 背景

### 2.1 名词

| 名词 | 含义 |
|---|---|
| 协作认证 | LeanGround 现有事实层（`/v1/facts/*`）的产出：某个目标身份（GoalKey）在若干前提下成立；可按 GoalKey 组合，最后组装成一份源码整份重检 |
| 项目核验 | 本方案新增：对一个钉住版本的 Lean 项目，按一份 Comparator 配置检查其中指定的目标 |
| Comparator | Lean 官方（leanprover）的核对工具：给定可信的陈述模块（Challenge）与待核对的证明模块（Solution），检查证明是否证出陈述 |
| 定义洞 | 陈述模块里留给证明方补全的定义，列在配置的 `definition_names` 中（§3.2） |
| 运行时 | 一套 Lean 工具链 + 一个 Lake 工作区（编译好的 Mathlib 与依赖） |
| 基座（base） | 运行时里一个根模块及其导入闭包，在 LeanGround 事实库里有一个 `base_key` |
| 内核重放 | 把常量放进干净的环境重新过一遍 Lean 内核 |
| `.olean` | Lean 的编译产物；导入时 Lean 不会重新检查其中的证明 |

### 2.2 现在的认证接口为什么做不到

- 组装是**源码拼接**（LeanGround `docs/13` §4.1），所以每份源码的写法受限。正文策略（`docs/13` 阶段 1，已实现未部署）只允许声明、
  `open … in`、白名单内的 `set_option … in`。[实测，`docs/13` §5.1，Lean 4.33.1 + Mathlib] LEAP 42 份证明原样提交，17 份通过；
  20 份因顶层 `open` / `variable` / `set_option` / `section` 被拒。
- 内核重放只覆盖**本次新增**的常量，导入的模块必须已在基座里（`docs/13` §2.1）。外部项目自己的模块不在基座里；放进基座又等于直接信任它们。

### 2.3 OpenAI 语料

本机副本：`.openai-math-corpus/upstream/`，`openai/math` 的 `1581d4cbd`，Lean 4.34.1，Mathlib `d13f23b`。
[实测：2026-10-13/14，读源码树与 `ComparatorChallenges/*.json`，导入关系按 `import` 行解析]

| 项 | 数字 | 所以 |
|---|---|---|
| OAI 证明源码 | 122,458 个文件，2,606 万行；同版本 Mathlib 8,529 个文件、233 万行 | 是 Mathlib 的 11 倍，不能整体编进基座，也不能整体转成 MathDoc 节点 |
| Comparator 配置 | 416 份 | 直接作为导入清单 |
| 每份的定理数 | 1 条：364 份；2–17 条：51 份；0 条（只有定义洞）：1 份 | 导入项目要能挂多个核验目标（§3.3） |
| 允许的公理 | 416 份都是 `propext`、`Quot.sound`、`Classical.choice` | 判定标准统一 |
| 带 `definition_names` 的配置 | 10 份，共 65 个定义洞；其中 63 个在陈述模块里写了完整定义体，只有 2 个含 `sorry` | 见 §3.2 末段：这些定义的含义由证明方决定，需要人审 |
| 每份证明的 OAI 导入闭包 | 文件数中位数 77、第 90 百分位 719、最大 4,490；行数中位数 2.25 万、第 90 百分位 12.4 万、最大 283 万 | 按份编译可行，最大几份很重 |
| 各份闭包合计 | 去重 105,829 个文件；不去重 126,771 个、2,652 万行；闭包 ≤ 100 个文件的有 230 份 | 总量接近全库 |
| 外部依赖 | Mathlib 416 份；PrimeNumberTheoremAnd 14 份；其余第三方包各 1–6 份 | 编译用上游自己的 Lake 工作区 |
| 上游自述 | 覆盖面 `Partial progress.`，审阅状态 `unchecked` | 核验只说明证明对得上陈述，不说明陈述忠实于论文 |

`definition_names` 一行的统计是按声明块里是否出现 `sorry` 判断的正则统计，没有用 Lean 解析器核对。

### 2.4 Seymour 试点（Codex 完成的部分）

[实测：`.openai-math-corpus/projects/seymour-second-neighborhood/`]

- `Main.lean` 导入闭包 43 个模块、4,861 行，用 Lean 4.34.1 逐个编译：全部通过，串行共 50.9 秒，产物 6.6 MB。
- 用 LeanGround 4.34.1 的 worker 检查原样 `Main.lean`：`ok`，`trusted`，公理为上面三条，49 毫秒。只重放了 `Main.lean` 自己的常量，
  另外 42 个模块的 `.olean` 直接信任；没有对照 Comparator 陈述；没有留下记录。
- 2026-10-13 时本机没有 `comparator`、`lean4export`、`landrun` [实测：`which`]；前两个已在阶段 0 编译到 `.openai-math-corpus/tools/`（§5.1），
  `landrun` 仍没有（macOS 不支持）。

## 3. 方案

### 3.1 两类判定并列

| | 协作认证（现有） | 项目核验（新增） |
|---|---|---|
| 输入 | 一条声明或一份拆解的源码 | 一个钉住版本的 Lean 项目（工作区路径或仓库 + 提交号）与一份 Comparator 配置 |
| 产出 | 认证：GoalKey 在前提下成立，可组合 | 核验记录：配置里每个目标的结果，不参与组合 |
| 写法 | 受限（§2.2） | Lean 接受的都行 |
| 判定 | LeanGround 事实层 | Comparator（§3.2） |
| 谁能用 | `work` 权限的成员与 Agent | owner 或维护者：要执行项目代码，单次可能数小时 |
| 接口 | 现有 `/v1/facts/*` | 一个新的异步作业接口：提交、查询状态、取结果 |

LeanGround 不重构成两套模式系统，只新增这一个作业接口。第一版作业使用指定版本的 Lean 工作区（OpenAI 语料用
`LeanGround/base/oai-4.34.1/lean`，复用其中已编译的依赖），由工作区的 `lean-toolchain` 决定 Lean 版本，不依赖事实服务的 worker 运行时。

### 3.2 「通过」的定义：按 Comparator 实际保证的范围

Comparator 的做法 [实测：读 `leanprover/comparator` `v4.34.0` 的 `Main.lean`、`Comparator/Compare.lean`、`Comparator/Axioms.lean`]：

1. 在沙箱里用 `lake build` 编译陈述模块与证明模块；
2. 用 `lean4export` 分别导出「目标 + 允许的公理 + 内核内建常量」及其全部依赖；
3. 比较：定理目标的签名（名字、宇宙参数、类型）必须相同；从定理类型出发，沿用到的常量逐个比较，**非定义洞的常量必须完全相同（含定义体）**，
   定义洞只比较名字、宇宙参数、类型与安全级别，然后只沿它的类型继续；
4. 公理：从定理与定义洞出发，沿类型与值用到的常量一路查，遇到不在 `permitted_axioms` 里的公理即拒绝；
5. 把导出的证明方环境（即目标的依赖闭包）重放进 Lean 内核；配置了外部内核（如 nanoda，独立实现的内核）时另跑一遍。

第一版明确承诺，一份配置**通过**表示：

1. `theorem_names` 中的每条定理，在证明模块里证出的陈述与陈述模块相同；
2. `definition_names` 中的每个定义洞，名字、类型、宇宙参数、安全级别与陈述模块一致，能通过类型检查，且不依赖白名单外的公理；
3. 这些目标所需的依赖经过内核重放；
4. 用到的公理不超出配置的 `permitted_axioms`。

不承诺：项目中与这些目标无关的常量都能通过；陈述忠实于论文。第一版不自己实现 Comparator。

**定义洞要另外核对。** README 写明定义洞应在陈述模块里以 `sorry` 给出、由证明方补全；Comparator 不比较定义体。
§2.3 的统计显示上游 65 个定义洞里有 63 个写了完整定义体 [实测，正则]。阶段 0 的 T6 证实了后果 [实测，n = 1]：
`EuclideanFiveColor` 的证明方把 `ProperColoring` 定义为 `False`，主定理随之平凡成立，Comparator 仍判通过。
所以对这 10 份配置，Comparator 通过不等于证出了陈述模块里写的定理。

核对办法不需要新代码（阶段 0 的 T8）：对陈述模块里**写了定义体**的定义洞，从配置中去掉它们再跑一遍 Comparator。
此时它们被当作陈述的依赖逐项比较，通过即说明证明方的定义与陈述模块相同。结果分三类：

| 定义洞 | 核对 | 显示 |
|---|---|---|
| 陈述模块写了定义体，第二遍通过 | 机器确认一致 | 通过 |
| 陈述模块写了定义体，第二遍不通过 | 证明方的定义与陈述不同 | 通过（定义与陈述不同，待审），展示两边原文 |
| 陈述模块是 `sorry`（真正的构造题） | 无法机器比对 | 通过（定义由证明方给出，待审），展示证明方原文 |

**沙箱。** Comparator 用 `landrun` 隔离编译与导出（依赖 Linux 内核的 Landlock）：编译时只允许读项目目录、写项目的 `.lake`，
导出时不可写。README 提供 `scripts/fake-landrun.sh` 供非 Linux 环境开发使用，那时没有隔离。隔离要防的是：编译期执行的项目代码
改写陈述模块的编译产物或导出结果、替换工具、读取本机令牌。已定（2026-10-14）：阶段 0 在 macOS 上用 `fake-landrun`，结果标为「未隔离」，
不算正式通过；正式核验在本机 colima 的 Linux 容器里用真 `landrun`。那时依赖要放在 `landrun` 允许读取的路径里：
阶段 0 用的「`.lake/packages` 符号链接到项目外」在真 `landrun` 下读不到 [推断：按 `Main.lean` 中 `readablePaths := #[projectDir]`]。

### 3.3 MathDoc：导入项目

- **一份 Comparator 配置对应一个导入项目**（一个 MathDoc 数据库），下面保存多个核验目标：每条 `theorem_names` 与每个
  `definition_names` 各一个目标。单定理、多定理、只有定义洞的配置都自然支持，不需要放宽协作系统「一个数据库一条根定理」，也不需要拆库。
- **不建协作看板。** 项目核验不按 GoalKey 组合，也不参与路线选择。核验作业在 LeanGround 执行，核验记录存在 MathDoc（均已定，
  2026-10-14）。LeanGround 保存作业状态直到 MathDoc 取走结果，沿用它现有异步作业的做法：同一幂等键重复提交拿回同一作业，执行者崩溃后
  租约过期由他人接手，迟到的结果丢弃（`src/leanground/jobs.py`）。MathDoc 的记录包括：配置、上游提交号、工作区与工具链、
  `lake-manifest.json` 哈希、陈述模块哈希、是否隔离、每个目标的结果（含 §3.2 定义洞的三类）、用到的公理、耗时、失败原因，
  以及 LeanGround 的作业 ID 与结果哈希，以便追溯。
- **节点**（不改上游源码；2026-10-14 改为把证明的导入闭包逐文件建成节点，见 §8）：
  - 论文：PDF 链接与 SHA-256；
  - 范围说明：`lean/docs/NNN.md`；
  - 陈述：说明文字 + Lean 块（陈述模块原文）；
  - 证明概要：证明模块名、导入闭包的文件数与行数、外部依赖、上游固定提交的源码链接；依赖证明主文件的节点；
  - 文件：导入闭包里每个上游文件一个节点，Lean 块放原文，依赖边就是它在仓库内的 `import`；说明文字给出路径、行数与仓库外的导入。
  - 陈述与文件的 Lean 块带来源元数据（模块、路径、提交、原文 SHA-256）。原文未改时节点状态显示为「导入」（`imported`），
    不显示「未提交 LeanGround」，不当作定义或定理，也不出现提交按钮：它由项目核验整体覆盖，不是协作节点。改过原文就按普通 Lean 块处理。
- **继续协作**：用户确实要在某个结果上接着做时，再选择目标、设置证明环境并创建看板。
- **实现要点** [实测：读代码]：MathDoc 的作业表 `mdc_job.project` 外键指向 `mdc_project(id)`，即作业必须属于一块看板，所以核验作业
  不能直接用现有队列，要有自己的记录与队列；LeanGround 的 `fact_record.kind` 有 CHECK 约束，新增记录类型要迁移，或单独建表。
- 现在 `oai-seymour-second-neighborhood` 里 Codex 建的 43 个文件节点与 2026-10-13 建的协作看板，按本节应换成导入项目的结构（§7）。

### 3.4 MathDoc 的三种模式（不是前置工程）

预览 / 验证 / 协作三种模式保留为方向，但不阻塞项目核验。第一版需要的「导入资料、阅读陈述、提交核验、展示结果」都在导入项目内完成，
不依赖数据库级模式。

如果现在就实现模式，只做：新建项目时选择模式，以及「预览 → 验证 → 协作」的单向升级。不做任意来回切换、已有协作项目的迁移、
切换后任务与审阅状态的保留。

| | 预览 | 验证 | 协作（现状） |
|---|---|---|---|
| 用途 | 阅读导入的语料 | 写证明或导入证明，让机器判对错 | 多人、多 Agent 分工证一条定理 |
| LeanGround | 不调用 | 项目核验；自己写的节点走协作认证（看板只当认证账本，不生成任务） | 协作认证全部功能 |

## 4. 不在主线的扩展项

这三项都值得做，但不会让第一份外部证明更快核验成功，单独排期：

| 扩展 | 内容 | 性质 |
|---|---|---|
| 协作认证放开只作用于本文件的写法 | `namespace` / `section`、`variable` / `omit` / `include`、顶层 `open`、`private`。依据：阶段 1 的 Lean 解析与「源码声明 → 常量」绑定表。全局属性、`instance`、记号与宏、元程序仍不放开：源码拼接下它们会改变别人证明的行为或目标身份 | 兼容性改进，可并入 `docs/13` §6 待定 1 |
| 模块提升 | 核验通过的模块提升为派生基座，让协作项目经连接定理（`docs/13` 阶段 2a）引用导入的定理。代价：每个派生基座要单独的 worker（约 0.8 GB 内存 [实测，`docs/13` §4.4，n = 1]），现在每个基座还要重采一遍 Mathlib（4.34.1 完整 Mathlib：1.1 GB 采集产物、约 2,900 万行依赖边 [实测]） | 复用外部成果的后续功能 |
| 多运行时服务化 | 一个 LeanGround 服务同时持有多套工具链（如 4.33.1 与 4.34.1）。现在一个进程只有一个运行时 [实测：读 `write/worker_pool.py`]，一套 MathDoc 也只能连一个事实服务（2026-10-13 serve 与 worker 连到不同服务，挂载报 `environment_mismatch`） | 调度能力 |

写法分布供扩展项一参考 [实测：正则统计全部 122,458 个 OAI 文件，近似]：`namespace` / `section` 99.7%，顶层 `open` 87.4%，
`variable` / `omit` / `include` 46.6%，`private` 5.5%，全局属性 11.4%，`attribute` 命令 9.9%，`instance` 4.8%，
`notation` / `macro` / `syntax` / `elab` 4.5%，`run_tac` / `run_cmd` 12 个文件，`unsafe` / `implemented_by` / `@[extern]` 6 个文件，`axiom` 声明 3 个文件。

## 5. 分阶段计划与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| 0 | Seymour 手动试点：在 Linux 环境装 `comparator`、`lean4export`（按 4.34.1 编译）、`landrun`，对 `SeymourSecondNeighborhood.json` 运行 | 原样通过；三个反例被拒：改动陈述模块中的定理陈述、在证明里注入 `sorry`、引入白名单外的公理。另取一份带完整定义体定义洞的配置（如 `EuclideanFiveColor`），在证明方改动该定义体，记录 Comparator 是否拒绝 |
| 1 | 10 份小配置（闭包 ≤ 100 个文件） | 实测编译与核验耗时、产物大小、失败率与原因，替换 §6 的外推 |
| 2 | LeanGround 项目核验作业接口：提交、状态、结果；调用 Comparator；记录（§3.3 列出的字段） | 阶段 0 的正反例全部复现；响应丢失后重发只执行一次；进程崩溃后作业可由他人接手 |
| 3 | MathDoc 导入项目：读配置建库与节点、提交核验、展示结果、标出定义洞 | 416 份配置全部建库；多定理与只有定义洞的配置正确展示；核验失败带原因 |
| 4 | 批量核验 416 份 | 按阶段 1 的数据选定机器与并行度；全部有结论（通过，或带原因的失败） |

### 5.1 阶段 0 的结果（2026-10-14）

方法：macOS 上用 `fake-landrun`（**未隔离**），Comparator 与 `lean4export` 取 `v4.34.0` 标签、用 Lean 4.34.1 编译；在上游副本
`.openai-math-corpus/upstream/lean`（未改动）里运行，依赖用符号链接复用 `LeanGround/base/oai-4.34.1` 已编译的产物，共享基座目录不写入；
反例模块临时放进上游副本的 `Phase0/` 子目录，跑完连同编译产物删除。复现脚本与逐条日志在 `.openai-math-corpus/phase0/`。

| 测试 | 改动 | 结论 [实测，各 n = 1] | 用时 |
|---|---|---|---|
| Seymour | 原样 | 通过 | 83 秒（含编译 43 个 OAI 模块） |
| T2 | 陈述模块中 `GoodVertex` 的 `≤` 改为 `<`，定理行不变 | 拒绝：`GoodVertex` 两边不一致 | 28 秒 |
| T3 | 陈述模块中定理改为 `∀ v, GoodVertex r v` | 拒绝：定理陈述不一致 | 18 秒 |
| T4 | 证明方主定理为 `sorry` | 拒绝：非法公理 `sorryAx` | 12 秒 |
| T5 | 证明方引入 `axiom phase0_bad : False` | 拒绝：非法公理 `phase0_bad` | 11 秒 |
| T6 | EuclideanFiveColor：证明方把定义洞 `ProperColoring` 定义为 `False` | **通过** | 240 秒 |
| T7 | EuclideanFiveColor 原样 | 通过 | 675 秒（含编译 70 个 OAI 模块） |
| T8 | EuclideanFiveColor 原样，配置去掉 `definition_names` | 通过：真证明的 `ProperColoring` 与陈述模块一致 | 207 秒 |

所以：

- 阶段 0 的验收项都已满足：原样通过，三个反例（改陈述、注入 `sorry`、白名单外公理）被拒；T2 另外说明陈述所用定义的定义体也受比较。
- 定义洞的弱点由 T6 证实，核对办法由 T8 给出（§3.2）。EuclideanFiveColor 的真证明经 T8 确认证出的是陈述模块写的定理。
- Lake 对 23 个第三方包报「有本地改动」，上游 `lean/patches/` 有 24 项补丁 [推断：改动来自补丁，没有逐个比对]。核验记录要记下补丁状态。

### 5.2 第一版实现（2026-10-14）

范围（维护者 2026-10-14 决定）：不跑全部 416 份，选 10 份上 MathDoc；删除并重建 Seymour；做源码浏览。

| 编号 | 配置 | 问题 | 定理 / 定义洞 | 导入闭包 |
|---|---|---|---|---|
| 1 | `SeymourSecondNeighborhood` | Seymour 第二邻域猜想 | 1 / 0 | 43 个文件，4,861 行 |
| 2 | `HadwigerCounterexample` | Hadwiger 猜想的任意大反例 | 2 / 0 | 288，39,776 |
| 3 | `SidorenkoCounterexample` | Sidorenko 猜想的反例 | 1 / 0 | 35，23,761 |
| 4 | `EuclideanFiveColor` | 平面不能 5 染色 | 1 / 1 | 70，30,336 |
| 5 | `KaplanskyDirectFiniteness` | Kaplansky 直接有限性猜想的反例（特征 2） | 1 / 0 | 121，13,010 |
| 6 | `DirectCrouzeix` | 完整 Crouzeix 不等式及最优性 | 2 / 0 | 42，7,389 |
| 7 | `MahlerConjecture` | 对称 Mahler 猜想 | 1 / 0 | 229，21,379 |
| 8 | `HotSpots` | 单连通平面区域的 hot spots 猜想 | 1 / 0 | 31，29,338 |
| 9 | `Naimark` | Naimark 问题的 ZFC 反例 | 1 / 7（都写了定义体） | 16，25,931 |
| 10 | `Nagata` | Nagata 猜想 | 3 / 0 | 117，36,997 |

数据库名为 `oaic-` 加配置名的连字符写法，如 `oaic-seymour-second-neighborhood`。导入闭包的文件数与行数，由 MathDoc 的实现（git 读取）与
另写的 Python 统计各算一遍，10 份一致 [实测]。

**LeanGround**（分支 `project-checks`，未提交）：

- `src/leanground/project_checks.py`：请求校验（工作区白名单、模块名、拒绝 `enable_nanoda`）、工作区身份、运行 Comparator 并把输出归成
  `statement_mismatch`、`declaration_mismatch`、`illegal_axiom`、`kernel_rejected`、`solution_build_failed` 等原因、定义洞第二遍、
  按需删除 `.lake/build/ir`（默认不删，见 §6）；作业表 `project_check`（`sql/12-project-checks.sql`），与 `verification_job` 同一套租约规则，
  执行期间每 60 秒续租（初始租约 15 分钟）。
- `Tools/DefinitionHoles.lean`：导入陈述模块，按 `Expr.hasSorry` 判断每个定义洞是 `sorry` 还是写了定义体；用被核验工作区自己的
  Lean 解释执行。
- 接口：`POST /v1/project-checks`、`GET /v1/project-checks/{id}`，要事实令牌，只读得到自己提交的；子进程只拿到 `PATH`、`HOME`
  等几个环境变量。
- 测试：`tests/test_project_checks.py` 29 项（替身 `lake` 与 `comparator`）+ 6 项需要数据库（状态机、续租），全过；默认测试集 165 项全过。

**MathDoc**：

- `coordinator/src/imports.ts`：按固定提交经 git 读取上游（`git cat-file --batch`），越出仓库的路径在交给 git 之前拒绝；生成导入记录、
  5 类摘要节点（概览、论文、范围说明、陈述、证明概要）与导入闭包的文件节点（§3.3）；核验记录表 `mdc_import`、`mdc_import_check`；提交时先落本地记录，回复丢失时用
  同一幂等键重发；读取时向 LeanGround 拉取进行中的核验。
- 接口：`GET/POST /api/imports`、`GET /api/imports/<库>`、`POST /api/imports/<库>/checks`、`GET /api/imports/<库>/source`、
  `GET /api/imports/<库>/source/file?module=`、`POST /api/imports/<库>/rebuild`（按导入记录的提交重建节点：节点 ID 不变、补上新节点、
  核验记录保留）；命令 `mdc external list|new|show|check|rebuild`。
- 删库：`remove` 之后退役该库的看板（标为已取代、记下删除、库名改为墓碑名，不会被换回环境时复活，也让出唯一索引）、删除证明环境与导入记录。
- 前端：`/p/<库>/<分支>/import` 导入页（核验结果、定义洞分类、未隔离提示、陈述），「源码」标签按目录列出陈述模块与导入闭包，
  只读 Monaco 显示文件，可沿「导入 / 被导入」跳转；编辑器顶栏在有导入记录时显示 Import 按钮。
- 测试：`coordinator/test/imports.test.ts` 3 项（含真实 PostgreSQL 与 TerminusDB、LeanGround 替身：权限、源码浏览越界、回复丢失重发、
  删库）全过；coordinator 全部 77 项中 68 过、0 失败、9 项需真实 LeanGround 跳过；前端 `svelte-check` 0 错误。

**结果**（LeanGround 项目核验，macOS 未隔离，各 n = 1；2026-10-14 全部完成）[实测]：

| 配置 | 结论 | 用时 | 备注 |
|---|---|---|---|
| `SeymourSecondNeighborhood` | 通过 | 62 秒 | 记录的证明与陈述源码哈希与 Codex 试点记下的一致 |
| `DirectCrouzeix` | 通过 | 974 秒 | 含重编 |
| `KaplanskyDirectFiniteness` | 通过 | 2,812 秒 | 大部分在释放内存之前 |
| `EuclideanFiveColor` | 通过 | 540 + 144 秒 | 定义洞 `ProperColoring`：陈述写了定义体，第二遍比较一致，不需人审 |
| `Naimark` | 通过 | 1,357 + 176 秒 | 7 个定义洞陈述都写了定义体，第二遍比较全部一致，不需人审 |
| `SidorenkoCounterexample` | 通过 | 483 秒 | — |
| `HotSpots` | 通过 | 1,129 秒 | — |
| `MahlerConjecture` | 通过 | 337 秒 | — |
| `HadwigerCounterexample` | 通过 | 552 秒 | — |
| `Nagata` | 通过 | 268 秒 | — |

所以 10 份全部通过；单份 62–2,812 秒，差别主要来自要编译的模块数和当时的内存压力。

停掉 8765 的 4 个 Mathlib `verify_worker` 与 colima 虚拟机（维护者 2026-10-14 决定）后，交换区用量由约 17.4 GB 降到 13.0 GB，
单个模块编译由 48–61 秒降到 4–6 秒（前后各 8 个模块，不是同一批模块的对照）。colima 里跑着 MathDoc 的 PostgreSQL 与
TerminusDB，停掉期间 MathDoc 不可用；核验在 LeanGround 照常进行，`colima start` 后打开导入页即取回结果。

**部署**（本机）：LeanGround 18765 由本会话按原配置重启（原进程已不在），加上项目核验配置，工作区 `openai-math` 指向
`.openai-math-corpus/upstream/lean`，`isolated: false`；MathDoc serve 与 worker 配 `MDC_IMPORT_SOURCES` 重启
（`.openai-math-corpus/ops/restart-mathdoc.mjs`）；旧库 `oai-seymour-second-neighborhood` 已删除，两块看板已退役。

## 6. 资源估算

依据 [实测，阶段 0，n = 2 份配置]：Seymour（43 个模块、4,861 行）端到端 83 秒；EuclideanFiveColor（70 个模块、30,336 行）首次 675 秒，
编译已缓存时 207 秒，即导出与内核重放约 3 分钟；编译约 470 秒，折合约 65 行/秒（多核并行）。113 个模块（约 3.5 万行）的产物：
`.olean` 84 MB（约 2.4 KB/行），Lake 同时生成的 C 代码（`ir/`）577 MB（约 16.4 KB/行）。

[推断：按上述速率线性外推；导出与重放的耗时取决于目标用到多少 Mathlib，不随 OAI 行数线性变化，下表未计入]

| 范围 | 编译估算 | 所以 |
|---|---|---|
| 中位数一份（2.25 万行） | 约 6 分钟 | 本机可做 |
| 第 90 百分位（12.4 万行） | 约 32 分钟 | 本机可做，需排队 |
| 最大一份（283 万行） | 约 12 小时 | 需要更强的机器 |
| 全部（约 2,600 万行） | 约 110 小时；`.olean` 约 62 GB，C 代码约 430 GB | 本机磁盘剩约 12 GiB [实测，2026-10-13]，全量核验需要换机器。清理 C 代码（`ir/`）能省磁盘，但 Lake 会把被清理的模块整个重编 [实测：保留时重跑 Seymour 重编 0 个模块，清理后 43 个]，所以只在磁盘吃紧的批量核验里清理 |

## 7. 决定与待定

已定（2026-10-14）：

1. 项目核验作业放在 LeanGround。
2. 正式核验在本机 colima 的 Linux 容器里用真 `landrun`；阶段 0 在 macOS 上用 `fake-landrun`，结果标为「未隔离」。
3. 核验记录存在 MathDoc（§3.3）。

待定（附建议）：

4. 定义洞的审阅：建议先按 §3.2 的三类由机器分流，只有「与陈述不同」和「陈述为 `sorry`」两类交人审；由项目 owner 审（与协作中审根陈述是同一角色）；
   审阅记录（谁、何时、结论、意见）与核验记录存在一起；审完前显示「通过（待审）」。
5. ~~`oai-seymour-second-neighborhood` 怎么处理~~：已定（2026-10-14）删除并重建为导入项目 `oaic-seymour-second-neighborhood`。
6. ~~源码浏览是否进第一版~~：已定（2026-10-14）做，已实现（§5.2）：按需从上游固定提交读取。同日改定：证明的导入闭包另外逐文件建成
   文档节点，原文复制进 TerminusDB（§3.3、§8）；导入页的源码标签保留。

## 8. 修订记录

2026-10-14 按 Codex 审阅修订：

| 审阅意见 | 核实 | 改动 |
|---|---|---|
| 「宽松模式」改称「项目核验」，与「协作认证」并列；区别在输入与产出；只加一个异步作业接口，不重构成两套模式 | — | §1、§3.1 改名与改写；删去「严格 / 宽松模式」的表述 |
| 直接采用 Comparator，按它实际保证的范围定义「通过」；不要求无关常量都通过；不自行实现 | README 的保证：陈述相同、公理受限、被内核接受；内部步骤为「核对陈述所用声明相同、重放 Solution 环境」 | §3.2 重写为四条承诺与两条不承诺 |
| `definition_names` 是允许补全的定义洞，不能要求定义体与陈述模块相同 | README：定义洞须在陈述模块以 `sorry` 给出，只核对名字、类型、宇宙参数、安全级别。本地统计：65 个定义洞中 63 个写了完整定义体 | §3.2 改正语义；新增「定义洞的含义要单独展示」与阶段 0 的反例 |
| 外部核验不依赖协作看板；一份配置一个导入项目，挂多个核验目标；要协作时再建看板 | MathDoc 作业表外键要求看板、LeanGround 记录类型有 CHECK 约束 [实测] | §3.3 新写；原待定「多定理配置」随之解决 |
| 三种模式不作为前置工程；如要实现只做新建时选择与单向升级 | — | §3.4 缩为方向与最小范围 |
| 语法放宽、模块提升、多运行时服务化移出主线；第一版用指定版本的 Lean 工作区 | — | 新增 §4；§5 只保留主线阶段 |

修订前的版本另有一条「内核重放项目的全部常量」，已按 Comparator 的范围改为「目标所需的依赖经过重放」。

2026-10-14 阶段 0 之后再修订：

| 内容 | 改动 |
|---|---|
| Comparator 的行为 | §3.2 由「读 README」改为「读源码」：非定义洞的陈述依赖逐项比较（含定义体），定义洞只比较签名，重放范围是导出的目标依赖闭包 |
| 定义洞 | T6 证实弱点，T8 给出无需新代码的核对办法；§3.2 加三类分流 |
| 资源估算 | §6 改用阶段 0 的实测速率；磁盘估算由「约 1.4 KB/行、全部约 35 GB」改为「`.olean` 约 2.4 KB/行、C 代码约 16.4 KB/行」，原估算漏了 C 代码，且按 Seymour 一份外推偏低 |
| 决定 | §7 记下作业位置、隔离方式、记录位置三项决定 |

2026-10-14 维护者看过导入结果后再修订：文档里只有文字节点，没有 Lean 代码，与「每个文件一个节点、原文放在 Lean 块、边是 import」的
预期不一致。改为逐文件建节点（§3.3），10 个库用 `mdc external rebuild` 原地重建 [实测：每库不到 1.5 秒；节点数 21–294，
共 992 个文件节点；核验记录与进行中的 LeanGround 作业不受影响]：

| 库 | 文件节点 | 全部节点 | 依赖边 |
|---|---|---|---|
| Naimark | 16 | 21 | 22 |
| HotSpots | 31 | 36 | 43 |
| Sidorenko | 35 | 40 | 40 |
| DirectCrouzeix | 42 | 48 | 88 |
| Seymour | 43 | 48 | 65 |
| EuclideanFiveColor | 70 | 75 | 88 |
| Nagata | 117 | 123 | 193 |
| Kaplansky | 121 | 128 | 128 |
| Mahler | 229 | 236 | 343 |
| Hadwiger | 288 | 294 | 367 |

逐文件建节点之后，主界面的图在 4 个库上退回了旧的 canvas 点阵：卡片图（`DocumentGraph`）的布局盒数上限是 450
（盒 = 卡片 + 长边每跨一层的折点，`docs/mathdoc.md` 2026-10-13 按随机图定的）。各库的盒数与布局用时
[实测：tsx 调用 `layoutDocument`，读各库 `/graph/full`，预热 2 次后取第 3 次，各 n = 1]：

| 库 | 节点 | 边 | 盒 | 布局毫秒 |
|---|---|---|---|---|
| DirectCrouzeix | 48 | 88 | 869 | 25 |
| Nagata | 123 | 193 | 509 | 201 |
| Mahler | 236 | 343 | 621 | 312 |
| Hadwiger | 294 | 367 | 2,773 | 492 |
| 其余 6 个库 | 21–128 | 22–128 | 23–129 | 0–14 |

上限改为 3,000：原来就画成卡片的图不受影响，这 4 个库改画卡片，最慢的 Hadwiger 打开图视图时布局约 0.5 秒。
去掉传递边（a→c 而 c 已能经 a 的其他依赖到达）只对 DirectCrouzeix 有效（869 → 48 盒），对其余 3 个库几乎没用
（Hadwiger 2,773 → 2,267），而且会改变 10-13 已定的 029 等图的样子，所以没有采用。

2026-10-14 实施中再修订：「每份核验后必须清理 `ir/`」是错的——清理后 Lake 会整模块重编（§6 的实测）。项目核验改为默认保留，
并补上执行期间续租（原先租约一次取约 8 小时，进程中途重启时这份核验要等 8 小时）。

## 9. 未覆盖

- 阶段 0 未隔离；colima 里的真 `landrun` 没有跑过。待核实：macOS 编译的 `.olean` 能否在 Linux 容器里复用、依赖如何放进 `landrun`
  允许读取的路径、宿主机磁盘是否够用。
- 阶段 0 只有 2 份配置、每个测试 1 次；§6 的外推可能偏差很大。
- 能否让 Lake 不生成 C 代码（`ir/`）没有研究；Comparator 固定调用 `lake build <模块>`。
- 内核重放的内存占用没有测。
- 2 份配置要求 nanoda，依赖没看；第一版的项目核验明确拒绝 `enable_nanoda: true`。
- 创建导入项目时若建库成功而导入节点失败，会留下一个没有导入记录的空库，重试会因库已存在而失败，需手动删库。
- 本机内存是瓶颈 [实测，2026-10-14]：核验期间交换区 17.4 GB 用掉约 16.9 GB；Comparator 单份最大常驻内存 2.2–5.8 GB（Seymour、DirectCrouzeix、
  Kaplansky 各 n = 1）。Kaplansky 端到端 1,444 秒，多数时间可能耗在换页上 [推断]。
- 论文、主声明、配置三者的对应关系没有核对：上游列 173 份论文、200 条主声明、416 份配置，不是一一对应。
- 陈述是否忠实于论文，这套流程不判断，仍需人审（范围说明 `docs/NNN.md` 是依据）。
- 工作区里有其他会话未提交的前端与后端改动（协作页重构等），§3.3、§3.4 的实现会动到同一批文件，需先对齐。
- 本方案没有同步到 `docs/mathdoc.md`。
- 文件节点的原文可以在文档里改；改过的节点不再显示为「导入」，但项目核验仍按上游提交跑，不看文档里的文本。导入库上若再建协作看板，
  改过的文件节点会被当成普通 Lean 节点参与转换，这条路径没有测。
- 近 300 个节点的文档（Hadwiger、Mahler）在图视图里是否流畅没有测。
- 并行跑 coordinator 全部测试时，每次有 1–4 项随机失败（TerminusDB「数据库已存在」等争用）；逐文件顺序跑 77 项中 68 过、0 失败、
  9 跳过，与改动前相同。没有查争用的来源。
