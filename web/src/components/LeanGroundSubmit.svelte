<script lang="ts">
  // "Submit to LeanGround": the first submission in a database creates its project board with
  // this node as the root theorem (owners only); later ones attach this branch to it. The
  // coordinator converts the branch from the root; progress shows in Collaboration.
  import { coordination, currentBranch } from "../lib/coordination";
  import { errMsg } from "../lib/format";
  let { fnode, submitted, onOpen }: {
    fnode: string; submitted: boolean; onOpen: (project: string | null) => void;
  } = $props();
  let open = $state(false), busy = $state(false), error = $state<string | null>(null);
  let budget = $state(1000), members = $state("");
  const branch = currentBranch();
  async function submit() {
    if (!branch || busy) return;
    busy = true; error = null;
    try {
      const board = await coordination.create({
        document: { ...branch, node: fnode },
        members: members.split(/[\s,]+/).filter(Boolean),
        budget,
      });
      open = false;
      onOpen(board.id);
    } catch (e) {
      const reason = errMsg(e);
      error = reason === "environment_not_configured"
        ? "Set the proof environment in Project settings first."
        : reason === "not_a_project_member"
          ? "This database already has a project. Ask its owner to add you as a member."
          : reason === "root_not_on_branch"
            ? "This branch does not contain the project's root theorem node."
            : reason;
    } finally { busy = false; }
  }
</script>
{#if submitted}
  <button class="submit" onclick={() => onOpen(null)}>Collaboration</button>
{:else}
  <button class="submit" onclick={() => (open = !open)} aria-expanded={open}>Submit to LeanGround</button>
{/if}
{#if open}
  <div class="popover" role="dialog" aria-label="Submit to LeanGround">
    <p>Attaches this branch to the database's project; with no project yet, this node becomes its root (owners only). <code>sorry</code> proofs become tasks.</p>
    <label>Members <input bind:value={members} placeholder="bob, carol" /></label>
    <label>Budget <input type="number" min="0" bind:value={budget} /></label>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button onclick={() => (open = false)} disabled={busy}>Cancel</button>
      <button onclick={() => void submit()} disabled={busy}>{busy ? "Submitting..." : "Submit"}</button>
    </div>
  </div>
{/if}
<style>
  .submit { font-size:.75rem; padding:.1rem .5rem; border:1px solid var(--mdc-accent); color:var(--mdc-accent); background:var(--mdc-accent-soft); border-radius:var(--mdc-radius-sm); cursor:pointer; }
  .popover { position:absolute; z-index:20; margin-top:1.8rem; width:min(360px,90vw); padding:.75rem; background:var(--mdc-bg); border:1px solid var(--mdc-border); border-radius:var(--mdc-radius-sm); box-shadow:0 6px 20px rgb(0 0 0 / .15); font-size:.8rem; }
  .popover p { color:var(--mdc-muted); margin:0 0 .5rem; }
  label { display:block; margin:.4rem 0; }
  input { display:block; width:100%; padding:.3rem; margin-top:.2rem; color:var(--mdc-fg); background:var(--mdc-bg); border:1px solid var(--mdc-border); border-radius:4px; }
  .actions { display:flex; justify-content:flex-end; gap:.4rem; }
  .error { color:var(--mdc-error); }
</style>
