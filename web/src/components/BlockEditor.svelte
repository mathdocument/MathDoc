<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import {
    AlertTriangle,
    ChevronDown,
    ChevronRight,
    Code2,
    Eye,
    Save as SaveIcon,
    Trash2,
  } from "@lucide/svelte";
  import { editor as Monaco, Uri, KeyMod, KeyCode } from "monaco-editor";
  import { loadSourceLanguage, setMonacoTheme, sourceOptions, renderSource } from "../lib/monaco";
  import { nativeMonacoScroll } from "../lib/monaco-scroll";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { Theme } from "../lib/theme";
  import { LatexSession } from "../lib/latex-session.svelte";
  import type { PreparedLatexPreview } from "../lib/latex";
  import LatexImports from "./LatexImports.svelte";
  import LatexPreview from "./LatexPreview.svelte";
  import { removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  import { latexAutocomplete } from "../lib/latex-completion-widget";

  interface Props {
    fnode: string;
    revision: string;
    block: SrcBlock;
    theme: Theme;
    active?: boolean;
    latexPreview?: boolean;
    preparedLatex?: PreparedLatexPreview;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void;
    onReady?: () => void;
    focusLabel?: string;
    onLatexNavigate?: (fnode: string, label: string) => void;
  }
  let { fnode, revision, block, theme, active = true, latexPreview = $bindable(false), preparedLatex, onDeleted, onSaved, onReady, focusLabel, onLatexNavigate }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let scroller: HTMLDivElement;
  let editorView: Monaco.IStandaloneCodeEditor | null = null;
  let model: Monaco.ITextModel | null = null;
  let disposeScroll: (() => void) | undefined;
  let completion: {dispose(): void} | undefined;
  let widgets: HTMLDivElement | undefined;
  let ready = $state(false);
  let dirty = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let lastSavedDoc = "";
  let error: string | null = $state(null);
  let expanded = $state(true);
  let previewing = $derived(block.srctype === "latex" && latexPreview);
  let showPreview = $state(false);
  let latex = $state<LatexSession | null>(null);
  let alive = false;
  let initializing = false;
  const draftId = Symbol("block draft");
  function setDirty(value: boolean) {
    dirty = value;
    setDraftDirty(draftId, value);
  }

  onMount(() => {
    alive = true;
    if (block.srctype === "latex") latex = new LatexSession(fnode, block.content, preparedLatex);
  });

  async function ensureEditor() {
    if (!alive || editorView || initializing) return;
    initializing = true;
    try {
      const language = await loadSourceLanguage(block.srctype as "text" | "latex" | "rocq");
      if (!alive) return;
      await setMonacoTheme(theme);
      if (!alive) return;
      model = Monaco.createModel(block.content, language, Uri.parse(`inmemory://mdc/${fnode}/${block.srctype}`));
      // Native overflow widgets must escape the block's clipping/stacking context.
      widgets = document.createElement('div');
      widgets.className = 'monaco-editor mdc-editor-widgets';
      document.body.append(widgets);
      editorView = Monaco.create(host!, {
        ...sourceOptions, model, ariaLabel: `${block.srctype} source`, overflowWidgetsDomNode: widgets,
        suggestFontSize: 12, suggestLineHeight: 28, suggest: {showIcons: false, showStatusBar: false},
      });
      disposeScroll = nativeMonacoScroll(editorView, scroller);
      const fit = () => scroller.style.setProperty('--source-height', `${editorView!.getContentHeight()}px`);
      editorView.onDidContentSizeChange(fit); fit();
      model.onDidChangeContent(() => {
        setDirty(model!.getValue() !== lastSavedDoc);
        latex?.schedule(model!.getValue());
      });
      editorView.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => void save());
      editorView.addCommand(KeyMod.CtrlCmd | KeyCode.Enter, () => void save());
      if (latex) completion = latexAutocomplete(latex, editorView);
      requestAnimationFrame(() => requestAnimationFrame(() => {
        if (!alive) return;
        renderSource(editorView!); ready = true; onReady?.();
      }));
    } catch (e) {
      if (alive) { error = errMsg(e); onReady?.(); }
    } finally { initializing = false; }
  }

  $effect(() => {
    if (previewing) showPreview = true;
    else if (ready) showPreview = false;
    if (!active || !expanded) return;
    if (!previewing) void ensureEditor();
    else if (latex?.preview || latex?.error) onReady?.();
  });

  $effect(() => { const next = theme; if (ready) void setMonacoTheme(next); });

  $effect(() => { latex?.setActive(active && expanded); });
  $effect(() => {
    if (!showPreview && ready) void tick().then(() => { if (alive && editorView) renderSource(editorView); });
  });
  let focusedLabel: string | undefined;
  $effect(() => {
    if (focusLabel && latex && focusedLabel !== focusLabel) {
      focusedLabel = focusLabel;
      expanded = true;
      latexPreview = true;
    }
  });

  async function save() {
    if (!editorView || !dirty || saving || deleting) return;
    const targetFnode = fnode;
    const targetSrctype = block.srctype;
    const targetRevision = revision;
    saving = true;
    const clearMutation = trackMutation();
    error = null;
    const content = editorView.getValue();
    const isCurrent = () => alive;

    try {
      const node = await api.putBlock(targetFnode, targetSrctype, content, targetRevision);
      if (!isCurrent() || !editorView) return;
      const updated = node.blocks.find((b) => b.srctype === targetSrctype);
      if (!updated) {
        error = `saved response is missing the ${targetSrctype} block`;
        return;
      }

      onSaved?.(node);
      lastSavedDoc = updated.content;
      // A response may normalize the submitted text, but it must never replace
      // edits made while that request was in flight.
      if (editorView.getValue() === content) {
        if (content !== updated.content) {
          editorView.setValue(updated.content);
        }
      }
    } catch (e) {
      if (isCurrent()) error = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) {
        if (editorView) setDirty(editorView.getValue() !== lastSavedDoc);
        saving = false;
      }
    }
  }

  async function onDelete() {
    if (saving || deleting) return;
    if (!confirm(`Delete the ${block.srctype} block from this node?`)) return;
    const targetFnode = fnode;
    const targetSrctype = block.srctype;
    const targetRevision = revision;
    error = null;
    deleting = true;
    const clearMutation = trackMutation();
    const isCurrent = () => alive;
    try {
      const node = await api.deleteBlock(targetFnode, targetSrctype, targetRevision);
      if (!isCurrent()) return;
      setDirty(false);
      clearMutation();
      onDeleted?.(node, targetSrctype);
    } catch (e) {
      if (isCurrent()) error = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) deleting = false;
    }
  }

  function toggleExpand() { expanded = !expanded; }

  onDestroy(() => {
    alive = false;
    latex?.destroy();
    removeDraft(draftId);
    completion?.dispose();
    disposeScroll?.();
    editorView?.dispose();
    widgets?.remove();
    model?.dispose();
    editorView = null;
  });

  // Update content changed by a save response or an external refresh.
  $effect(() => {
    const nextContent = block.content;
    if (lastSavedDoc === nextContent) return;
    error = null;
    lastSavedDoc = nextContent;
    if (editorView && !dirty && editorView.getValue() !== nextContent) editorView.setValue(nextContent);
    if (!editorView && latex && latex.source !== nextContent) latex.schedule(nextContent);
    setDirty(editorView !== null && editorView.getValue() !== lastSavedDoc);
  });
