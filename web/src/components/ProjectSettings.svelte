<script lang="ts">
  import { modal } from "../lib/modal";
  import LeanProjectForm from "./LeanProjectForm.svelte";
  import LatexProjectForm from "./LatexProjectForm.svelte";
  import { localLeanEditor } from "../lib/features";
  let { onClose }: { onClose: () => void } = $props();
  // The Lake project only configures local Lean; without it the stored configuration is
  // preserved unchanged (plan §5) and LeanGround environments are set per proof request.
  let tab = $state<"lean" | "latex">(localLeanEditor ? "lean" : "latex");
  let form = $state<{canClose: () => boolean}>();
  function close() { if (form?.canClose()) onClose(); }
  function select(next: typeof tab) { if (next !== tab && form?.canClose()) tab = next; }
</script>
<dialog class="modal-dialog" aria-label="Project settings" use:modal oncancel={(event) => { event.preventDefault(); close(); }}>
  <header class="dialog-head"><h2>Project settings</h2></header>
  <div class="tabs" role="tablist" aria-label="Project language">
    {#if localLeanEditor}<button role="tab" aria-selected={tab === 'lean'} onclick={() => select('lean')}>Lean</button>{/if}
    <button role="tab" aria-selected={tab === 'latex'} onclick={() => select('latex')}>LaTeX</button>
  </div>
  {#if tab === 'lean'}<LeanProjectForm bind:this={form} onClose={close} />
  {:else}<LatexProjectForm bind:this={form} onClose={close} />{/if}
</dialog>
<style>
  dialog { width:min(760px,92vw); max-height:90vh; overflow:auto; }
  .tabs { display:flex; gap:.4rem; padding:.75rem 1rem 0; }
  .tabs button { padding:.4rem .9rem; border:1px solid var(--mdc-border); border-radius:var(--mdc-radius-sm); background:var(--mdc-bg); color:var(--mdc-muted); cursor:pointer; }
  .tabs button[aria-selected="true"] { color:var(--mdc-accent); border-color:var(--mdc-accent); background:var(--mdc-accent-soft); }
</style>
