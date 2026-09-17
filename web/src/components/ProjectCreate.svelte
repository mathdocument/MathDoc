<script lang="ts">
  import { GitBranchPlus, Plus, X } from "@lucide/svelte";
  import { projectsApi } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { modal } from "../lib/modal";

  let { source, onCreated, onClose }: {
    source?: string; onCreated: (project: string) => Promise<void>; onClose: () => void;
  } = $props();
  let name = $state("");
  let busy = $state(false);
  let error = $state("");
  let nameInput = $state<HTMLInputElement>();
  $effect(() => { nameInput?.focus(); });
  const title = $derived(source ? "New branch" : "Init project");
  const target = $derived(source ? `${source.split('/')[0]}/${name || '…'}` : `${name || '…'}/main`);
  function close() { if (!busy) onClose(); }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true; error = "";
    try {
      const result = await projectsApi.change(source ? {action: "new_branch", project: source, name} : {action: "init", name});
      await onCreated(result.project);
      onClose();
    } catch (e) { error = errMsg(e); }
    finally { busy = false; }
  }
</script>

<dialog class="modal-dialog" aria-label={title} use:modal oncancel={event => {event.preventDefault(); close();}} onclick={event => {if (event.target === event.currentTarget) close();}}>
  <form onsubmit={submit}>
    <header class="dialog-head">
      <span class="head-icon">{#if source}<GitBranchPlus size={17} />{:else}<Plus size={18} />{/if}</span>
      <span><h2>{title}</h2></span>
      <button type="button" class="close-btn" aria-label="Close" onclick={close} disabled={busy}><X size={17} /></button>
    </header>
    <div class="body">
      {#if source}<p class="origin">From <code>{source}</code></p>{/if}
      <label>{source ? "Branch name" : "Project name"}<input bind:this={nameInput} bind:value={name} required pattern={"[A-Za-z0-9_\\-]+"} title="Letters, digits, hyphens and underscores" autocomplete="off" spellcheck="false" disabled={busy} /></label>
      <div class="target"><GitBranchPlus size={14} /><code>{target}</code></div>
      {#if error}<p class="modal-error" role="alert">{error}</p>{/if}
    </div>
    <footer><button type="button" class="secondary" onclick={close} disabled={busy}>Cancel</button><button type="submit" class="primary" disabled={busy || !name}>{busy ? "Creating…" : source ? "New branch" : "Init"}</button></footer>
  </form>
</dialog>

<style>
  dialog { width: min(460px, calc(100vw - 32px)); }
  .head-icon { color: var(--mdc-accent); background: color-mix(in srgb, var(--mdc-accent) 12%, transparent); }
  .close-btn { margin-left: auto; }
  .body { padding: 20px; }
  .origin { margin: 0 0 18px; color: var(--mdc-dim); font-size: 12px; overflow-wrap: anywhere; }
  code { font-family: var(--mdc-mono); font-size: 12px; }
  label { display: grid; gap: 8px; color: var(--mdc-dim); font-size: 12px; }
  input { width: 100%; min-width: 0; padding: 10px 12px; border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); background: var(--mdc-bg); color: var(--mdc-fg); }
  .target { display: flex; align-items: center; gap: 8px; margin-top: 12px; color: var(--mdc-muted); overflow-wrap: anywhere; }
  .target code { min-width: 0; }
  footer { display: flex; justify-content: flex-end; gap: 8px; padding: 14px 20px; border-top: 1px solid var(--mdc-border); }
  footer button { border: 1px solid var(--mdc-border-strong); border-radius: var(--mdc-radius-sm); padding: 7px 14px; cursor: pointer; font-size: 12px; }
  .secondary { color: var(--mdc-fg-soft); background: var(--mdc-card); }
  .primary { color: var(--mdc-on-accent); background: var(--mdc-accent); border-color: var(--mdc-accent); font-weight: 600; }
  button:disabled { cursor: default; opacity: .5; }
</style>
