<script lang="ts">
  import { onMount } from "svelte";
  import katex from "katex";
  import type { Board, Command, Task } from "../../coordinator/src/domain";
  import type { DocNode, Snapshot } from "../../coordinator/src/documents";
  let token = $state("");
  let actor = $state("");
  let admin = $state(false);
  let boards = $state<Board[]>([]);
  let selected = $state<Board>();
  let error = $state("");
  let busy = $state(false);
  let tab = $state("overview");
  let showCreate = $state(false);
  let title = $state("");
  let proposition = $state("");
  let base = $state(1);
  let budget = $state(100);
  let members = $state("");
  let context = $state("{}");
  let options = $state("{}");
  let binding = $state("");
  let projectMembers = $state("");
  let resolveStatement = $state("");
  let resolveGoal = $state("");
  let dependencies = $state("");
  let notes = $state("");
  let jobs = $state<
    Array<{ id: string; kind: string; status: string; error?: string }>
  >([]);
  let source = $state("");
  let expectedRoot = $state("target");
  let component = $state("lean-worker");
  let mode = $state<"leaf" | "sketch">("leaf");
  let goal = $state("");
  let certificate = $state("");
  let allocation = $state(10);
  let snapshot = $state<Snapshot>();
  let document = $state<DocNode>();
  let documentType = $state<"text" | "latex" | "lean" | "rocq">("text");
  let content = $state("");
  let docHistory = $state("");
  let branchName = $state("");
  let evidence = $state("");
  const labels: Record<string, string> = {
    insufficient: "尚缺证明",
    derivable: "可推导",
    certified: "已认证",
    ready: "可领取",
    leased: "进行中",
    cancelled: "已取消",
    active: "启用",
    paused: "暂停",
    retired: "退役",
    queued: "排队中",
    running: "执行中",
    done: "已完成",
    failed: "失败",
    prove: "证明",
    decompose: "探索拆法",
    formalize: "形式化",
  };
  async function api<T = any>(
    path: string,
    method = "GET",
    body?: unknown,
    headers: Record<string, string> = {},
  ): Promise<T> {
    // Collaboration routes live under /api/coordination (plan §6.3); identity is shared.
    const r = await fetch(
      `/api${path === "/me" ? "" : "/coordination"}${path}`,
      {
        method,
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
          ...headers,
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      },
    );
    const v = await r.json();
    if (!r.ok) throw new Error(v.reason ?? "请求失败");
    return v;
  }
  async function act(fn: () => Promise<void>) {
    if (busy) return;
    busy = true;
    error = "";
    try {
      await fn();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
  async function refresh() {
    boards = await api("/projects");
    if (selected) {
      selected = boards.find((b) => b.id === selected!.id);
      if (selected) jobs = await api(`/projects/${selected.id}/jobs`);
    }
  }
  async function login() {
    await act(async () => {
      const me = await api("/me");
      actor = me.actor;
      admin = me.admin;
      await refresh();
    });
  }
  function choose(b: Board) {
    selected = b;
    notes = b.notes;
    projectMembers = b.members.join(", ");
    resolveGoal = b.root;
    goal = b.root;
    tab = "overview";
    snapshot = undefined;
    document = undefined;
    evidence = "";
    void act(refresh);
  }
  async function cmd(c: Command) {
    await act(async () => {
      if (!selected) return;
      await api(`/projects/${selected.id}/commands`, "POST", c, {
        "if-match": `"${selected.revision}"`,
        "idempotency-key": crypto.randomUUID(),
      });
      await refresh();
    });
  }
  async function create() {
    await act(async () => {
      const b = await api<Board>(
        "/projects",
        "POST",
        {
          title,
          proposition,
          base_key: base,
          budget,
          members: members
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean),
          context: JSON.parse(context),
          options: JSON.parse(options),
          ...(binding.trim() ? { node_ref: JSON.parse(binding) } : {}),
        },
        { "idempotency-key": crypto.randomUUID() },
      );
      showCreate = false;
      await refresh();
      choose(b);
    });
  }
  function currentAttempt(t: Task) {
    return t.attempt && selected?.attempts[t.attempt];
  }
  async function loadDocs() {
    await act(async () => {
      snapshot = await api(`/projects/${selected!.id}/documents`);
      document = undefined;
    });
  }
  function selectDoc(n: DocNode) {
    document = structuredClone(n);
    dependencies = n.depens.join(", ");
    loadBlock();
  }
  function loadBlock() {
    content =
      document?.blocks.find((b) => b.srctype === documentType)?.content ?? "";
  }
  async function saveDoc() {
    await act(async () => {
      if (!document || !snapshot) return;
      document.depens = dependencies
        .split(",")
        .map((x) => x.trim())
        .filter(Boolean);
      const old = document.blocks.find((b) => b.srctype === documentType);
      document.blocks = document.blocks
        .filter((b) => b.srctype !== documentType)
        .concat({
          srctype: documentType,
          content,
          metadata: old?.metadata ?? {},
        });
      await api(`/projects/${selected!.id}/documents`, "PUT", {
        version: snapshot.version,
        node: document,
      });
      snapshot = await api(`/projects/${selected!.id}/documents`);
    });
  }
  function latex(text: string) {
    return katex.renderToString(text, {
      throwOnError: false,
      trust: false,
      displayMode: true,
      strict: "warn",
    });
  }
  onMount(() => {
    const timer = setInterval(() => {
      if (actor && !busy)
        void refresh().catch((e) => {
          error = e.message;
        });
    }, 5000);
    return () => clearInterval(timer);
  });
</script>

<svelte:head
  ><title>MathDoc · {selected?.title ?? "证明协作"}</title></svelte:head
>
{#if !actor}
  <main class="login">
    <div class="brand">M<span>·</span></div>
    <p class="eyebrow">MATHDOC / COLLABORATION</p>
    <h1>一起完成证明。</h1>
    <p class="muted">组织目标、共享引理，让每一步成果都有证据。</p>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void login();
      }}
    >
      <label
        >访问令牌<input
          type="password"
          bind:value={token}
          autocomplete="off"
          required
          placeholder="由项目管理员提供"
        /></label
      ><button disabled={busy}>进入工作台 →</button>
    </form>
    {#if error}<p role="alert" class="error">{error}</p>{/if}
  </main>
{:else}
  <div class="shell">
    <aside>
      <div class="wordmark">MathDoc <span>协作</span></div>
      <p class="eyebrow">证明项目</p>
      {#each boards as b}<button
          class:chosen={selected?.id === b.id}
          class="project"
          onclick={() => choose(b)}
          >{b.title}<small>{labels[b.goals[b.root]?.status]}</small></button
        >{/each}{#if admin}<button
          class="secondary"
          onclick={() => {
            showCreate = true;
          }}>＋ 新建项目</button
        >{/if}
      <footer>
        <span class="avatar">{actor.slice(0, 1).toUpperCase()}</span
        >{actor}<button
          class="link"
          onclick={() => {
            actor = "";
            token = "";
            selected = undefined;
            boards = [];
          }}>退出</button
        >
      </footer>
    </aside>
    <main>
      <header>
        <div>
          <p class="eyebrow">共享证明工作台</p>
          <h1>{selected?.title ?? "选择一个项目"}</h1>
        </div>
        {#if selected}<button
            class="secondary"
            disabled={busy}
            onclick={() => cmd({ type: "sync" })}>同步成果</button
          >{/if}
      </header>
      {#if error}<div role="alert" class="error">
          {error}<button
            class="link"
            onclick={() => {
              error = "";
            }}>关闭</button
          >
        </div>{/if}
      {#if showCreate}<section class="card">
          <h2>创建证明项目</h2>
          <form
            onsubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <div class="grid">
              <label>项目名称<input bind:value={title} required /></label><label
                >Base 编号<input
                  type="number"
                  min="0"
                  bind:value={base}
                  required
                /></label
              ><label
                >总预算（统一记账单位）<input
                  type="number"
                  min="0"
                  bind:value={budget}
                  required
                /></label
              ><label>成员，逗号分隔<input bind:value={members} /></label>
            </div>
            <label
              >Lean 命题<textarea
                bind:value={proposition}
                required
                placeholder="∀ n : Nat, n = n"></textarea></label
            >
            <details>
              <summary>证明环境与文档绑定</summary><label
                >Context JSON<textarea bind:value={context}></textarea></label
              ><label
                >Options JSON<textarea bind:value={options}></textarea></label
              ><label
                >文档引用 JSON（database、branch、node、revision）<textarea
                  bind:value={binding}
                  placeholder="可选：绑定现有文档节点及分支版本"
                ></textarea></label
              >
            </details>
            <div class="actions">
              <button disabled={busy}>创建项目</button><button
                type="button"
                class="secondary"
                onclick={() => {
                  showCreate = false;
                }}>取消</button
              >
            </div>
          </form>
        </section>{/if}
      {#if selected}
        <nav>
          {#each [["overview", "概览"], ["tasks", "任务"], ["routes", "拆法"], ["review", "成果审阅"], ["documents", "知识文档"]] as [id, label]}<button
              class:active={tab === id}
              onclick={() => {
                tab = id;
              }}>{label}</button
            >{/each}
        </nav>
        {#if !selected.ready}<p class="notice">
            项目正在初始化。后台服务会建立事实共享范围。
          </p>{/if}
        {#if selected.sync_error}<p class="error">
            同步异常：{selected.sync_error}
          </p>{/if}
        {#if tab === "overview"}
          <div class="stats">
            <section>
              <small>根目标</small><strong
                >{labels[selected.goals[selected.root].status]}</strong
              >
            </section>
            <section>
              <small>共享目标</small><strong
                >{Object.keys(selected.goals).length}</strong
              >
            </section>
            <section>
              <small>已记账 / 总预算</small><strong
                >{selected.spent} / {selected.budget}</strong
              >
            </section>
            <section>
              <small>最近同步</small><strong class="small"
                >{selected.last_sync
                  ? new Date(selected.last_sync).toLocaleTimeString()
                  : "等待同步"}</strong
              >
            </section>
          </div>
          <section class="card">
            <p class="eyebrow">待证明命题</p>
            <pre>{selected.goals[selected.root].statement}</pre>
            <p class="muted">
              信任要求：{selected.minimum_trust} · 成员：{selected.members.join(
                "、",
              )}
            </p>
            <details>
              <summary>目标身份与证明环境</summary>
              <pre>{JSON.stringify(
                  {
                    goal: selected.goals[selected.root].identity,
                    base: selected.base,
                    context: selected.context,
                    options: selected.options,
                  },
                  null,
                  2,
                )}</pre>
            </details>
          </section>
          {#if selected.owner === actor}<section class="card">
              <h2>项目成员</h2>
              <label
                >用户名，逗号分隔<input bind:value={projectMembers} /></label
              ><button
                disabled={busy}
                onclick={() =>
                  cmd({
                    type: "members",
                    members: projectMembers
                      .split(",")
                      .map((s) => s.trim())
                      .filter(Boolean),
                  })}>更新成员</button
              >
              <p class="muted">
                移除成员立即撤销工作台访问与任务租约；事实层共享范围由后台同步。
              </p>
            </section>{/if}
          <section class="card">
            <h2>协作说明</h2>
            <textarea
              rows="5"
              bind:value={notes}
              placeholder="记录思路、分工和需要审阅的问题"></textarea><button
              disabled={busy}
              onclick={() => cmd({ type: "notes", notes })}>保存说明</button
            >
          </section>
        {:else if tab === "tasks"}
          <div class="section-title">
            <h2>目标与工作任务</h2>
            <label
              >本次预留预算<input
                class="short"
                type="number"
                min="0"
                bind:value={allocation}
              /></label
            >
          </div>
          {#each Object.values(selected.tasks).sort((a, b) => b.priority - a.priority) as task}
            {@const attempt = currentAttempt(task)}
            <section class="card task">
              <div>
                <span class="badge">{labels[task.kind]}</span>
                <h3>
                  {selected.goals[task.goal].statement ||
                    task.goal.slice(0, 24)}
                </h3>
                <code title={task.goal}>{task.goal.slice(0, 24)}…</code>
                <p class="muted">
                  数学状态：{labels[selected.goals[task.goal].status]} · 工作状态：{labels[
                    task.state
                  ]}
                </p>
              </div>
              <div class="actions">
                {#if attempt}<div>
                    <small
                      >{attempt.actor} · 租约至 {new Date(
                        attempt.expires,
                      ).toLocaleTimeString()}</small
                    >{#if attempt.actor === actor}<div class="actions">
                        <button
                          class="secondary"
                          disabled={busy}
                          onclick={() =>
                            cmd({
                              type: "heartbeat",
                              attempt: attempt.id,
                              epoch: attempt.epoch,
                              ttl: 300,
                            })}>续租</button
                        ><button
                          class="secondary"
                          disabled={busy}
                          onclick={() =>
                            cmd({
                              type: "finish",
                              attempt: attempt.id,
                              epoch: attempt.epoch,
                              spent: attempt.reserved,
                              outcome: "released",
                            })}>结束尝试</button
                        >
                      </div>
                      <small>结束按预留额记账；实际费用可通过 API 回报。</small
                      >{/if}
                  </div>{:else}<button
                    disabled={busy ||
                      !selected.ready ||
                      selected.goals[task.goal].status === "certified"}
                    onclick={() =>
                      cmd({
                        type: "claim",
                        task: task.id,
                        allocation,
                        ttl: 300,
                      })}>领取任务</button
                  >{/if}
              </div>
            </section>{/each}
          <section class="card">
            <h2>登记子目标陈述</h2>
            <p class="muted">
              新拆法中的前提只有身份时，填写其 Lean 陈述；服务会解析并核对
              GoalKey，匹配后才能提交证明。
            </p>
            <label
              >目标<select bind:value={resolveGoal}
                >{#each Object.values(selected.goals) as g}<option value={g.key}
                    >{g.statement || g.key}</option
                  >{/each}</select
              ></label
            ><label
              >Lean 陈述<textarea bind:value={resolveStatement}
              ></textarea></label
            ><button
              disabled={busy || !resolveStatement}
              onclick={() =>
                cmd({
                  type: "resolve",
                  goal: resolveGoal,
                  proposition: resolveStatement,
                })}>解析并核对</button
            >{#if selected.owner === actor}<button
                class="secondary"
                disabled={busy}
                onclick={() =>
                  cmd({
                    type: "task",
                    goal: resolveGoal,
                    kind: "decompose",
                    priority: 0,
                  })}>创建拆分任务</button
              >{/if}
          </section>
          <section class="card">
            <h2>提交成果</h2>
            <p class="muted">
              完整证明选择 leaf；草图选择 sketch
              并填写专门验证组件。提交由后台验证，不能自行标记认证成功。
            </p>
            <label
              >目标<select bind:value={goal}
                >{#each Object.values(selected.goals).filter((g) => g.identity) as g}<option
                    value={g.key}>{g.statement || g.key}</option
                  >{/each}</select
              ></label
            >
            <div class="grid">
              <label>组件<input bind:value={component} /></label><label
                >结论声明名<input bind:value={expectedRoot} /></label
              ><label
                >类型<select bind:value={mode}
                  ><option value="leaf">leaf</option><option value="sketch"
                    >sketch</option
                  ></select
                ></label
              >
            </div>
            <textarea
              rows="8"
              bind:value={source}
              placeholder="theorem target ..."></textarea><button
              disabled={busy || !source}
              onclick={() =>
                cmd({
                  type: "submit",
                  goal,
                  source,
                  expected_root: expectedRoot,
                  component,
                  mode,
                })}>提交验证</button
            >
            <hr />
            <label>导入已有认证 ID<input bind:value={certificate} /></label
            ><button
              class="secondary"
              disabled={busy || !certificate}
              onclick={() => cmd({ type: "import", certificate })}
              >读取并核对认证</button
            >
          </section>
        {:else if tab === "routes"}
          <h2>共享证明路线</h2>
          <p class="muted">
            同一个目标可以有多种拆法。暂停只影响选路，已验证的数学事实仍然保留。
          </p>
          {#each Object.values(selected.facts) as fact}<section class="card">
              <div class="section-title">
                <code>{fact.goal.slice(0, 24)}…</code><span class="badge"
                  >{fact.available ? labels[fact.state] : "暂不可访问"}</span
                >
              </div>
              <div class="route">
                {#if !fact.premises.length}<span class="success"
                    >无前提认证</span
                  >{:else}{#each fact.premises as premise}<span class="chip"
                      >{premise.slice(0, 16)}… · {labels[
                        selected.goals[premise].status
                      ]}</span
                    >{/each}<span>→ 结论</span>{/if}
              </div>
              <small>认证 {fact.id}</small
              >{#if selected.owner === actor && fact.premises.length}<div
                  class="actions"
                >
                  {#each ["active", "paused", "retired"] as state}<button
                      class="secondary"
                      disabled={busy || fact.state === state}
                      onclick={() =>
                        cmd({
                          type: "decomposition",
                          certificate: fact.id,
                          state: state as "active" | "paused" | "retired",
                        })}>{labels[state]}</button
                    >{/each}
                </div>{/if}
            </section>{/each}
          {#if !Object.keys(selected.facts).length}<p class="empty">
              尚未收到认证。先领取目标并提交证明，或导入已有认证。
            </p>{/if}
        {:else if tab === "review"}
          <section class="card">
            <h2>组装最终证明</h2>
            <p class="muted">
              使用启用的拆法固定证据链，再由 LeanGround 组装并检查完整证明。
            </p>
            <label>验证组件<input bind:value={component} /></label><button
              disabled={busy || selected.owner !== actor || !selected.ready}
              onclick={() => cmd({ type: "assemble", component })}
              >创建计划并组装</button
            >
          </section>
          {#each Object.values(selected.runs) as run}<section class="card">
              <div class="section-title">
                <code>{run.id.slice(0, 16)}</code><span class="badge"
                  >{labels[run.status]}</span
                >
              </div>
              {#if run.failure}<p class="error">{run.failure}</p>{/if}
              <p>计划：{run.plan_id ?? "等待生成"}</p>
              {#if run.certification_id}<p>认证：{run.certification_id}</p>
                <button
                  class="secondary"
                  onclick={() =>
                    act(async () => {
                      evidence = JSON.stringify(
                        await api(
                          `/projects/${selected!.id}/facts/${run.certification_id}`,
                        ),
                        null,
                        2,
                      );
                    })}>查看证据与源码</button
                >{/if}
              <div class="actions">
                {#if run.status === "failed" && run.retryable}<button
                    disabled={busy || selected.owner !== actor}
                    onclick={() => cmd({ type: "retry", run: run.id })}
                    >重试同一计划</button
                  >{/if}{#if run.status === "certified" && selected.node_ref}<button
                    disabled={busy || selected.owner !== actor}
                    onclick={() => cmd({ type: "accept", run: run.id })}
                    >接受并写回绑定文档</button
                  >{/if}
              </div>
            </section>{/each}
          {#if selected.accepted}<p class="notice">
              成果已写回文档，版本：{selected.accepted.revision}
            </p>{/if}{#if evidence}<section class="card">
              <h2>认证证据</h2>
              <pre>{evidence}</pre>
            </section>{/if}
          <h2>后台操作</h2>
          {#each jobs as job}<div class="job">
              <span>{job.kind}</span><span>{labels[job.status]}</span><small
                >{job.error ?? ""}</small
              >
            </div>{/each}
        {:else if tab === "documents"}
          {#if selected.node_ref}<div class="section-title">
              <h2>{selected.node_ref.database} / {selected.node_ref.branch}</h2>
              <button class="secondary" onclick={loadDocs} disabled={busy}
                >读取文档</button
              >
            </div>
            <p class="muted">
              编辑文档会产生新版本；已创建证明请求仍绑定旧版本，接受成果时会检查冲突。
            </p>
            <div class="actions">
              <button
                class="secondary"
                onclick={() =>
                  act(async () => {
                    docHistory = JSON.stringify(
                      await api(`/projects/${selected!.id}/document-history`),
                      null,
                      2,
                    );
                  })}>版本历史</button
              ><input
                aria-label="新分支名"
                placeholder="新分支名"
                bind:value={branchName}
              /><button
                class="secondary"
                disabled={busy || selected.owner !== actor || !branchName}
                onclick={() =>
                  act(async () => {
                    await api(`/projects/${selected!.id}/branches`, "POST", {
                      name: branchName,
                    });
                    branchName = "";
                  })}>创建文档分支</button
              >
            </div>
            {#if snapshot}{#if selected.owner === actor}<button
                  class="secondary"
                  onclick={() => {
                    const id = crypto.randomUUID();
                    selectDoc({
                      fnode: id,
                      title: "新节点",
                      module: "Lib.N_" + id.replaceAll("-", ""),
                      depens: [],
                      blocks: [],
                    });
                  }}>＋ 新建知识节点</button
                >{/if}
              <div class="document-layout">
                <div>
                  {#each snapshot.nodes as n}<button
                      class="project"
                      onclick={() => selectDoc(n)}>{n.title}</button
                    >{/each}
                </div>
                {#if document}<section class="card">
                    <label>标题<input bind:value={document.title} /></label
                    ><label
                      >内容类型<select
                        bind:value={documentType}
                        onchange={loadBlock}
                        ><option>text</option><option>latex</option><option
                          >lean</option
                        ><option>rocq</option></select
                      ></label
                    ><textarea rows="14" bind:value={content}
                    ></textarea>{#if documentType === "latex"}<div class="math">
                        {@html latex(content)}
                      </div>{/if}<label
                      >知识依赖 UUID，逗号分隔<input
                        bind:value={dependencies}
                      /></label
                    ><button
                      disabled={busy || selected.owner !== actor}
                      onclick={saveDoc}>保存新版本</button
                    >
                  </section>{/if}
              </div>{/if}{#if docHistory}<pre>{docHistory}</pre>{/if}
          {:else}<p class="empty">
              此项目未绑定文档节点。创建项目时可关联现有 TerminusDB 文档及版本。
            </p>{/if}
        {/if}
      {:else if !showCreate}<section class="empty">
          <h2>从一个待证命题开始</h2>
          <p>选择左侧项目，查看共享目标、领取工作并审阅成果。</p>
        </section>{/if}
    </main>
  </div>
{/if}
