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
    Zap,
  } from "@lucide/svelte";
  import { Compartment, EditorState, StateEffect, Text, type Extension } from "@codemirror/state";
  import {
    EditorView,
    keymap,
    lineNumbers,
    highlightSpecialChars,
    highlightActiveLine,
    drawSelection,
    rectangularSelection,
    crosshairCursor,
    highlightActiveLineGutter,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { indentUnit } from "@codemirror/language";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { shikiHighlight } from "../lib/cm-shiki";
  import { getHighlighter, srctypeToLang } from "../lib/shiki";
  import type { Theme } from "../lib/theme";
  import { LatexSession } from "../lib/latex-session.svelte";
  import LatexImports from "./LatexImports.svelte";
  import { removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";

  interface Props {
    fnode: string;
    revision: string;
    block: SrcBlock;
    theme: Theme;
    active?: boolean;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void;
    onReady?: () => void;
    focusLabel?: string;
    onLatexNavigate?: (fnode: string, label: string) => void;
  }
  let { fnode, revision, block, theme, active = true, onDeleted, onSaved, onReady, focusLabel, onLatexNavigate }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let editorView: EditorView | null = null;
  let dirty = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let lastSavedDoc: Text | null = null;
  let error: string | null = $state(null);
  let expanded = $state(true);
  let shikiError: string | null = $state(null);
  let previewing = $state(false);
  let latex = $state<LatexSession | null>(null);
  let previewError: string | null = $state(null);
  let LatexPreviewComponent = $state<typeof import("./LatexPreview.svelte").default | null>(null);
  let latexPreviewPromise: Promise<typeof import("./LatexPreview.svelte").default> | null = null;
  let previewRequest = 0;
  let alive = false;
  const draftId = Symbol("block draft");
  const syntaxCompartment = new Compartment();
  let readyReported = false;
  let syntaxRequest = 0;
  let pendingSyntaxTheme: Theme | null = null;
  let appliedSyntaxTheme: Theme | null = null;
  let syntaxRetryCount = 0;
  let syntaxRetryTimer: ReturnType<typeof setTimeout> | null = null;

  const SHIKI_THEMES: Record<Theme, string> = {
    dark: "tokyo-night",
    light: "github-light",
  };

  function buildBaseExtensions(): Extension[] {
    return [
      lineNumbers(),
      highlightSpecialChars(),
      highlightActiveLine(),
      drawSelection(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLineGutter(),
      history(),
      keymap.of([
        // Save with Ctrl/Cmd+S or Ctrl+Enter, and stop the browser's
        // "save page" default while the editor has focus.
        { key: "Mod-s", run: () => { void save(); return true; } },
        { key: "Mod-Enter", run: () => { void save(); return true; } },
        ...defaultKeymap,
        ...historyKeymap,
        indentWithTab,
      ]),
      EditorState.tabSize.of(4),
      indentUnit.of("    "),
      EditorView.lineWrapping,
      EditorView.editorAttributes.of(view => ({
        "data-selection": view.state.selection.ranges.some(range => !range.empty) ? "range" : "cursor",
      })),
      EditorView.theme({
        "&": {
          backgroundColor: "var(--mdc-code-bg)",
          color: "var(--mdc-code-fg)",
        },
        "&.cm-focused": {
          outline: "none",
        },
        ".cm-content": { caretColor: "var(--mdc-accent)" },
        ".cm-content:focus-visible": { outline: "none" },
        ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--mdc-accent)" },
        "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
          backgroundColor: "var(--mdc-editor-selection) !important",
        },
        ".cm-gutters": {
          backgroundColor: "var(--mdc-code-bg)",
          borderRight: "1px solid var(--mdc-border)",
          color: "var(--mdc-code-dim)",
        },
        ".cm-activeLine": { backgroundColor: "var(--mdc-editor-active)" },
        '&[data-selection="range"] .cm-activeLine': { backgroundColor: "transparent" },
        ".cm-activeLineGutter": { backgroundColor: "var(--mdc-editor-active)" },
      }),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          setDirty(lastSavedDoc === null || !u.state.doc.eq(lastSavedDoc));
          latex?.schedule(u.state.doc.toString());
        }
      }),
    ];
  }

  function reportReadyAfterMeasure(view: EditorView) {
    view.requestMeasure({
      read: () => null,
      write: () => {
        if (!alive || editorView !== view || readyReported) return;
        readyReported = true;
        onReady?.();
        // Let the editor paint before grammar compilation occupies the main thread.
        requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(ensureSyntaxHighlighting, 50)));
      },
    });
  }

  function ensureSyntaxHighlighting() {
    if (!editorView || !readyReported || !active || !expanded || previewing ||
      appliedSyntaxTheme === theme || pendingSyntaxTheme === theme) return;
    const requestedTheme = theme;
    const request = ++syntaxRequest;
    pendingSyntaxTheme = requestedTheme;
    shikiError = null;
    const lang = srctypeToLang(block.srctype);
    getHighlighter(lang)
      .then((hl) => {
        if (!alive || request !== syntaxRequest || !editorView) return;
        pendingSyntaxTheme = null;
        if (!active || !expanded || previewing || theme !== requestedTheme) {
          return;
        }
        syntaxRetryCount = 0;
        editorView.dispatch({
          effects: syntaxCompartment.reconfigure(
            shikiHighlight(hl, lang, SHIKI_THEMES[requestedTheme], (error) => {
              if (alive) shikiError = error === null ? null : errMsg(error);
            }),
          ),
        });
        appliedSyntaxTheme = requestedTheme;
      })
      .catch((e) => {
        if (!alive || request !== syntaxRequest) return;
        pendingSyntaxTheme = null;
        shikiError = errMsg(e);
        if (active && syntaxRetryCount < 2) {
          const delay = 500 * 2 ** syntaxRetryCount++;
          syntaxRetryTimer = setTimeout(() => {
            syntaxRetryTimer = null;
            if (alive && active) ensureSyntaxHighlighting();
          }, delay);
        }
      });
  }

  function suspendSyntaxHighlighting() {
    if (!editorView ||
      (pendingSyntaxTheme === null && appliedSyntaxTheme === null && syntaxRetryTimer === null)) return;
    syntaxRequest++;
    pendingSyntaxTheme = null;
    appliedSyntaxTheme = null;
    if (syntaxRetryTimer) {
      clearTimeout(syntaxRetryTimer);
      syntaxRetryTimer = null;
    }
    editorView.dispatch({ effects: syntaxCompartment.reconfigure([]) });
  }

  function setDirty(value: boolean) {
    dirty = value;
    setDraftDirty(draftId, value);
  }

  onMount(() => {
    alive = true;
    const initialState = EditorState.create({
      doc: block.content,
      extensions: [...buildBaseExtensions(), syntaxCompartment.of([])],
    });
    lastSavedDoc = initialState.doc;
    editorView = new EditorView({
      state: initialState,
      parent: host!,
    });
    if (block.srctype === "latex") {
      const session = new LatexSession(fnode, block.content);
      latex = session;
      void import("../lib/latex-completion").then(({latexAutocomplete}) => {
        if (alive && editorView) editorView.dispatch({effects: StateEffect.appendConfig.of(latexAutocomplete(session))});
      }).catch(e => { if (alive) previewError = errMsg(e); });
    }
    reportReadyAfterMeasure(editorView);
  });

  $effect(() => {
    void theme;
    if (!alive) return;
    if (active && expanded && !previewing) ensureSyntaxHighlighting();
    else suspendSyntaxHighlighting();
  });

  $effect(() => { latex?.setActive(active && expanded); });
  let focusedLabel: string | undefined;
  $effect(() => {
    if (focusLabel && latex && focusedLabel !== focusLabel) {
      focusedLabel = focusLabel;
      expanded = true;
      if (!previewing) void toggleLatexPreview();
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
    const contentDoc = editorView.state.doc;
    const content = contentDoc.toString();
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
      lastSavedDoc = Text.of(updated.content.split("\n"));
      // A response may normalize the submitted text, but it must never replace
      // edits made while that request was in flight.
      if (editorView.state.doc.eq(contentDoc)) {
        if (content !== updated.content) {
          editorView.dispatch({
            changes: { from: 0, to: editorView.state.doc.length, insert: updated.content },
          });
        }
      }
    } catch (e) {
      if (isCurrent()) error = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) {
        if (editorView) setDirty(lastSavedDoc === null || !editorView.state.doc.eq(lastSavedDoc));
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

  async function toggleLatexPreview() {
    if (block.srctype !== "latex") return;
    if (previewing) {
      previewRequest++;
      previewing = false;
      await tick();
      editorView?.requestMeasure();
      return;
    }
    previewing = true;
    previewError = null;
    if (LatexPreviewComponent) return;
    const request = ++previewRequest;
    latexPreviewPromise ??= import("./LatexPreview.svelte").then((module) => module.default);
    try {
      const component = await latexPreviewPromise;
      if (!alive || request !== previewRequest) return;
      LatexPreviewComponent = component;
    } catch (loadError) {
      latexPreviewPromise = null;
      if (!alive || request !== previewRequest) return;
      previewing = false;
      previewError = `preview failed: ${errMsg(loadError)}`;
    }
  }

  onDestroy(() => {
    alive = false;
    latex?.destroy();
    syntaxRequest++;
    previewRequest++;
    if (syntaxRetryTimer) clearTimeout(syntaxRetryTimer);
    removeDraft(draftId);
    editorView?.destroy();
    editorView = null;
  });

  // Update content changed by a save response or an external refresh.
  $effect(() => {
    const nextContent = block.content;
    const nextSavedDoc = Text.of(nextContent.split("\n"));
    if (lastSavedDoc?.eq(nextSavedDoc)) return;
    error = null;
    lastSavedDoc = nextSavedDoc;
    if (editorView) {
      if (!dirty && !editorView.state.doc.eq(lastSavedDoc)) {
        editorView.dispatch({
          changes: { from: 0, to: editorView.state.doc.length, insert: nextContent },
        });
      }
    }
    setDirty(editorView !== null && !editorView.state.doc.eq(lastSavedDoc));
  });
</script>

<article class="source-block" data-srctype={block.srctype}>
  <header class="block-head">
    <span class="srctype">{block.srctype}</span>
    <span class="spacer"></span>
    {#if dirty}<span class="dirty" title="Unsaved changes"><span class="dirty-dot"></span><span class="btn-label">Unsaved</span></span>{/if}
    {#if latex?.working}<span class="saving">rendering…</span>{/if}
    {#if saving}<span class="saving">saving…</span>{/if}
    {#if deleting}<span class="saving">deleting…</span>{/if}
    {#if error || previewError}<span class="error" title={error ?? previewError ?? "error"}><AlertTriangle size={14} strokeWidth={1.9} /></span>{/if}
    {#if shikiError}<span class="error" title={`highlight: ${shikiError}`}><Zap size={14} strokeWidth={1.9} /></span>{/if}
    {#if block.srctype === "latex"}
      <button
        class="preview-toggle"
        class:active={previewing}
        onclick={() => void toggleLatexPreview()}
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
  <div
    class="editor-host"
    class:collapsed={!expanded || previewing}
    inert={deleting}
    bind:this={host}
  >
  </div>
  {#if previewing && expanded}
    {#if LatexPreviewComponent && latex?.preview}
      <LatexPreviewComponent html={latex.preview.html} labels={latex.preview.labels} {fnode} {focusLabel} onNavigate={onLatexNavigate} />
    {:else if !latex?.error}
      <div class="preview-loading" aria-busy="true">Preparing preview…</div>
    {/if}
  {/if}
  {#if latex?.error || latex?.contextError}<div class="error-bar" role="alert">{latex.error ?? latex.contextError}<button onclick={() => { void latex?.refresh(); void latex?.render(); }}>Retry</button></div>{/if}
  {#if latex?.preview?.diagnostics.length}
    <details class="latex-diagnostics"><summary>{latex.preview.diagnostics.length} LaTeX diagnostic(s)</summary>
      {#each latex.preview.diagnostics as message}<p>{message}</p>{/each}
    </details>
  {/if}
  {#if error || previewError}<div class="error-bar">{error ?? previewError}</div>{/if}
</article>

<style>
  .editor-host { display:flex; min-height:0; background:var(--mdc-code-bg); }
  .editor-host.collapsed { display: none; }
  .editor-host :global(.cm-editor) {
    flex: 1;
    min-width: 0;
    min-height: 0;
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-sm);
    line-height: 1.65;
  }
  .editor-host :global(.cm-editor .cm-scroller) { overflow:auto; overscroll-behavior:contain; font-family:var(--mdc-mono); }
  .editor-host :global(.cm-content) { min-height:10rem; }
  .preview-loading {
    min-height: 9rem;
    display: grid;
    place-items: center;
    color: var(--mdc-code-dim);
    background: var(--mdc-code-bg);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
  }
  .latex-diagnostics { flex-shrink:0; max-height:30cqh; overflow:auto; overscroll-behavior:contain; padding:.5rem .75rem; color:var(--mdc-warning); font-size:var(--mdc-text-xs); }
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
