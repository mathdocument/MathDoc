<script lang="ts">
  // The inspector: the selected goal, in three tabs (overview with statement and task, notes,
  // certificates), or a certificate (what it proves and from what, its source, review and
  // route), with their commands.
  import type { Collab, InspectorTab, Selection } from "../../lib/collab.svelte";
  import { actorName, when } from "../../lib/collab.svelte";
  import { goalStatementBlock, goalStatusHelp, timeLeft } from "../../lib/coordination";
  import MarkdownEditor from "../MarkdownEditor.svelte";
  import MarkdownPreview from "../MarkdownPreview.svelte";
  import CertificateSource from "./CertificateSource.svelte";
  import GoalNotes from "./GoalNotes.svelte";
  import GoalStatus from "./GoalStatus.svelte";
  let { c, selection, onSelect, tab = $bindable("overview") }: {
    c: Collab; selection: Selection; onSelect: (s: Selection) => void; tab?: InspectorTab;
  } = $props();
  let allocation = $state(100), ttl = $state(300);
  let comment = $state("");
  const kindHelp: Record<string, string> = {
    prove: "Prove the goal outright (mdc prove), or split it into smaller goals (mdc split)",
    decompose: "Split the goal into smaller goals (mdc split)",
    formalize: "State the goal in Lean",
  };
  const goal = $derived(selection?.kind === "goal" && c.board ? c.board.goals[selection.goal] ?? null : null);
  const fact = $derived(selection?.kind === "fact" && c.board ? c.board.facts[selection.fact] ?? null : null);
  const tasks = $derived(goal && c.board ? Object.values(c.board.tasks).filter((t) => t.goal === goal.key && t.state !== "cancelled") : []);
  const certificates = $derived(goal ? c.facts.filter((f) => f.goal === goal.key) : []);
  const latest = $derived(goal?.notes?.at(-1) ?? null);
  const certKind = (premises: number) => (premises ? `decomposition · ${premises} ${premises === 1 ? "premise" : "premises"}` : "complete proof");
  async function decide(state: "active" | "retired") {
    if (!fact) return;
    await c.review(fact.id, state, comment);
    if (!c.refused) comment = "";
  }
</script>

