<script lang="ts">
  import { onMount, onDestroy, tick, type Component } from "svelte";
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
  import { Compartment, EditorState, Text, type Extension } from "@codemirror/state";
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
  }
  let { fnode, revision, block, theme, active = true, onDeleted, onSaved, onReady }: Props = $props();

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
  let previewSource = $state("");
  let previewError: string | null = $state(null);
  let LatexPreviewComponent = $state<Component<{ source: string }> | null>(null);
  let latexPreviewPromise: Promise<Component<{ source: string }>> | null = null;
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
      EditorView.theme({
        "&": {
          backgroundColor: "var(--mdc-code-bg)",
          color: "var(--mdc-code-fg)",
        },
        "&.cm-focused": {
          outline: "2px solid var(--mdc-accent)",
          outlineOffset: "-2px",
        },
        ".cm-content": { caretColor: "var(--mdc-accent)" },
        ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--mdc-accent)" },
        "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
          backgroundColor: "var(--mdc-editor-selection) !important",
        },
        ".cm-gutters": {
          backgroundColor: "var(--mdc-code-bg)",
          borderRight: "1px solid var(--mdc-border)",
          color: "var(--mdc-code-dim)",
        },
        ".cm-activeLine": { backgroundColor: "var(--mdc-editor-active)" },
        ".cm-activeLineGutter": { backgroundColor: "var(--mdc-editor-active)" },
      }),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          setDirty(lastSavedDoc === null || !u.state.doc.eq(lastSavedDoc));
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
    reportReadyAfterMeasure(editorView);
  });

  $effect(() => {
    void theme;
    if (!alive) return;
    if (active && expanded && !previewing) ensureSyntaxHighlighting();
    else suspendSyntaxHighlighting();
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
    previewSource = editorView?.state.doc.toString() ?? block.content;
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
      if (previewing && editorView.state.doc.eq(lastSavedDoc)) previewSource = nextContent;
    }
    setDirty(editorView !== null && !editorView.state.doc.eq(lastSavedDoc));
  });
</script>

