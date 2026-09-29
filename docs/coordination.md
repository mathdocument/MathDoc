# MathDoc 协调应用：重构、接口与验收

本分支将默认应用改为 **TypeScript + Svelte + PostgreSQL**。LeanGround 保存认证事实、校验计划并组装；
独立形式化证明组件负责草图检查。MathDoc 负责协作与文档审阅。不集成 EvoHarness，不实现证明搜索算法。

## 模块裁剪

| 模块                                              | 本分支处理                                                              |
| ------------------------------------------------- | ----------------------------------------------------------------------- |
| 默认 API、后台作业、配置                          | 新增 `coordinator/` TypeScript 实现                                     |
| 默认界面                                          | 新增 `app/` Svelte 工作台，复用原 MathDoc 的暗色视觉与数学内容概念      |
| 文档、知识依赖、历史与分支                        | 通过 TerminusDB HTTP 适配器原位读写；不迁移、不删除数据                 |
| Lean/Rocq 内容                                    | 保留内容编辑；编辑不会启动证明器                                        |
| 本地 Lean/Lake、Worker、编译认证、Python 排版服务 | 从默认启动、依赖安装和镜像裁出；原实现留在 `src/`                       |
| Monaco、VS Code shim、Infoview、TikZ WASM         | 从默认前端依赖和构建裁出；完整编辑器留在 `web/`                         |
| 旧 Rust/Web 应用                                  | 使用 `Dockerfile.legacy`、`compose.legacy.yaml` 或原 Cargo 命令单独启动 |
| 旧性能实验与完整 IDE 回归                         | 保留，新的默认 CI 独立验证协调应用；旧发布检查改为手动 workflow         |

没有机械删除尚有数据兼容价值的代码。`npm ci` 仅安装根工作区 `coordinator` 和 `app`，不会安装
`web` 的重型依赖。新容器不安装 Rust、Lean、Lake 或 Python。

新界面包含证明概览、任务、拆法、成果审阅、知识文档五个视图。基础 LaTeX 用 KaTeX 展示；
旧应用的 BibTeX、跨节点排版宏、TikZ 和交互 Infoview 仍应使用 legacy 应用，尚未迁入轻量工作台。
现有节点的其他 block 和元数据在编辑另一 block 时保留。手动编辑清除新协调层认证标记，不能沿用旧认证。

## 数据与身份

- PostgreSQL `mdc_project`：固定环境、目标、任务、尝试、引用认证、计划运行和文档绑定。
- `mdc_command`：按项目、调用者、幂等键去重；同键不同请求返回冲突。
- `mdc_job`：持久队列、作业租约、重试次数和机器可读错误。
- `mdc_event`：项目操作审计。
- TerminusDB：原有 Node/Project 文档模式、知识依赖与版本历史。
- LeanGround：认证、源码、验证证据和固定组装计划；MathDoc 不复制源码到事实缓存。

首版将一个协调项目的状态存为一条 JSONB，以行锁保证跨表事务。支持多 API/worker 进程共享数据库，
但同一项目的状态写入串行。未做大型图和高并发负载验收；规模扩大时应把目标、任务与尝试规范化分表，
不把当前模型宣传为千 Agent 调度器。

Goal 按服务返回的 GoalKey 合并。文档引用包含 database、branch、node 和 **整个分支的
TerminusDB-Data-Version**。这是保守的版本保护：即使其他节点改变，回写也可能要求重新审阅，
但不会忽略上游变化覆盖旧证据。

数学状态 `insufficient/derivable/certified` 由已核对事实推导；本地路线只用于选择工作和候选证据链。
LeanGround 再次检查计划和完整组装。暂停/退役不删除数学事实，但选路会排除这些拆法。无叶子证据的环
不能自我证明。证否关联、跨 Base 重放、新定义支持不在本次实现中。

## 配置与运行

参考根目录 `config.example.env`。必须配置：

- `MDC_DATABASE_URL`：协调数据库连接；迁移命令只需要这一项。
- `MDC_ACTORS`：用户名到 `{token, admin}` 的 JSON；令牌至少 16 字符且各不相同。
- `LEANGROUND_SERVER_URL`、`LEANGROUND_FACT_TOKEN`、`LEANGROUND_ACTOR`：固定服务地址和协调服务身份。
  actor 必须与 LeanGround 为 token 配置的身份一致。
- 文档功能还需要 `MDC_TERMINUS_URL`、`MDC_TERMINUS_USER`、`MDC_TERMINUS_PASSWORD`。