</script>

<article class="source-block" data-srctype={block.srctype}>
  <header class="block-head">
    <span class="srctype">{block.srctype}</span>
    <span class="spacer"></span>
    {#if dirty}<span class="dirty" title="Unsaved changes"><span class="dirty-dot"></span><span class="btn-label">Unsaved</span></span>{/if}
    {#if latex?.working}<span class="saving">rendering...</span>{/if}
    {#if saving}<span class="saving">saving...</span>{/if}
    {#if deleting}<span class="saving">deleting...</span>{/if}
    {#if error}<span class="error" title={error}><AlertTriangle size={14} strokeWidth={1.9} /></span>{/if}
    {#if block.srctype === "latex"}
      <button
        class="preview-toggle"
        class:active={previewing}
        onclick={() => { latexPreview = !latexPreview; }}
        disabled={!expanded}
        aria-pressed={previewing}
        title={previewing ? "Return to LaTeX editor" : "Render LaTeX preview"}
        aria-label={previewing ? "Return to LaTeX editor" : "Render LaTeX preview"}
      >
        {#if previewing}<Code2 size={14} strokeWidth={1.8} /><span class="btn-label">Edit</span>
        {:else}<Eye size={14} strokeWidth={1.8} /><span class="btn-label">Preview</span>{/if}
      </button>
    {/if}
    <button class="icon-btn expand" onclick={toggleExpand} title={expanded ? "Collapse" : "Expand"} aria-label={expanded ? "Collapse block" : "Expand block"}>
      {#if expanded}<ChevronDown size={15} strokeWidth={1.9} />{:else}<ChevronRight size={15} strokeWidth={1.9} />{/if}
    </button>
    <button class="save" onclick={save} disabled={!dirty || saving || deleting} title="Save (Ctrl/⌘+S)" aria-label="Save"><SaveIcon size={13} strokeWidth={1.9} /><span class="btn-label">Save</span></button>
    <button class="delete" onclick={onDelete} disabled={saving || deleting} title="Delete block" aria-label="Delete block"><Trash2 size={14} strokeWidth={1.8} /></button>
  </header>
  {#if latex && expanded}<LatexImports imports={latex.context?.imports ?? null} />{/if}
  <div class="editor-scroll" class:pending={!ready} class:collapsed={!expanded || showPreview} inert={deleting || !ready || !expanded || showPreview} bind:this={scroller}>
    <div class="editor-size"><div class="editor-host" bind:this={host}></div></div>
  </div>
  {#if showPreview && expanded}
    {#if latex?.preview}
      <LatexPreview html={latex.preview.html} labels={latex.preview.labels} {fnode} {focusLabel} onNavigate={onLatexNavigate} />
    {:else if !latex?.error}
      <div class="preview-loading" aria-busy="true">Preparing preview...</div>
    {/if}
  {/if}
  {#if latex?.error || latex?.contextError}<div class="error-bar" role="alert">{latex.error ?? latex.contextError}<button onclick={() => { void latex?.refresh(); void latex?.render(); }}>Retry</button></div>{/if}
  {#if latex?.preview?.diagnostics.length}
    <details class="latex-diagnostics"><summary>{latex.preview.diagnostics.length} LaTeX diagnostic(s)</summary>
      {#each latex.preview.diagnostics as message}<p>{message}</p>{/each}
    </details>
  {/if}
  {#if error}<div class="error-bar">{error}</div>{/if}
</article>

<style>
  .editor-scroll { min-height:0; height:clamp(10rem, var(--source-height, 10rem), 100cqh); overflow:auto; }
  .editor-scroll.collapsed { height:0; visibility:hidden; overflow:hidden; }
  .editor-scroll.pending { visibility:hidden; }
  .editor-size { min-width:100%; min-height:100%; }
  .editor-host { position:sticky; top:0; left:0; overflow:hidden; }
  .preview-loading {
    min-height: 9rem;
    display: grid;
    place-items: center;
    color: var(--mdc-code-dim);
    background: var(--mdc-code-bg);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
  }
  .latex-diagnostics { flex-shrink:0; max-height:30cqh; overflow:auto; padding:.5rem .75rem; color:var(--mdc-warning); font-size:var(--mdc-text-xs); }
  .latex-diagnostics p { margin:.4rem 0; }
  .error-bar button { margin-left:.5rem; }
  .error-bar {
    padding: 0.45rem 0.65rem;
    background: color-mix(in srgb, var(--mdc-error) 10%, transparent);
    color: var(--mdc-code-error);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
    border-top: 1px solid color-mix(in srgb, var(--mdc-error) 25%, transparent);
  }

</style>
