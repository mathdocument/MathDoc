<script lang="ts">
  import { modal } from "../lib/modal";
  import LeanProjectForm from "./LeanProjectForm.svelte";
  import LatexProjectForm from "./LatexProjectForm.svelte";
  let { onClose }: { onClose: () => void } = $props();
  let tab = $state<"lean" | "latex">("lean");
  let form = $state<{canClose: () => boolean}>();
  function close() { if (form?.canClose()) onClose(); }
  function select(next: typeof tab) { if (next !== tab && form?.canClose()) tab = next; }
</script>
<dialog class="modal-dialog" aria-label="Project settings" use:modal oncancel={(event) => { event.preventDefault(); close(); }}>
  <header class="dialog-head"><h2>Project settings</h2></header>
  <div class="tabs" role="tablist" aria-label="Project language">
    <button role="tab" aria-selected={tab === 'lean'} onclick={() => select('lean')}>Lean</button>
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
