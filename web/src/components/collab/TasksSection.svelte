<script lang="ts">
  // Tasks (§2.7.5): each goal without a proof, in the server's recommended order; claims,
  // notes and the budget by participant. Selecting a goal opens it in the inspector.
  import type { Collab, Selection } from "../../lib/collab.svelte";
  import { actorName } from "../../lib/collab.svelte";
  import { spentBy, timeLeft, type Task } from "../../lib/coordination";
  import GoalStatus from "./GoalStatus.svelte";
  let { c, onSelect }: { c: Collab; onSelect: (s: Selection) => void } = $props();
  const done = $derived(c.tasks.filter((t) => c.solved.has(t.goal)));
</script>

{#snippet row(t: Task)}
  {@const a = t.attempt ? c.board!.attempts[t.attempt] : undefined}
  {@const status = c.statusOf(t.goal)}
  <tr>
    <td>{c.ranking.has(t.id) ? `#${c.ranking.get(t.id)}` : ""}</td>
    <td>
      <button class="link goal-name" onclick={() => onSelect({ kind: "goal", goal: t.goal })}>{c.name(t.goal)}</button>
      {#if c.board!.negations?.[t.goal]} <span class="muted">refutes {c.name(c.board!.negations[t.goal])}</span>{/if}
    </td>
    <td>{t.kind}</td>
    <td>
      <GoalStatus {status} />
      {#if !c.active.has(t.goal)}<span class="status" title="No active decomposition leads to this goal yet (for example, one awaiting review), so its task cannot be claimed">inactive</span>{/if}
    </td>
    <td>{#if a}{actorName(a.actor)} · {timeLeft(a.expires)}{:else}<span class="muted">nobody</span>{/if}</td>
    <td>{(c.board!.goals[t.goal]?.notes ?? []).length || ""}</td>
  </tr>
{/snippet}
{#snippet head()}
  <thead><tr><th title="Recommended order: needed by the routes closest to closing, fewest failures first">Next</th><th>Goal</th><th>Kind</th><th>Status</th><th>Claimed by</th><th>Notes</th></tr></thead>
{/snippet}

<section aria-label="Tasks">
  <div class="section-head"><h2>Tasks</h2></div>
  <p class="muted">Each goal without a proof has a task: prove it outright, or split it into smaller goals. Agents take tasks with <code>mdc pull</code>; select a goal to claim it or leave a note.</p>
  {#if c.openTasks.length}
    <table aria-label="Open tasks">{@render head()}<tbody>{#each c.openTasks as t (t.id)}{@render row(t)}{/each}</tbody></table>
  {:else}
    <p class="muted">No open task: every goal with a task has a proof.</p>
  {/if}
  {#if done.length}
    <details><summary>Done ({done.length}): the goal has a proof</summary>
      <table aria-label="Done tasks">{@render head()}<tbody>{#each done as t (t.id)}{@render row(t)}{/each}</tbody></table>
    </details>
  {/if}
  {#if Object.keys(c.board?.attempts ?? {}).length}
    <p class="group">Budget by participant</p>
    <table aria-label="Budget by participant">
      <thead><tr><th>Participant</th><th>Claims</th><th title="Reported by finished claims">Spent</th><th title="Held by claims still open">Reserved</th></tr></thead>
      <tbody>{#each spentBy(c.board!) as r (r.actor)}<tr><td>{actorName(r.actor)}</td><td>{r.attempts}</td><td>{r.spent}</td><td>{r.reserved}</td></tr>{/each}</tbody>
    </table>
  {/if}
</section>
