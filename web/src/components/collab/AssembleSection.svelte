<script lang="ts">
  // Assembly and writeback (§4.9): assemble this branch's route into one Lean file LeanGround
  // checks without premises, preview what accepting writes, accept it into the branch.
  import type { Collab } from "../../lib/collab.svelte";
  import { writtenBack, type PlannedChange } from "../../lib/coordination";
  import { lineDiff } from "../../lib/diff";
  let { c, onOpenNode }: { c: Collab; onOpenNode: (fnode: string) => void } = $props();
  const changeLabel: Record<PlannedChange["kind"], string> = {
    update_lean_block: "Update the Lean block of",
    create_goal_node: "Create the goal node",
    create_definition_node: "Create the definition node",
  };
  const opLabel = (kind: string) =>
    ({ update_lean_block: "updated the Lean block of", create_goal_node: "created the goal node", create_definition_node: "created the definition node" })[kind] ?? kind.replaceAll("_", " ");
  const kindOf = (cert: string) => {
    const f = c.board?.facts[cert];
    return f ? c.kindOf(f) : "";
  };
  const orphans = $derived(Object.values(c.view?.writebacks ?? {}).filter((w) => !Object.hasOwn(c.board?.runs ?? {}, w.run)));
</script>

<section aria-label="Assemble and write back">
  <div class="section-head"><h2>Assemble and write back</h2></div>
  <p class="muted">
    Assembling joins the certificates on {c.board?.project ? `${c.branch?.branch}'s route` : "the route"} (one per goal) into a single Lean file, which LeanGround checks with no premises left.
    A certified assembly can then be written back: <strong>Preview changes</strong> shows what it writes, and <strong>Accept and write back</strong> writes it into {c.board?.project ? `branch ${c.branch?.branch}` : "the document"} in one commit.
  </p>
  {#if c.isOwner}
    <div class="controls">
      <button class="primary" disabled={c.busy || !!c.frozen || !!c.assembleBlocked} onclick={() => void c.run({ type: "assemble", component: "lean-worker", ...c.here })}>Assemble a route</button>
      {#if c.assembleBlocked && !c.frozen}<span class="muted">{c.assembleBlocked}</span>{/if}
    </div>
  {:else}
    <p class="muted">Only the owner, {c.board?.owner}, assembles and writes back.</p>
  {/if}
  {#each c.runs as r (r.id)}
    {@const target = r.branch ? c.board?.branches?.[r.branch] : c.view}
    {@const batch = writtenBack(c.board!, r)}
    {@const records = Object.values((r.branch ? c.board?.branches?.[r.branch]?.writebacks : c.view?.writebacks) ?? {}).filter((w) => w.run === r.id)}
    {@const p = c.previews[r.id]}
    <article class="card run" data-status={batch ? "written" : r.status} style="margin-top: .9rem">
      <h3 style="margin: 0 0 .3rem; font-size: 1rem">Assembly {r.id.slice(0, 8)}{r.branch ? ` · ${r.branch}` : ""} · <span class="status" data-status={r.status === "certified" ? "certified" : "insufficient"}>{r.status}</span>{#if batch}{" · written back"}{/if}</h3>
      {#if r.failure}<p class="error">{r.failure}{r.retryable ? " (retryable)" : ""}</p>{/if}
      {#if r.details}<pre class="messages">{r.details}</pre>{/if}
      <details open={!batch}>
        <summary>Route: {r.route.steps.length} {r.route.steps.length === 1 ? "certificate" : "certificates"}</summary>
        <ol>{#each r.route.steps as s (s.certificate_id)}<li>{c.name(c.board?.facts[s.certificate_id]?.goal ?? "")}: {kindOf(s.certificate_id)} <code class="muted" title={s.certificate_id}>{s.certificate_id.slice(0, 10)}</code></li>{/each}</ol>
      </details>
      {#if c.isOwner && r.status === "failed" && r.retryable}<button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "retry", run: r.id })}>Retry same plan</button>{/if}
      {#each records as w (w.batch_id)}
        {#if w.status === "committed"}
          <p style="color: var(--mdc-accent-down)">Written back to {r.branch ? `branch ${r.branch}` : "the document"} (writeback {w.batch_id.slice(3, 11)}): {#each w.contract?.operations ?? [] as o, i (o.node_id)}{i ? "; " : ""}{opLabel(o.kind)} <button class="link" onclick={() => onOpenNode(o.node_id)}>{target?.nodes[o.node_id]?.title ?? o.node_id.slice(0, 8)}</button>{/each}.</p>
        {:else}
          <p class="error">Writeback {w.batch_id.slice(3, 11)} aborted: {w.reason}: {w.details}. Nothing was written.</p>
        {/if}
      {/each}
      {#if !batch && r.status === "certified"}
        <div class="controls">
          {#if c.board?.project}<button onclick={() => void c.togglePreview(r.id)} aria-expanded={!!p}>{p ? "Hide preview" : "Preview changes"}</button>{/if}
          {#if c.isOwner}<button class="primary" disabled={c.busy || !!c.frozen || !!target?.request.stale} onclick={() => void c.run({ type: "accept", run: r.id })} title={`Write the certified sources back to ${r.branch ? `branch ${r.branch}` : "the document"} in one batch`}>Accept and write back</button>{/if}
        </div>
        {#if p}
          <section class="preview" aria-label={`Preview of assembly ${r.id.slice(0, 8)}`}>
            {#if p === "loading"}<p class="muted">Reading the branch and the certified sources…</p>
            {:else if "error" in p}<p class="error">{p.error}</p>
            {:else if p.status === "written"}<p class="muted">Already written back (writeback {p.writeback.batch_id.slice(3, 11)}).</p>
            {:else}
              {#if p.status === "conflict"}
                <p class="error">Accepting would abort: {p.reason}: {p.details}. Nothing would be written.</p>
              {:else}
                <p>Accepting writes {p.changes.length} {p.changes.length === 1 ? "change" : "changes"} into branch {r.branch}, in one commit:</p>
                {#each p.changes as ch (ch.node)}
                  <details class="change" open>
                    <summary>{changeLabel[ch.kind]} {#if ch.before === null}<strong>{ch.title}</strong>{:else}<button class="link" onclick={() => onOpenNode(ch.node)}>{ch.title}</button>{/if}</summary>
                    <pre class="diff">{#each lineDiff(ch.before ?? "", ch.after) as l, i (i)}<span class="line" data-op={l.op}>{l.op} {l.text}</span>{/each}</pre>
                  </details>
                {/each}
              {/if}
              {#if p.assembled}
                <details class="change">
                  <summary>The assembled proof, checked by LeanGround as one file without premises</summary>
                  <pre class="messages">{p.assembled}</pre>
                </details>
              {/if}
            {/if}
          </section>
        {/if}
      {/if}
    </article>
  {/each}
  {#each orphans as w (w.batch_id)}
    <article class="card" data-status={w.status}>
      <h3 style="margin: 0; font-size: 1rem">Writeback {w.batch_id.slice(3, 11)} · {w.status}</h3>
      {#if w.status === "aborted"}<p class="error">{w.reason}: {w.details}. Nothing was written.</p>{/if}
    </article>
  {/each}
</section>

<style>
  .diff { margin: .3rem 0; padding: .4rem 0; max-height: 22rem; overflow: auto; font-family: var(--mdc-mono); font-size: var(--mdc-text-xs); background: var(--mdc-code-bg); color: var(--mdc-code-fg); border-radius: var(--mdc-radius-sm); }
  .diff .line { display: block; padding: 0 .6rem; white-space: pre-wrap; }
  .diff .line[data-op="+"] { background: color-mix(in srgb, var(--mdc-accent-down) 18%, transparent); }
  .diff .line[data-op="-"] { background: color-mix(in srgb, var(--mdc-error) 16%, transparent); }
  .change { margin: .4rem 0; }
</style>
