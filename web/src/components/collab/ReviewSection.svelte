<script lang="ts">
  // Decompositions awaiting the owner's review (§2.7.3): what each claims, its premises with
  // their statements, the definitions it uses, and the owner's decision. Then what was decided:
  // the owner's reviews, and the decompositions that counted at once while review was off.
  import type { Collab, Selection } from "../../lib/collab.svelte";
  import { actorName, when } from "../../lib/collab.svelte";
  import { goalStatement } from "../../lib/coordination";
  import MarkdownEditor from "../MarkdownEditor.svelte";
  import MarkdownPreview from "../MarkdownPreview.svelte";
  import CertificateSource from "./CertificateSource.svelte";
  import GoalStatus from "./GoalStatus.svelte";
  let { c, onSelect }: { c: Collab; onSelect: (s: Selection) => void } = $props();
  let comments = $state<Record<string, string>>({});
  const verb: Record<string, string> = { active: "Activated", paused: "Paused", retired: "Retired", refuted: "Refuted" };
  // Newest first. A decomposition an actor submitted with no review counted at once: review was off.
  const decided = $derived(
    c.facts
      .flatMap((f) =>
        f.review ? [{ f, at: f.review.at, by: f.review.actor, what: verb[f.review.state] ?? f.review.state, comment: f.review.comment }]
        : f.premises.length && f.state !== "proposed" && c.submitted(f) ? [{ f, at: f.submitted_at ?? 0, by: f.submitter!, what: "Counted at once, unreviewed", comment: undefined }]
        : [])
      .sort((a, b) => b.at - a.at),
  );
</script>

<section aria-label="Review">
  <div class="section-head"><h2>Review</h2></div>
  {#if c.board?.review === "none"}
    <p class="card notice" role="status">
      <strong>Review is off.</strong> Agents' new decompositions count at once and never wait here; they are listed under Decided below.
      {#if c.isOwner && !c.frozen}<button disabled={c.busy} onclick={() => void c.run({ type: "policy", review: "required" })}>Require review</button>{/if}
    </p>
  {/if}
  {#if !c.proposed.length}
    <p class="muted">No decomposition waits for review.</p>
  {:else}
    <section aria-label="Pending review">
      {#each c.proposed as f (f.id)}
        {@const needing = c.needing(f.goal)}
        <article class="card attention">
          <p class="eyebrow-label">Decomposition · <code>{f.id.slice(0, 10)}</code></p>
          <h3 style="margin: .2rem 0"><button class="link goal-name" onclick={() => onSelect({ kind: "goal", goal: f.goal })}>{c.name(f.goal)}</button> ⇐ {f.premises.length} {f.premises.length === 1 ? "premise" : "premises"}</h3>
          <p class="muted">
            {f.submitter ? `Submitted by ${f.submitter}` : "Found by sync"}{f.submitted_at ? ` · ${when(f.submitted_at)}` : ""}
            · {needing.length ? `needed by ${needing.length === 1 ? "branch" : "branches"} ${needing.join(", ")}` : "no branch route reaches its goal"}
          </p>
          {#if f.description}<div class="description"><MarkdownPreview compact source={f.description} /></div>{/if}
          <p class="group">Premises: they become goals once activated</p>
          {#each f.premises as p (p)}
            <p style="margin: .3rem 0 0"><button class="link goal-name" onclick={() => onSelect({ kind: "goal", goal: p })}>{c.name(p)}</button> <GoalStatus status={c.statusOf(p)} /></p>
            <pre class="lean">{goalStatement(c.board!, p)}</pre>
          {/each}
          <CertificateSource {c} cert={f.id} />
          {#if c.isOwner && !c.frozen}
            <p class="group">Your review</p>
            <MarkdownEditor label={`Review comment on ${f.id.slice(0, 10)}`} placeholder="Review comment (optional)" maxlength={2000} bind:value={() => comments[f.id] ?? "", (v) => (comments[f.id] = v)}>
              {#snippet actions()}
                <button disabled={c.busy} onclick={() => void c.review(f.id, "retired", comments[f.id])}>Retire</button>
                <button class="primary" disabled={c.busy || (comments[f.id] ?? "").trim().length > 2000} onclick={() => void c.review(f.id, "active", comments[f.id])}>Activate</button>
              {/snippet}
            </MarkdownEditor>
          {:else}
            <p class="muted">Waiting for the owner's review.</p>
          {/if}
        </article>
      {/each}
    </section>
  {/if}
  {#if decided.length}
    <section aria-label="Decided">
      <p class="group">Decided ({decided.length}), newest first</p>
      <ul class="decided">
        {#each decided as d (d.f.id)}
          <li>
            <p>
              <button class="link goal-name" onclick={() => onSelect({ kind: "fact", fact: d.f.id })}>{c.name(d.f.goal)} ⇐ {d.f.premises.length} {d.f.premises.length === 1 ? "premise" : "premises"}</button>
              <span class="status" data-status={d.what === "Activated" ? "certified" : d.what.startsWith("Counted") ? "insufficient" : "refuted"}>{d.what}</span>
              <span class="muted">{d.what.startsWith("Counted") ? `submitted by ${actorName(d.by)}` : `by ${actorName(d.by)}`}{d.at ? ` · ${when(d.at)}` : ""}</span>
            </p>
            {#if d.comment}<div class="description"><MarkdownPreview compact source={d.comment} /></div>{/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}
</section>
