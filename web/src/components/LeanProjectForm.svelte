<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api, type LeanProject } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { confirmDiscardDraft, removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  let { onClose }: { onClose: () => void } = $props();
  let toolchain = $state(""), lakefile = $state(""), manifest = $state("");
  let baseline = $state(""), revision = $state("");
  let busy = $state(true), error = $state<string | null>(null);
  let original = $state<LeanProject | null>(null);
  const draft = Symbol("Lean project");
  const value = () => ({ ...original, toolchain, lakefile, manifest: manifest.trim() || null });
  $effect(() => setDraftDirty(draft, baseline !== "" && JSON.stringify(value()) !== baseline));
  onDestroy(() => removeDraft(draft));
  onMount(async () => {
    try {
      const result = await api.project();
      original = result.project;
      ({ toolchain, lakefile } = result.project); manifest = result.project.manifest ?? "";
      revision = result.revision; baseline = JSON.stringify(value());
    } catch (e) { error = errMsg(e); }
    finally { busy = false; }
  });
  export function canClose() { return !busy && confirmDiscardDraft(draft); }
  async function save() {
    busy = true; error = null; const release = trackMutation();
    try { await api.putProject(value(), revision); baseline = JSON.stringify(value()); removeDraft(draft); busy = false; onClose(); }
    catch (e) { error = errMsg(e); }
    finally { busy = false; release(); }
  }
</script>


  <div class="body">
    <p>Pin the toolchain and libraries here. Each environment keeps its own build cache. Reload open Lean editors after changing it.</p>
    <label>Lean toolchain<input bind:value={toolchain} disabled={busy} /></label>
    <label>{original?.lakefile_name ?? "lakefile.toml"}<textarea bind:value={lakefile} rows="9" spellcheck="false" disabled={busy}></textarea></label>
    <label>lake-manifest.json<textarea bind:value={manifest} rows="7" spellcheck="false" disabled={busy} placeholder="Required for external libraries; paste the manifest produced by Lake."></textarea></label>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </div>
  <footer class="dialog-footer actions">
    <button onclick={onClose} disabled={busy}>Cancel</button>
    <button onclick={() => void save()} disabled={busy || !revision}>Save environment</button>
  </footer>

<style>
  .body { padding:1rem; }
  p { color:var(--mdc-muted); font-size:.85rem; }
  label { display:block; margin:.8rem 0; font-size:.8rem; }
  input,textarea { display:block; width:100%; box-sizing:border-box; margin-top:.3rem; padding:.5rem; border:1px solid var(--mdc-border); border-radius:4px; background:var(--mdc-bg); color:var(--mdc-fg); font-family:var(--mdc-mono); }
  .error { color:var(--mdc-error); }
</style>