<aside class="inspector" aria-label="Inspector">
  {#if goal}
    {@const status = c.statusOf(goal.key)}
    {@const tabs = [
      { key: "overview" as const, label: "Overview" },
      { key: "notes" as const, label: `Notes ${goal.notes?.length ?? 0}` },
      { key: "certificates" as const, label: `Certificates ${certificates.length}` },
    ]}
    <p class="eyebrow-label">Goal</p>
    <h3>{c.name(goal.key)}</h3>
    <p class="status-line"><GoalStatus {status} /> <span class="muted">{goalStatusHelp[status]}</span></p>
    {#if goal.refuted}<p class="error">Refuted: a complete proof of its negation exists.</p>{/if}
    <div class="tabs" role="tablist" aria-label="Goal">
      {#each tabs as t (t.key)}
        <button role="tab" aria-selected={tab === t.key} onclick={() => (tab = t.key)}>{t.label}</button>
      {/each}
    </div>

    {#if tab === "overview"}
      <section class="facet" aria-label="Statement">
        <p class="group">Statement</p>
        <pre class="lean">{goalStatementBlock(c.board!, goal.key)}</pre>
        {#if c.needing(goal.key).length}<p class="muted">Needed by {c.needing(goal.key).join(", ")}</p>{/if}
      </section>

      {#each tasks as t (t.id)}
        {@const a = t.attempt ? c.board!.attempts[t.attempt] : undefined}
        <section class="facet" aria-label="Task">
          <p class="group">Task</p>
          <dl class="facts">
            <dt>Kind</dt><dd title={kindHelp[t.kind] ?? ""}>{t.kind}{#if c.ranking.has(t.id)} <span class="muted">· #{c.ranking.get(t.id)} in the recommended order</span>{/if}</dd>
            {#if a}
              <dt>Claimed by</dt><dd>{actorName(a.actor)}</dd>
              <dt>Lease</dt><dd>{timeLeft(a.expires)}</dd>
            {/if}
          </dl>
          {#if a}
            {#if a.actor === c.me}
              <div class="controls">
                <button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "heartbeat", attempt: a.id, epoch: a.epoch, ttl })}>Extend</button>
                <button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "finish", attempt: a.id, epoch: a.epoch, spent: 0, outcome: "released" })}>Release</button>
              </div>
            {:else if a.actor.split("/")[0] === c.me || c.isOwner}
              <!-- A lease a stopped session left: its token's person, or the owner, frees the task. -->
              <div class="controls">
                <button title="Free the task now instead of waiting for the lease to expire; the holder's work is kept" disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "finish", attempt: a.id, epoch: a.epoch, spent: 0, outcome: "released" })}>Release</button>
              </div>
            {/if}
          {:else if c.solved.has(t.goal)}
            <p class="muted">Done: active certificates prove this goal.</p>
          {:else if !c.active.has(t.goal)}
            <p class="muted">Not claimable yet: no active decomposition leads to this goal (one may be awaiting review).</p>
          {:else if !c.frozen}
            <div class="controls">
              <label title="Budget units a claim reserves until it finishes">Reserve <input aria-label="Allocation" type="number" min="0" bind:value={allocation} /></label>
              <label title="A claim not extended within this time expires">for <input aria-label="Lease (s)" type="number" min="30" max="3600" bind:value={ttl} /> s</label>
              <button disabled={c.busy} onclick={() => void c.run({ type: "claim", task: t.id, allocation, ttl })}>Claim</button>
            </div>
          {/if}
        </section>
      {/each}

      <section class="facet" aria-label="Latest note">
        <p class="group">Latest note</p>
        {#if latest}
          <p class="note-meta"><strong>{actorName(latest.actor)}</strong> <span class="muted">· {when(latest.at)}{latest.edited ? " · edited" : ""}</span></p>
          <div class="note-body clamp"><MarkdownPreview compact source={latest.text} /></div>
        {:else}
          <p class="muted">None yet.</p>
        {/if}
        <button class="link" onclick={() => (tab = "notes")}>{goal.notes?.length ? "All notes →" : "Add a note →"}</button>
      </section>
    {:else if tab === "notes"}
      {#key goal.key}<GoalNotes {c} goal={goal.key} />{/key}
    {:else}
      <section class="facet" aria-label="Certificates">
        {#each certificates as f (f.id)}
          <div class="cert-item">
            <p class="cert-line">
              <button class="link" onclick={() => onSelect({ kind: "fact", fact: f.id })}>{certKind(f.premises.length)}</button>
              {#if f.state !== "active"}<span class="status">{f.state}</span>{/if}
              {#if c.onRoute.has(f.id)}<span class="route">on {c.branch?.branch}'s route</span>{/if}
            </p>
            <p class="muted">{c.origin(f)}</p>
          </div>
        {:else}
          <p class="muted">No certificate yet: a complete proof or a decomposition of this goal appears here.</p>
        {/each}
      </section>
    {/if}
  {:else if fact}
    <button class="link back" onclick={() => { tab = "certificates"; onSelect({ kind: "goal", goal: fact.goal }); }}>← {c.name(fact.goal)}</button>
    <p class="eyebrow-label">{fact.premises.length ? "Decomposition" : "Complete proof"} · <code>{fact.id.slice(0, 10)}</code></p>
    <h3>{c.name(fact.goal)}{fact.premises.length ? " ⇐" : ""}</h3>
    <p class="muted">
      {c.origin(fact)}{fact.state !== "active" ? ` · ${fact.state}` : ""}{fact.trust !== "trusted" ? ` · trust ${fact.trust}` : ""}
      {#if !fact.available}<span class="warn"> · no longer shared</span>{/if}
    </p>
    {#if fact.review}
      <p class="muted">{fact.review.state} by {actorName(fact.review.actor)}{fact.review.at ? ` · ${when(fact.review.at)}` : ""}</p>
      {#if fact.review.comment}<div class="description"><MarkdownPreview compact source={fact.review.comment} /></div>{/if}
    {/if}
    {#if c.onRoute.has(fact.id) || c.pickedBy(fact.id).length}
      <p>{#if c.onRoute.has(fact.id)}<span class="route">on {c.branch?.branch}'s route</span>{/if} {#each c.pickedBy(fact.id) as b (b)}<span class="route">picked by {b} </span>{/each}</p>
    {/if}
    {#if fact.refuted_by}<p class="warn">Its premise {c.name(fact.refuted_by)} is false.</p>{/if}
    {#if fact.description}<div class="description"><MarkdownPreview compact source={fact.description} /></div>{/if}
    <p class="group">Proves</p>
    <p><button class="link" onclick={() => onSelect({ kind: "goal", goal: fact.goal })}>{c.name(fact.goal)}</button></p>
    {#if fact.premises.length}
      <p class="group">From the premises</p>
      {#each fact.premises as p (p)}
        <p><button class="link" onclick={() => onSelect({ kind: "goal", goal: p })}>{c.name(p)}</button> <GoalStatus status={c.statusOf(p)} /></p>
        <pre class="lean">{goalStatementBlock(c.board!, p)}</pre>
      {/each}
    {/if}
    <CertificateSource {c} cert={fact.id} />
    {#if fact.state === "proposed"}
      {#if c.isOwner && !c.frozen}
        <p class="group">Your review</p>
        {#key fact.id}
          <MarkdownEditor label={`Review comment on ${fact.id.slice(0, 10)}`} placeholder="Review comment (optional)" maxlength={2000} bind:value={comment}>
            {#snippet actions()}
              <button disabled={c.busy} onclick={() => void decide("retired")}>Retire</button>
              <button class="primary" disabled={c.busy || comment.trim().length > 2000} onclick={() => void decide("active")}>Activate</button>
            {/snippet}
          </MarkdownEditor>
        {/key}
      {:else}
        <p class="muted">Waiting for the owner's review.</p>
      {/if}
    {:else}
      <div class="controls" style="margin-top: .8rem">
        {#if c.canPick && c.view && c.board?.project && c.branch}
          {#if c.view.picks[fact.goal] === fact.id}
            <button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "pick", branch: c.branch!.branch, goal: fact.goal, certificate: null })} title="Let assembly choose this goal's certificate again">Unpick</button>
          {:else if fact.state === "active"}
            <button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "pick", branch: c.branch!.branch, goal: fact.goal, certificate: fact.id })} title="This branch's route proves the goal with this certificate">Use on this branch</button>
          {/if}
        {/if}
        {#if c.isOwner && fact.state !== "refuted"}
          {#each ["active", "paused", "retired"] as const as state (state)}
            {#if state !== fact.state}<button disabled={c.busy || !!c.frozen} onclick={() => void c.run({ type: "decomposition", certificate: fact.id, state })} title={state === "active" ? "Use it in routes and assembly again" : state === "paused" ? "Leave it out of routes and assembly until it is activated again" : "Withdraw it for good"}>{state === "active" ? "Activate" : state === "paused" ? "Pause" : "Retire"}</button>{/if}
          {/each}
        {/if}
      </div>
    {/if}
  {:else}
    <p class="muted">Select a goal or a certificate to see it here.</p>
  {/if}
</aside>
