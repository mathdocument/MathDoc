<script lang="ts">
  // The four collaboration views (plan §4.2, §9): proof overview, task board,
  // decompositions and result review. People use the same commands as agents.
  import { onDestroy, onMount } from "svelte";
  import { coordination, currentBranch, goalName, type Board, type Command } from "../lib/coordination";
  import { errMsg } from "../lib/format";
  import { modal } from "../lib/modal";
  let { project = null, onClose, onOpenNode }: {
    project?: string | null; onClose: () => void; onOpenNode?: (fnode: string) => void;
  } = $props();
  type Tab = "overview" | "tasks" | "decompositions" | "review";
  const branch = currentBranch();
  let tab = $state<Tab>("overview");
  let boards = $state<Board[]>([]), selected = $state<string | null>(null);
  let board = $state<Board | null>(null), me = $state<string>("");
  let error = $state<string | null>(null), busy = $state(false);
  let jobs = $state<Awaited<ReturnType<typeof coordination.jobs>>>([]);
  let allocation = $state(100), ttl = $state(300);
  let alive = true, timer: ReturnType<typeof setInterval> | undefined;
  const now = () => Date.now();

  async function refresh() {
    if (!branch) return;
    try {
      boards = await coordination.projects(branch.database, branch.branch);
      selected ??= project ?? boards[0]?.id ?? null;
      if (selected) {
        board = await coordination.project(selected);
        jobs = await coordination.jobs(selected);
      }
      error = null;
    } catch (e) { if (alive) error = errMsg(e); }
  }
  onMount(async () => {
    try { me = (await coordination.me()).actor; } catch { /* shown by refresh */ }
    await refresh();
    timer = setInterval(() => void refresh(), 5000);
  });
  onDestroy(() => { alive = false; clearInterval(timer); });

  async function run(command: Command) {
    if (!board || busy) return;
    busy = true; error = null;
    try { await coordination.command(board, command); await refresh(); }
    catch (e) { error = errMsg(e); await refresh(); }
    finally { busy = false; }
  }
  const isOwner = $derived(board !== null && board.owner === me);
  const goals = $derived(board ? Object.values(board.goals) : []);
  const facts = $derived(board ? Object.values(board.facts) : []);
  const openAttempts = $derived(board ? Object.values(board.attempts).filter((a) => !a.outcome) : []);
  const reserved = $derived(openAttempts.reduce((sum, a) => sum + a.reserved, 0));
  const certifiedRuns = $derived(board ? Object.values(board.runs).filter((r) => r.status === "certified") : []);
  const onRoute = $derived(new Set(certifiedRuns.flatMap((r) => r.route.steps.map((s) => s.certificate_id))));
  const name = (goal: string) => (board ? goalName(board, goal) : goal);
  const failures = $derived(jobs.filter((j) => j.status === "failed").slice(0, 5));
  const statusOf = (goal: string) => board?.goals[goal]?.status ?? "insufficient";
</script>