<article class="block" data-srctype={block.srctype}>
  <header class="block-head">
    <span class="srctype">{block.srctype}</span>
    <span class="spacer"></span>
    {#if dirty}<span class="dirty" title="Unsaved changes"><span class="dirty-dot"></span><span class="btn-label">Unsaved</span></span>{/if}
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
      >
        {#if previewing}<Code2 size={14} strokeWidth={1.8} /><span class="btn-label">Edit</span>
        {:else}<Eye size={14} strokeWidth={1.8} /><span class="btn-label">Preview</span>{/if}
      </button>
    {/if}
    <button class="icon-btn expand" onclick={toggleExpand} title={expanded ? "Collapse" : "Expand"} aria-label={expanded ? "Collapse block" : "Expand block"}>
      {#if expanded}<ChevronDown size={15} strokeWidth={1.9} />{:else}<ChevronRight size={15} strokeWidth={1.9} />{/if}
    </button>
    <button class="save" onclick={save} disabled={!dirty || saving || deleting} title="Save (Ctrl/⌘+S)"><SaveIcon size={13} strokeWidth={1.9} /><span class="btn-label">Save</span></button>
    <button class="delete" onclick={onDelete} disabled={saving || deleting} title="Delete block" aria-label="Delete block"><Trash2 size={14} strokeWidth={1.8} /></button>
  </header>
  <div
    class="editor-host"
    class:expanded
    class:collapsed={!expanded || previewing}
    inert={deleting}
    bind:this={host}
  >
  </div>
  {#if LatexPreviewComponent}
    <div class:preview-hidden={!previewing || !expanded} aria-hidden={!previewing || !expanded}>
      <LatexPreviewComponent source={previewSource} />
    </div>
  {:else if previewing && expanded}
    <div class="preview-loading" aria-busy="true">Loading renderer…</div>
  {/if}
  {#if error || previewError}<div class="error-bar">{error ?? previewError}</div>{/if}
</article>

<style>
  .block {
    --block-accent: var(--mdc-accent);
    position: relative;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    overflow: clip;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    background: var(--mdc-code-bg);
    box-shadow: var(--mdc-shadow-sm);
    transition: border-color var(--mdc-dur-fast) var(--mdc-ease),
      box-shadow var(--mdc-dur-fast) var(--mdc-ease);
    container-type: inline-size;
  }
  .block:focus-within {
    border-color: color-mix(in srgb, var(--block-accent) 40%, var(--mdc-border));
    box-shadow: var(--mdc-shadow-md);
  }
  /* Per-srctype accent so block kinds are scannable at a glance. */
  .block[data-srctype="text"] { --block-accent: var(--mdc-block-text); }
  .block[data-srctype="latex"] { --block-accent: var(--mdc-block-latex); }
  .block[data-srctype="python"] { --block-accent: var(--mdc-block-python); }
  .block[data-srctype="lean"] { --block-accent: var(--mdc-block-lean); }
  .block[data-srctype="rocq"] { --block-accent: var(--mdc-block-rocq); }
  /* Left rail carries the block kind; it is the only always-on decoration. */
  .block::before {
    content: "";
    position: absolute;
    inset: 0 auto 0 0;
    width: 2px;
    z-index: 4;
    background: var(--block-accent);
    opacity: 0.9;
  }
  .block-head {
    position: sticky;
    top: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-height: 38px;
    padding: 0.3rem 0.4rem 0.3rem 0.6rem;
    background: var(--mdc-panel-raised);
    font-size: var(--mdc-text-xs);
    color: var(--mdc-muted);
    border-bottom: 1px solid var(--mdc-border);
  }
  /* Type is stated once, as a coloured word rather than a bordered chip. */
  .srctype {
    margin-right: 0.15rem;
    color: var(--block-accent);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-2xs);
    font-weight: 650;
    letter-spacing: var(--mdc-tracking-label);
    text-transform: uppercase;
  }
  .spacer { flex: 1; }
  .dirty {
    display: inline-flex;
    align-items: center;
    gap: 0.32rem;
    margin-right: 0.15rem;
    color: var(--mdc-warning);
    font-size: var(--mdc-text-2xs);
    font-weight: 550;
  }
  .dirty-dot {
    width: 5px;
    height: 5px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--mdc-warning) 20%, transparent);
  }
  .saving { color: var(--mdc-muted); font-family: var(--mdc-mono); font-size: var(--mdc-text-2xs); }
  .error { display: inline-flex; color: var(--mdc-error); cursor: help; }
  .save, .delete, .icon-btn, .preview-toggle {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.3rem;
    min-height: 26px;
    background: transparent;
    color: var(--mdc-dim);
    border: 1px solid transparent;
    border-radius: 7px;
    padding: 0 0.45rem;
    font-size: var(--mdc-text-2xs);
    font-weight: 550;
    cursor: pointer;
    font-family: inherit;
    transition: background var(--mdc-dur-fast) var(--mdc-ease),
      color var(--mdc-dur-fast) var(--mdc-ease);
  }
  .icon-btn,
  .delete {
    width: 26px;
    padding: 0;
  }
  .preview-toggle.active,
  .preview-toggle:hover:not(:disabled) {
    color: var(--mdc-accent-up);
    background: color-mix(in srgb, var(--mdc-accent-up) 14%, transparent);
  }
  .preview-toggle:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .save:disabled { opacity: 0.3; cursor: default; }
  /* Save only draws attention once there is something to save. */
  .save:not(:disabled) {
    color: var(--mdc-on-accent);
    background: var(--block-accent);
    font-weight: 620;
  }
  .save:not(:disabled):hover {
    filter: brightness(1.08);
  }
  .delete:hover {
    background: color-mix(in srgb, var(--mdc-error) 14%, transparent);
    color: var(--mdc-error);
  }
  .expand:hover { background: var(--mdc-card-hover); color: var(--mdc-fg); }
  .editor-host { background: var(--mdc-code-bg); }
  .editor-host.expanded { height: auto; }
  .editor-host.expanded :global(.cm-editor) { height: auto; }
  .editor-host.expanded :global(.cm-scroller) { overflow: hidden; }
  .editor-host.collapsed { display: none; }
  .editor-host :global(.cm-editor) {
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-sm);
    line-height: 1.65;
  }
  .editor-host :global(.cm-editor .cm-scroller) { font-family: var(--mdc-mono); }
  .preview-loading {
    min-height: 9rem;
    display: grid;
    place-items: center;
    color: var(--mdc-code-dim);
    background: var(--mdc-code-bg);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
  }
  .preview-hidden { display: none; }
  .error-bar {
    padding: 0.45rem 0.65rem;
    background: color-mix(in srgb, var(--mdc-error) 10%, transparent);
    color: var(--mdc-code-error);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
    border-top: 1px solid color-mix(in srgb, var(--mdc-error) 25%, transparent);
  }

  /* Narrow blocks (the graph view's side pane, or a phone) shed text labels and
     keep only glyphs. Labels are removed from layout rather than made
     transparent, so the remaining icon stays centred instead of being clipped. */
  @container (max-width: 420px) {
    .btn-label {
      display: none;
    }
    .dirty,
    .preview-toggle,
    .save {
      gap: 0;
    }
    .preview-toggle,
    .save {
      width: 26px;
      padding: 0;
    }
  }
</style>
