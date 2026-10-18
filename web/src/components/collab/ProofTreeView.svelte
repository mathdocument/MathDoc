<script lang="ts">
  // The proof tree as an outline: each goal, its certificates, each decomposition's premises.
  // Each goal row shows who holds its task and how many notes it has. Certificates not in use
  // (paused, retired, refuted) fold under one line per goal. A filter keeps the goals whose
  // name matches and which are open, have notes or are claimed, with the path to them.
  // Selecting a row shows it in the inspector.
  import { ChevronDown, ChevronRight, NotebookPen, Search, UserRound } from "@lucide/svelte";
  import type { Collab, Selection } from "../../lib/collab.svelte";
  import { actorName, when } from "../../lib/collab.svelte";
  import { inUse, timeLeft, type TreeFact, type TreeGoal } from "../../lib/coordination";
  import GoalStatus from "./GoalStatus.svelte";
  let { c, selection, onSelect }: { c: Collab; selection: Selection; onSelect: (s: Selection) => void } = $props();
  let collapsed = $state<Record<string, boolean>>({});
  let unfolded = $state<Record<string, boolean>>({});
  let query = $state("");
  type Only = "all" | "open" | "notes" | "claimed";
  let only = $state<Only>("all");
  const ONLY: { key: Only; label: string; title: string }[] = [
    { key: "all", label: "All", title: "Every goal" },
    { key: "open", label: "Open", title: "Goals with no proof yet" },
    { key: "notes", label: "With notes", title: "Goals someone left notes on" },
    { key: "claimed", label: "Claimed", title: "Goals whose task someone holds" },
  ];
  const goalSelected = (goal: string) => selection?.kind === "goal" && selection.goal === goal;
  const factSelected = (fact: string) => selection?.kind === "fact" && selection.fact === fact;
  /** Whether the selection is this certificate or below it, so its fold opens to show it. */
  const holds = (t: TreeFact): boolean =>
    factSelected(t.fact.id) || t.premises.some((g) => !g.repeat && (goalSelected(g.goal) || g.facts.some(holds)));
  const tally = (facts: TreeFact[]) =>
    (["retired", "paused", "refuted"] as const)
      .map((state) => [state, facts.filter((t) => t.fact.state === state).length] as const)
      .filter(([, n]) => n).map(([state, n]) => `${n} ${state}`).join(", ");
  const notesOf = (goal: string) => c.board?.goals[goal]?.notes ?? [];
  /** The live claim on the goal's prove task, if any. */
  const claimOf = (goal: string) => {
    const b = c.board;
    const t = b && Object.values(b.tasks).find((t) => t.goal === goal && t.kind === "prove" && t.attempt);
    const a = t && b.attempts[t.attempt!];
    return a && !a.outcome ? a : undefined;
  };
  const matches = (goal: string) => {
    const name = c.name(goal).toLowerCase();
    if (query.trim() && !name.includes(query.trim().toLowerCase())) return false;
    if (only === "open") return c.statusOf(goal) === "insufficient" && !c.board?.goals[goal]?.refuted;
    if (only === "notes") return notesOf(goal).length > 0;
    if (only === "claimed") return !!claimOf(goal);
    return true;
  };
  const filtering = $derived(only !== "all" || query.trim() !== "");
  // Filtered: which goals and certificates lead to a match (a match keeps its own row).
  const kept = $derived.by(() => {
    const goals = new Set<TreeGoal>(), facts = new Set<TreeFact>();
    if (!filtering) return { goals, facts };
    const visit = (g: TreeGoal): boolean => {
      let hit = matches(g.goal);
      if (!g.repeat) for (const t of g.facts) if (t.premises.map(visit).some(Boolean)) { facts.add(t); hit = true; }
      if (hit) goals.add(g);
      return hit;
    };
    if (c.tree.root) visit(c.tree.root);
    c.tree.others.forEach(visit);
    return { goals, facts };
  });
</script>

