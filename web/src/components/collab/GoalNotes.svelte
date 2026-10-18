<script lang="ts">
  // A goal's attempt notes (§2.7.3) in the inspector: a new note, then every note newest first,
  // rendered as Markdown. Authors edit and delete their notes; the owner deletes any. The
  // inspector mounts one per goal, so another goal starts without a draft.
  import type { Collab } from "../../lib/collab.svelte";
  import { actorName, when } from "../../lib/collab.svelte";
  import type { Note } from "../../lib/coordination";
  import MarkdownEditor from "../MarkdownEditor.svelte";
  import MarkdownPreview from "../MarkdownPreview.svelte";
  let { c, goal }: { c: Collab; goal: string } = $props();
  const LENGTH = 500;
  const notes = $derived([...(c.board?.goals[goal]?.notes ?? [])].reverse());
  let text = $state("");
  let editing = $state<string | null>(null);
  let draft = $state("");
  const keyOf = (n: Note) => `${n.actor} ${n.at}`;
  async function add() {
    const t = text.trim();
    if (!t || t.length > LENGTH || c.busy) return;
    await c.run({ type: "note", goal, text: t });
    if (!c.refused) text = "";
  }
  async function save(n: Note) {
    const t = draft.trim();
    if (!t || t.length > LENGTH || c.busy) return;
    await c.run({ type: "edit_note", goal, note: { actor: n.actor, at: n.at }, text: t });
    if (!c.refused) editing = null;
  }
  async function remove(n: Note) {
    if (!confirm(`Delete this note on ${c.name(goal)}? Agents no longer see it.`)) return;
    await c.run({ type: "delete_note", goal, note: { actor: n.actor, at: n.at } });
  }
</script>

<section class="goal-notes" aria-label={`Notes on ${c.name(goal)}`}>
  <p class="muted note-help">What was tried and where it got stuck, in Markdown with math in <code>$…$</code>. Agents read the latest 20 in the file <code>mdc pull</code> writes.</p>
  {#if !c.frozen}
    <MarkdownEditor label="New note" placeholder="What was tried, where it got stuck" maxlength={LENGTH} bind:value={text} onSubmit={() => void add()}>
        {#snippet actions()}
          <button class="primary" disabled={c.busy || !text.trim() || text.trim().length > LENGTH} onclick={() => void add()}>Add note</button>
        {/snippet}
    </MarkdownEditor>
  {/if}
  {#each notes as n (keyOf(n))}
    <div class="note">
      <p class="note-meta">
        <strong>{actorName(n.actor)}</strong>
        <span class="muted">· {when(n.at)}{n.edited ? ` · edited ${when(n.edited)}` : ""}{n.attempt ? " · when a claim ended" : ""}</span>
        <span style="flex: 1"></span>
        {#if !c.frozen && editing !== keyOf(n)}
          {#if c.ownsNote(n)}<button class="link" disabled={c.busy} onclick={() => { editing = keyOf(n); draft = n.text; }}>Edit</button>{/if}
          {#if c.canDeleteNote(n)}<button class="link danger" disabled={c.busy} onclick={() => void remove(n)}>Delete</button>{/if}
        {/if}
      </p>
      {#if editing === keyOf(n)}
        <MarkdownEditor label={`Edit note by ${actorName(n.actor)}`} maxlength={LENGTH} focus bind:value={draft}
          onSubmit={() => void save(n)} onCancel={() => (editing = null)}>
          {#snippet actions()}
            <button onclick={() => (editing = null)}>Cancel</button>
            <button class="primary" disabled={c.busy || !draft.trim() || draft.trim().length > LENGTH} onclick={() => void save(n)}>Save</button>
          {/snippet}
        </MarkdownEditor>
      {:else}
        <div class="note-body"><MarkdownPreview compact source={n.text} /></div>
      {/if}
    </div>
  {:else}
    <p class="muted">No notes yet.</p>
  {/each}
</section>