`MDC_HOST` 默认 127.0.0.1，`MDC_PORT` 默认 17843，`MDC_APP_DIR` 默认 `app/dist`。
不自动读取 `.env`，请由 shell 或进程管理器注入环境。生产反向代理负责 TLS；这是静态令牌身份系统，
不是完整账号注册、SSO 或不可信代码沙箱。

```sh
npm ci
npm run build
npm run migrate
npm start
# 另一个终端，使用相同配置
npm run worker
```

`npm run dev` 启动前端开发服务，代理至本机协调 API。生产服务直接提供已构建的静态资源。
容器启动使用 `docker compose -f compose.server.yaml up --build`；独立 migrate 服务先完成迁移。
容器中的 LeanGround/TerminusDB 地址必须可达，不能把容器自身的 localhost 当成宿主机。
TerminusDB 沿用现有实例；首次创建文档数据库仍用原 MathDoc 管理入口。

## 完整控制流

1. 管理员创建项目，调用 LeanGround 获取 Base 并解析根 GoalKey。可绑定现有文档节点及分支版本。
2. 本地创建与初始化作业同事务保存；后台登记 LeanGround 项目成员后，项目进入可领取状态。
3. 成员领取 Task，同时预留预算、创建 Attempt、递增 lease epoch。
4. Agent 在外部尝试，定期续租；可提交完整证明或选择专门草图组件提交条件证明。
5. 服务端读取并核对认证，检查环境、最低信任要求、项目可见性；去重目标并更新路线。
   只有身份而没有陈述的新子目标，可提交 `resolve` 命令，核对解析所得 GoalKey 后再证明。
6. 后台读取 LeanGround 事件；整批更新与返回的 cursor 同事务保存。相同事实的放宽范围事件仍会处理。
7. 所有前提已闭合时，维护者请求组装。协调层从启用路线生成候选，由 LeanGround 固定计划再组装。
8. 最终无前提认证经过实时读取核对后进入审阅；维护者接受时通过 TerminusDB 版本前置条件回写。

成果也可以由参与者直接提交到 LeanGround，然后在工作台导入认证 ID。必须是公开事实或共享到同一个
项目的事实，不能把协调服务自己能读取的私有事实直接公布给项目成员。

新计划、组装认证最初属于协调服务身份；服务请求放宽为项目范围，LeanGround 检查传递输入权限。
只有放宽及重新读取成功后，工作台才标记组装成功。本次不自动公开成果，不执行 managed admission 或 promotion。

## API

所有 `/api` 请求使用 `Authorization: Bearer <token>`，身份不接受请求体自报。
错误为 `{reason}`，未知/不可见项目均返回 404；未登录返回 401。

| 请求                                       | 用途                                                       |
| ------------------------------------------ | ---------------------------------------------------------- |
| `GET /api/me`                              | 当前身份与管理员角色                                       |
| `GET /api/projects`                        | 当前成员可见项目                                           |
| `POST /api/projects`                       | 管理员建项目；要求 `Idempotency-Key`                       |
| `GET /api/projects/:id`                    | 固定环境、目标、任务、尝试、成果                           |
| `POST /api/projects/:id/commands`          | 协作命令；要求 `If-Match: "revision"` 和 `Idempotency-Key` |
| `GET /api/projects/:id/jobs`               | 最近后台操作和失败原因，不返回候选源码                     |
| `GET /api/projects/:id/history`            | 最近协作审计记录                                           |
| `GET /api/projects/:id/facts/:certificate` | 实时核对权限后读取认证与源码                               |
| `GET /api/documents?database=D&branch=B`   | 管理员选择文档绑定，返回分支 version 与节点                |
| `GET /api/projects/:id/documents`          | 读取绑定分支的知识节点                                     |
| `PUT /api/projects/:id/documents`          | owner 以 `{version,node}` 创建/编辑节点，检查依赖闭环      |
| `GET /api/projects/:id/document-history`   | TerminusDB 历史                                            |
| `POST /api/projects/:id/branches`          | owner 以 `{name}` 创建文档分支                             |

创建项目字段：`title, proposition, base_key, context?, options?, members, budget,
minimum_trust?, node_ref?`。`node_ref` 为 `{database,branch,node,revision}`。
成员须在服务的 `MDC_ACTORS` 中配置。

命令使用共享运行时 schema（`coordinator/src/domain.ts`）：

