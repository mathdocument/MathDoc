<script lang="ts">
  // The project: this branch's document nodes in LeanGround, the database's branches, failed
  // background jobs, and the board's owner, members, budget, trust and review policy.
  import type { Collab } from "../../lib/collab.svelte";
  import { goalStatusHelp, nodeStateHelp, routeCloses, type Board, type Goal } from "../../lib/coordination";
  import GoalStatus from "./GoalStatus.svelte";
  let { c, onOpenNode }: { c: Collab; onOpenNode: (fnode: string) => void } = $props();
  const reserved = $derived(Object.values(c.board?.attempts ?? {}).filter((a) => !a.outcome).reduce((s, a) => s + a.reserved, 0));
  const branchState = (s: NonNullable<Board["branches"]>[string]) =>
    s.request.bound ? (s.request.stale ? "changed since converted" : "converted") : s.request.error ?? "converting…";
</script>

{#if c.board}
  {@const b = c.board}
  <section aria-label="Project">
    <div class="section-head"><h2>Project</h2>
      <button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "sync" })} title="Read this database's branches again and fetch new certificates from LeanGround">Sync now</button>
    </div>
    {#if c.frozen}<p class="warn" role="status">{c.frozen}</p>{/if}
    {#if c.view?.request.stale}<p class="warn" role="status">This branch's document changed after it was converted: its results stay in history but cannot certify the new text. Submit the root node again to convert the branch anew.</p>{/if}
    {#if c.view?.request.error}<p class="error">This branch: {c.view.request.error}</p>{/if}
    {#if b.sync_error}<p class="error">Sync: {b.sync_error}</p>{/if}
    {#if b.project && c.branch && !c.view && !c.frozen}
      <p class="muted">This branch is not attached to the project yet.</p>
      <button disabled={c.busy || !b.ready} onclick={() => void c.run({ type: "attach", branch: c.branch!.branch })}>Attach this branch</button>
    {/if}
    {#if !b.ready && !b.sync_error}<p class="muted">Converting nodes and submitting them to LeanGround…</p>{/if}

    {#if Object.keys(c.view?.nodes ?? {}).length}
      <p class="group">Document nodes on this branch</p>
      <table aria-label="Document nodes">
        <thead><tr><th>Node</th><th>Kind</th><th title="Where the node is in LeanGround">Submission</th><th title="Whether the goal the node states is proved">Goal</th><th></th></tr></thead>
        <tbody>
          {#each Object.values(c.view?.nodes ?? {}) as n (n.node)}
            <tr>
              <td><button class="link" onclick={() => onOpenNode(n.node)}>{n.title}</button></td>
              <td>{n.role}</td>
              <td title={[nodeStateHelp[n.state], n.details].filter(Boolean).join("\n")}>{n.state}{n.reason ? ` · ${n.reason}` : ""}</td>
              <td>{#if n.goal}<GoalStatus status={c.statusOf(n.goal)} />{/if}</td>
              <td>{#if n.role === "theorem" && !n.file && b.mode !== "modules" && !c.frozen}<button title="Read this node again from the branch and submit its current proof" disabled={c.busy || !b.ready} onclick={() => void c.run({ type: "submit_node", node: n.node, ...c.here })}>Resubmit</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <details>
        <summary>What these words mean</summary>
        <p class="group">Submission: where the node is in LeanGround</p>
        <dl>{#each Object.entries(nodeStateHelp) as [state, help] (state)}<dt>{state}</dt><dd>{help}</dd>{/each}</dl>
        <p class="group">Goal: whether the statement is proved, by any certificate</p>
        <dl>{#each Object.entries(goalStatusHelp) as [status, help] (status)}<dt><GoalStatus status={status as Goal["status"]} /></dt><dd>{help}</dd>{/each}</dl>
      </details>
    {/if}

    {#if Object.keys(b.branches ?? {}).length > 1}
      <p class="group">Branches: each proves the root its own way</p>
      <table aria-label="Branches">
        <thead><tr><th>Branch</th><th>Document</th><th title="Goals for which the branch chose the certificate to use">Picks</th><th title="Whether every goal on the branch's route has a proof">Route</th></tr></thead>
        <tbody>
          {#each Object.entries(b.branches ?? {}) as [name, s] (name)}
            <tr>
              <td>{name}{name === c.branch?.branch ? " (this branch)" : ""}</td>
              <td>{branchState(s)}</td>
              <td>{Object.keys(s.picks).length}</td>
              <td>{s.request.bound && b.root ? (routeCloses(b, s.picks) ? "complete: can be assembled" : "goals still open") : ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    {#if c.failures.length}
      <p class="group">Recent failures: background jobs that failed, with LeanGround's explanation</p>
      <ul>{#each c.failures as j (j.id)}<li><code>{j.kind}</code> {j.error}{#if j.details}<pre class="messages">{j.details}</pre>{/if}</li>{/each}</ul>
    {/if}

    <p class="group">Settings</p>
    <dl>
      <dt>Owner</dt><dd>{b.owner}: reviews decompositions, assembles and writes back</dd>
      <dt>Members</dt><dd>{b.members.join(", ")}: can claim tasks and submit proofs</dd>
      <dt>Budget</dt><dd>{b.spent} spent · {reserved} reserved by open claims · {b.budget} in total</dd>
      <dt>Trust</dt><dd>{b.minimum_trust}: certificates LeanGround trusts less than this do not count</dd>
      <dt>Mode</dt><dd>{b.mode === "modules" ? "modules: every Lean node is a whole file; a proof is a set of files (mdc pull writes a directory)" : "declarations: Lean nodes are converted declaration by declaration"}</dd>
      {#if b.project}
        <dt>Review</dt>
        <dd>
          {b.review === "none" ? "new decompositions count at once" : "new decompositions from agents wait for the owner's review"}
          {#if c.isOwner && !c.frozen}
            <button disabled={c.busy} onclick={() => void c.run({ type: "policy", review: b.review === "none" ? "required" : "none" })}>{b.review === "none" ? "Require review" : "Waive review"}</button>
          {/if}
        </dd>
      {/if}
      <dt>Last sync</dt><dd>{b.last_sync ? new Date(b.last_sync).toLocaleTimeString() : "never"}</dd>
    </dl>
  </section>
{/if}