<dialog class="modal-dialog collab" aria-label="Collaboration" use:modal oncancel={(e) => { e.preventDefault(); onClose(); }}>
  <header class="dialog-head">
    <h2>Collaboration</h2>
    {#if boards.length > 1}
      <select aria-label="Proof request" bind:value={selected} onchange={() => void refresh()}>
        {#each boards as b}<option value={b.id}>{b.title}</option>{/each}
      </select>
    {/if}
    <button class="close" onclick={onClose} aria-label="Close">×</button>
  </header>
  <div class="tabs" role="tablist" aria-label="Collaboration view">
    {#each [["overview", "Proof overview"], ["tasks", "Task board"], ["decompositions", "Decompositions"], ["review", "Review"]] as [key, label]}
      <button role="tab" aria-selected={tab === key} onclick={() => (tab = key as Tab)}>{label}</button>
    {/each}
  </div>
  <div class="body">
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if !board}
      <p class="muted">{boards.length ? "Loading..." : "No proof request on this branch yet. Open a Lean node and use “Submit to LeanGround”."}</p>
    {:else if tab === "overview"}
      <section aria-label="Proof overview">
        <h3>{board.title} <span class="status" data-status={board.root ? statusOf(board.root) : "insufficient"}>{board.root ? statusOf(board.root) : "binding"}</span></h3>
        {#if board.request?.stale}<p class="warn" role="status">The document changed after this request was bound: its results stay in history but cannot certify the new text. Submit the root node again to start a new request.</p>{/if}
        {#if board.sync_error}<p class="error">Sync: {board.sync_error}</p>{/if}
        {#if !board.ready && !board.sync_error}<p class="muted">Converting nodes and submitting them to LeanGround...</p>{/if}
        <dl>
          <dt>Owner</dt><dd>{board.owner}</dd>
          <dt>Members</dt><dd>{board.members.join(", ")}</dd>
          <dt>Budget</dt><dd>{board.spent} spent · {reserved} reserved · {board.budget} total</dd>
          <dt>Trust</dt><dd>{board.minimum_trust}</dd>
          <dt>Last sync</dt><dd>{board.last_sync ? new Date(board.last_sync).toLocaleTimeString() : "never"}</dd>
        </dl>
        <table>
          <thead><tr><th>Node</th><th>Role</th><th>State</th><th>Status</th><th></th></tr></thead>
          <tbody>
            {#each Object.values(board.nodes ?? {}) as n (n.node)}
              <tr>
                <td><button class="link" onclick={() => onOpenNode?.(n.node)}>{n.title}</button></td>
                <td>{n.role}</td>
                <td title={n.details ?? ""}>{n.state}{n.reason ? ` · ${n.reason}` : ""}</td>
                <td>{#if n.goal}<span class="status" data-status={statusOf(n.goal)}>{statusOf(n.goal)}</span>{/if}</td>
                <td>{#if n.role === "theorem" && n.state !== "open"}<button disabled={busy || !board.ready} onclick={() => void run({ type: "submit_node", node: n.node })}>Resubmit</button>{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if failures.length}
          <h4>Recent failures</h4>
          <ul class="failures">{#each failures as j}<li><code>{j.kind}</code> {j.error}</li>{/each}</ul>
        {/if}
        <button disabled={busy} onclick={() => void run({ type: "sync" })}>Sync now</button>
      </section>
    {:else if tab === "tasks"}
      <section aria-label="Task board">
        <div class="controls">
          <label>Allocation <input type="number" min="0" bind:value={allocation} /></label>
          <label>Lease (s) <input type="number" min="30" max="3600" bind:value={ttl} /></label>
        </div>
        <table>
          <thead><tr><th>Goal</th><th>Kind</th><th>Status</th><th>Lease</th><th></th></tr></thead>
          <tbody>
            {#each Object.values(board.tasks).filter((t) => t.state !== "cancelled") as t (t.id)}
              {@const a = t.attempt ? board.attempts[t.attempt] : undefined}
              <tr>
                <td>{name(t.goal)}</td>
                <td>{t.kind}</td>
                <td><span class="status" data-status={statusOf(t.goal)}>{statusOf(t.goal)}</span></td>
                <td>{#if a}{a.actor} · {Math.max(0, Math.round((a.expires - now()) / 1000))}s{:else}ready{/if}</td>
                <td>
                  {#if !a && statusOf(t.goal) !== "certified"}
                    <button disabled={busy} onclick={() => void run({ type: "claim", task: t.id, allocation, ttl })}>Claim</button>
                  {:else if a && a.actor === me}
                    <button disabled={busy} onclick={() => void run({ type: "heartbeat", attempt: a.id, epoch: a.epoch, ttl })}>Extend</button>
                    <button disabled={busy} onclick={() => void run({ type: "finish", attempt: a.id, epoch: a.epoch, spent: 0, outcome: "released" })}>Release</button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if isOwner}
          <p class="muted">Add a decomposition task for a goal:</p>
          <div class="controls">
            {#each goals.filter((g) => g.status !== "certified").slice(0, 20) as g (g.key)}
              <button disabled={busy} onclick={() => void run({ type: "task", goal: g.key, kind: "decompose", priority: 0 })}>Decompose {name(g.key)}</button>
            {/each}
          </div>
        {/if}
      </section>
    {:else if tab === "decompositions"}
      <section aria-label="Decompositions">
        <p class="muted">A decomposition is a conditional certificate: its goal follows from its premises. Paused ones stay as facts but are never chosen for assembly.</p>
        {#each goals.filter((g) => facts.some((f) => f.goal === g.key)) as g (g.key)}
          <h4>{name(g.key)} <span class="status" data-status={g.status}>{g.status}</span></h4>
          <ul class="facts">
            {#each facts.filter((f) => f.goal === g.key) as f (f.id)}
              <li class:paused={f.state !== "active"} class:unavailable={!f.available}>
                <code title={f.id}>{f.id.slice(0, 10)}</code>
                {#if f.premises.length}⇐ {f.premises.map(name).join(", ")}{:else}complete proof{/if}
                · {f.trust} · {f.state}{f.available ? "" : " · no longer shared"}
                {#if f.premises.includes(f.goal)}<span class="warn">cycle</span>{/if}
                {#if onRoute.has(f.id)}<span class="route">on route</span>{/if}
                {#if isOwner}
                  {#each ["active", "paused", "retired"] as const as state}
                    {#if state !== f.state}<button disabled={busy} onclick={() => void run({ type: "decomposition", certificate: f.id, state })}>{state === "active" ? "Activate" : state === "paused" ? "Pause" : "Retire"}</button>{/if}
                  {/each}
                {/if}
              </li>
            {/each}
          </ul>
        {/each}
      </section>
    {:else}
      <section aria-label="Review">
        {#if isOwner}
          <button disabled={busy || !board.root || statusOf(board.root) === "insufficient"} onclick={() => void run({ type: "assemble", component: "lean-worker" })}>Assemble a route</button>
        {/if}
        {#each Object.values(board.runs) as r (r.id)}
          <article class="run">
            <h4>Assembly {r.id.slice(0, 8)} · <span class="status" data-status={r.status === "certified" ? "certified" : "insufficient"}>{r.status}</span></h4>
            {#if r.failure}<p class="error">{r.failure}{r.retryable ? " (retryable)" : ""}</p>{/if}
            <ol>{#each r.route.steps as s}<li>{name(board.facts[s.certificate_id]?.goal ?? "")} <code>{s.certificate_id.slice(0, 10)}</code></li>{/each}</ol>
            {#if isOwner && r.status === "failed" && r.retryable}<button disabled={busy} onclick={() => void run({ type: "retry", run: r.id })}>Retry same plan</button>{/if}
            {#if isOwner && r.status === "certified"}
              <button disabled={busy || board.request?.stale} onclick={() => void run({ type: "accept", run: r.id })} title="Write the certified sources back to the document in one batch">Accept and write back</button>
            {/if}
          </article>
        {/each}
        {#each Object.values(board.writebacks ?? {}) as w (w.batch_id)}
          <article class="batch" data-status={w.status}>
            <h4>Writeback {w.batch_id.slice(3, 11)} · {w.status}</h4>
            {#if w.status === "aborted"}<p class="error">{w.reason}: {w.details}. Nothing was written.</p>{/if}
            {#if w.contract}
              <ul>{#each w.contract.operations as o}<li>{o.kind.replaceAll("_", " ")} <button class="link" onclick={() => onOpenNode?.(o.node_id)}>{board.nodes?.[o.node_id]?.title ?? o.node_id.slice(0, 8)}</button></li>{/each}</ul>
            {/if}
          </article>
        {/each}
      </section>
    {/if}
  </div>
</dialog>

<style>
  .collab { width:min(980px,95vw); max-height:92vh; overflow:auto; }
  .dialog-head { display:flex; align-items:center; gap:.75rem; }
  .dialog-head h2 { flex:1; }
  .close { border:none; background:none; font-size:1.3rem; cursor:pointer; color:var(--mdc-muted); }
  .tabs { display:flex; gap:.4rem; padding:.75rem 1rem 0; }
  .tabs button { padding:.4rem .9rem; border:1px solid var(--mdc-border); border-radius:var(--mdc-radius-sm); background:var(--mdc-bg); color:var(--mdc-muted); cursor:pointer; }
  .tabs button[aria-selected="true"] { color:var(--mdc-accent); border-color:var(--mdc-accent); background:var(--mdc-accent-soft); }
  .body { padding:1rem; font-size:.85rem; }
  table { width:100%; border-collapse:collapse; margin:.75rem 0; }
  th, td { text-align:left; padding:.3rem .4rem; border-bottom:1px solid var(--mdc-border); }
  dl { display:grid; grid-template-columns:max-content 1fr; gap:.2rem .8rem; }
  dt { color:var(--mdc-muted); }
  .status { padding:0 .35rem; border-radius:3px; font-size:.75rem; background:var(--mdc-bg); border:1px solid var(--mdc-border); }
  .status[data-status="certified"] { color:var(--mdc-accent-down); border-color:var(--mdc-accent-down); }
  .status[data-status="derivable"] { color:var(--mdc-accent); border-color:var(--mdc-accent); }
  .muted { color:var(--mdc-muted); }
  .warn { color:var(--mdc-warning); }
  .error { color:var(--mdc-error); }
  .route { color:var(--mdc-accent); margin-left:.3rem; }
  .facts li.paused { opacity:.6; }
  .facts li.unavailable { text-decoration:line-through; }
  .controls { display:flex; flex-wrap:wrap; gap:.5rem; align-items:center; }
  .controls input { width:6rem; }
  .link { border:none; background:none; color:var(--mdc-accent); cursor:pointer; padding:0; }
  .run, .batch { border:1px solid var(--mdc-border); border-radius:var(--mdc-radius-sm); padding:.5rem .75rem; margin:.6rem 0; }
  .batch[data-status="aborted"] { border-color:var(--mdc-error); }
  button { cursor:pointer; }
</style>