| type          | 其他字段                                     | 权限/语义                                                  |
| ------------- | -------------------------------------------- | ---------------------------------------------------------- |
| notes         | notes                                        | 成员更新协作说明                                           |
| members       | members                                      | owner；移除者立即失去工作台权限，后台同步事实层成员        |
| task          | goal, kind, priority?                        | owner；kind 为 prove/decompose/formalize                   |
| claim         | task, allocation, ttl?                       | 成员；ttl 30–3600 秒，默认 300                             |
| heartbeat     | attempt, epoch, ttl?                         | 仅有效租约持有人                                           |
| finish        | attempt, epoch, spent, outcome               | outcome 为 no_progress/submitted/execution_failed/released |
| decomposition | certificate, state                           | owner；active/paused/retired                               |
| resolve       | goal, proposition                            | 解析子目标陈述，必须匹配已有 GoalKey                       |
| sync          | 无                                           | 请求同步；初始化失败时重试初始化                           |
| import        | certificate                                  | 实时读取并核对已有认证                                     |
| submit        | goal, source, expected_root, component, mode | mode 为 leaf/sketch                                        |
| assemble      | component                                    | owner；选择启用路线并固定计划                              |
| retry         | run                                          | owner；仅可重试失败，保留 plan_id、新建 attempt_id         |
| accept        | run                                          | owner；已认证成果回写绑定文档                              |

示例：

```json
{ "type": "claim", "task": "任务UUID", "allocation": 10, "ttl": 300 }
```

命令响应含 `revision, result, job?`。异步接收不表示证明成功；查询 jobs/项目状态取得最终结果。
Agent 应持久保存幂等键和原请求，断线后使用同一键重试。浏览器目前遇到不确定网络结果时应先刷新状态，
不会在后台静默重发用户操作。

## 恢复、预算和权限取舍

- 数据库行锁保证竞争领取与预算预留原子性，租约计时使用 PostgreSQL 时间。
- 租约过期或成员撤销，预留额按已花费记账，避免掉线导致重复分配预算。Agent 正常结束可回报实际开销。
  外部 Agent 的实际消费不受该预算强制控制。
- 旧租约不能结束新尝试，但迟到的有效认证仍可导入。
- 作业采用至少一次执行、租约 epoch 隔离。网络请求不占用数据库事务锁。
- 组件/网络故障自动有限重试；组装使用固定 run ID 作为 attempt_id，响应丢失不会改用新尝试。
  已保存的可重试组装失败由用户显式创建新 run，继续使用原计划。
- 回写在 Lean block metadata 保存操作 ID；响应丢失后可识别已完成写入，不覆盖之后的编辑。
- 成员变化立即更新本地授权；LeanGround 端成员变更是异步作业，完成前项目暂停新任务操作。
  直接持有 LeanGround 凭证的被移除成员，在远端同步完成前仍可能有短暂访问窗口，界面明确显示初始化状态。
- 定期重新读取已缓存认证，撤销不可访问的缓存；查看源码、计划生成、成果接受均再次核权。
- 不自动删除用户项目、数据库、文档或版本历史；不自动部署到已有服务。

## 验收与尚未完成的边界

自动测试覆盖状态机、真实 PostgreSQL 并发、幂等、作业租约恢复、HTTP 身份、事件游标原子性、
共享目标、暂停选路、组装失败重试、版本冲突、回写恢复和文档依赖检查。
集成测试中的 LeanGround/TerminusDB 使用确定性 HTTP 合同替身，**不等于真实 Lean 内核或现有生产
TerminusDB 的联调验收**。LeanGround 本身的内核测试属于其仓库。

```sh
npm run check
MDC_TEST_DATABASE_URL=postgresql://... npm test
npm run build
# 使用已安装的 Playwright Chromium，或通过 MDC_CHROME_PATH 指定测试浏览器
MDC_TEST_DATABASE_URL=postgresql://... npm run test:browser -w @mathdoc/app
```

浏览器验收覆盖登录、项目查看、领取/续租/结束任务、暂停拆法和移动端布局。测试使用临时 schema、
独立浏览器上下文，截图默认写入 `/tmp/mathdoc-browser`。

未承诺：千级负载、SSO、主动通知渠道、完整自动搜索、证否关联、通用文档分支合并、旧 IDE 的全部功能迁移。
Docker 构建及真实外部服务部署需在相应服务可用的环境中另行验收。

本次本地验收：9 项自动测试通过（其中 3 项使用真实 PostgreSQL），前后端类型检查 0 错误、0 警告；
生产构建和浏览器交互验收通过。外部证明/文档服务使用合同替身；本机 Docker daemon 未启动且缺少 Compose 插件，未执行容器构建/启动；相关 YAML 已通过语法解析。
本地 Node.js 为 v25.2.1，Node.js 22 的 CI 配置已加入，但本次未运行远端 CI。
