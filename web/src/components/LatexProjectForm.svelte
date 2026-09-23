<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { latexApi, type LatexProject } from '../lib/latex';
  import { errMsg } from '../lib/format';
  import { confirmDiscardDraft, removeDraft, setDraftDirty, trackMutation } from '../lib/unsaved';
  let { onClose }: {onClose: () => void} = $props();
  let project = $state<LatexProject | null>(null);
  let revision = $state(''), baseline = $state('');
  let busy = $state(true), error = $state<string | null>(null);
  let loadingFiles = $state(0);
  let alive = true;
  const draft = Symbol('LaTeX project');
  $effect(() => setDraftDirty(draft, baseline !== '' && JSON.stringify(project) !== baseline));
  onDestroy(() => { alive = false; removeDraft(draft); });
  onMount(async () => {
    try {
      const result = await latexApi.project();
      if (!alive) return;
      project = result.project; revision = result.revision; baseline = JSON.stringify(project);
    } catch (e) { if (alive) error = errMsg(e); }
    finally { if (alive) busy = false; }
  });
  export function canClose() { return !busy && loadingFiles === 0 && confirmDiscardDraft(draft); }
  async function upload(event: Event, kind: 'preamble' | 'bibliography') {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file || !project) return;
    error = null;
    loadingFiles++;
    try {
      if (file.size > (kind === 'preamble' ? 2 : 16) * 1024 * 1024) throw new Error(`${kind} file is too large`);
      const content = await file.text();
      if (alive && project) project = {...project, [kind]: content, [`${kind}_name`]: file.name};
    } catch (e) { if (alive) error = errMsg(e); }
    finally { loadingFiles--; input.value = ''; }
  }
  async function save() {
    if (!project || busy || loadingFiles) return;
    busy = true; error = null; const release = trackMutation();
    try {
      await latexApi.putProject(project, revision);
      if (!alive) return;
      baseline = JSON.stringify(project); removeDraft(draft); busy = false;
      window.dispatchEvent(new Event('mdc-latex-project-changed'));
      onClose();
    } catch (e) { if (alive) error = errMsg(e); }
    finally { busy = false; release(); }
  }
</script>
<div class="body">
  <p>Share ordinary macros and bibliography across this branch. Uploaded contents are versioned with the graph.</p>
  <label>Class or preamble<input type="file" accept=".cls,.tex" disabled={busy || loadingFiles > 0} onchange={event => void upload(event, 'preamble')} /></label>
  {#if project}<span class="file">{project.preamble_name} · {project.preamble.length.toLocaleString()} characters</span>{/if}
  <label>Bibliography<input type="file" accept=".bib" disabled={busy || loadingFiles > 0} onchange={event => void upload(event, 'bibliography')} /></label>
  {#if project}<span class="file">{project.bibliography_name} · {project.bibliography.length.toLocaleString()} characters</span>{/if}
  <p>Node dependencies supply the available references automatically. Use Preview in each LaTeX block to view the result.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
<footer class="dialog-footer actions">
  <button onclick={onClose} disabled={busy || loadingFiles > 0}>Cancel</button>
  <button onclick={() => void save()} disabled={busy || loadingFiles > 0 || !revision}>{busy ? 'Preparing...' : 'Save LaTeX project'}</button>
</footer>
<style>
  .body { padding:1rem; }
  p { color:var(--mdc-muted); font-size:.85rem; }
  label { display:block; margin:.9rem 0 .3rem; font-size:.8rem; }
  input { display:block; width:100%; margin-top:.5rem; padding:.5rem; color:var(--mdc-fg); background:var(--mdc-bg); border:1px solid var(--mdc-border); border-radius:4px; }
  .file { font-family:var(--mdc-mono); font-size:.75rem; color:var(--mdc-muted); }
  .error { color:var(--mdc-error); }
</style>