{#snippet goalNode(g: TreeGoal, folded = false)}
  {@const closed = !filtering && !!collapsed[g.goal]}
  {@const facts = filtering ? g.facts.filter((t) => kept.facts.has(t)) : g.facts}
  {@const used = filtering ? facts : facts.filter((t) => inUse(t.fact))}
  {@const unused = filtering ? [] : facts.filter((t) => !inUse(t.fact))}
  {@const open = !!unfolded[g.goal] || unused.some(holds)}
  {@const notes = notesOf(g.goal)}
  {@const claim = claimOf(g.goal)}
  <li class="goal-node">
    <div class="tree-row" class:selected={!g.repeat && goalSelected(g.goal)}>
      <button class="twisty" disabled={g.repeat || !facts.length || filtering} aria-label={closed ? `Expand ${c.name(g.goal)}` : `Collapse ${c.name(g.goal)}`} aria-expanded={!closed} onclick={() => (collapsed[g.goal] = !closed)}>
        {#if closed}<ChevronRight size={15} />{:else}<ChevronDown size={15} />{/if}
      </button>
      <button onclick={() => onSelect({ kind: "goal", goal: g.goal })}>
        <span class="goal-name">{c.name(g.goal)}</span>
        <GoalStatus status={c.statusOf(g.goal)} />
        {#if c.board?.goals[g.goal]?.refuted}<span class="status" data-status="refuted">refuted</span>{/if}
        {#if g.repeat}<span class="muted">{folded ? "shown elsewhere" : "shown above"}</span>{:else if !g.facts.length}<span class="muted">no certificate yet</span>{:else if !g.facts.some((t) => inUse(t.fact))}<span class="muted">no certificate in use</span>{/if}
      </button>
      {#if !g.repeat}
        <span class="row-marks">
          {#if claim}<span class="mark" title={`Claimed by ${actorName(claim.actor)}, ${timeLeft(claim.expires)}`}><UserRound size={12} />{actorName(claim.actor).split(" ")[0]} · {timeLeft(claim.expires).replace(" left", "")}</span>{/if}
          {#if notes.length}<span class="mark" title={`${notes.length} ${notes.length === 1 ? "note" : "notes"}, the latest ${when(notes.at(-1)!.at)}`}><NotebookPen size={12} />{notes.length}</span>{/if}
        </span>
      {/if}
    </div>
    {#if !g.repeat && !closed && facts.length}
      <ul class="tree">
        {#each used as t (t.fact.id)}{@render factNode(t, folded)}{/each}
        {#if unused.length}
          <li class="unused-fold">
            <button class="link" aria-expanded={open} onclick={() => (unfolded[g.goal] = !open)}>
              {#if open}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
              {tally(unused)}
            </button>
          </li>
          {#if open}{#each unused as t (t.fact.id)}{@render factNode(t, true)}{/each}{/if}
        {/if}
      </ul>
    {/if}
  </li>
{/snippet}

{#snippet factNode(t: TreeFact, folded: boolean)}
  {@const f = t.fact}
  {@const premises = filtering ? t.premises.filter((g) => kept.goals.has(g)) : t.premises}
  <li class="cert-node" class:dim={f.state !== "active"}>
    <div class="tree-row" class:selected={factSelected(f.id)}>
      <button onclick={() => onSelect({ kind: "fact", fact: f.id })}>
        <span class="kind">{c.kindOf(f)}</span>
        <span class="muted">{c.origin(f)}</span>
        {#if f.state !== "active"}<span class="status">{f.state}</span>{/if}
        {#if c.onRoute.has(f.id)}<span class="route">on {c.branch?.branch}'s route</span>{/if}
        {#each c.pickedBy(f.id) as branch (branch)}<span class="route">picked by {branch}</span>{/each}
        {#if f.review}<span class="muted" title={f.review.comment ?? ""}>{f.review.state} by {f.review.actor}{f.review.comment ? `: ${f.review.comment}` : ""}</span>{/if}
        <code class="cert-id" title={f.id}>{f.id.slice(0, 10)}</code>
      </button>
    </div>
    {#if premises.length}
      <ul class="tree">{#each premises as p (p.goal)}{@render goalNode(p, folded)}{/each}</ul>
    {/if}
  </li>
{/snippet}

<section aria-label="Proof tree">
  <div class="tree-filter">
    <label class="search"><Search size={13} strokeWidth={1.8} aria-hidden="true" /><input type="search" aria-label="Filter goals" placeholder="Filter goals by name" bind:value={query} /></label>
    <div class="seg" role="group" aria-label="Show goals">
      {#each ONLY as o (o.key)}<button aria-pressed={only === o.key} title={o.title} onclick={() => (only = o.key)}>{o.label}</button>{/each}
    </div>
  </div>
  {#if c.tree.root && (!filtering || kept.goals.has(c.tree.root))}
    <ul class="tree top">{@render goalNode(c.tree.root)}</ul>
  {:else if !c.tree.root}
    <p class="muted">The root theorem is not converted yet.</p>
  {/if}
  {#if c.tree.others.some((g) => !filtering || kept.goals.has(g))}
    <p class="group">Goals the root does not reach</p>
    <ul class="tree top">{#each c.tree.others.filter((g) => !filtering || kept.goals.has(g)) as g (g.goal)}{@render goalNode(g)}{/each}</ul>
  {/if}
  {#if filtering && !kept.goals.size}<p class="muted">No goal matches.</p>{/if}
</section>
