<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    AlertTriangle,
    ChevronDown,
    ChevronRight,
    Save as SaveIcon,
    Trash2,
    Zap,
  } from "@lucide/svelte";
  import { Compartment, EditorState } from "@codemirror/state";
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
  import type { Extension } from "@codemirror/state";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { shikiHighlight } from "../lib/cm-shiki";
  import { getHighlighter, srctypeToLang } from "../lib/shiki";
  import { removeDraft, setDraftDirty, setMutationPending } from "../lib/unsaved";

  interface Props {
    fnode: string;
    revision: string;
    block: SrcBlock;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void;
    onReady?: () => void;
  }
  let { fnode, revision, block, onDeleted, onSaved, onReady }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let editorView: EditorView | null = null;
  let editorExtensions: Extension[] = [];
  let dirty = $state(false);
  let saving = $state(false);
  let deleting = $state(false);
  let lastSavedContent = $state("");
  let error: string | null = $state(null);
  let expanded = $state(true);
  let shikiError: string | null = $state(null);
  let alive = false;
  const draftId = Symbol("block draft");
  const mutationId = Symbol("block mutation");
  let identityVersion = 0;
  let prevFnode: string | null = null;
  let prevSrctype: string | null = null;
  let prevContent: string | null = null;
  let pendingSaveContent: string | null = null;
  const syntaxCompartment = new Compartment();
  let syntaxExtension: Extension = [];
  let readyReported = false;

  const SHIKI_THEME = "tokyo-night";

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
        ".cm-content": { caretColor: "var(--mdc-accent)" },
        ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--mdc-accent)" },
        "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
          backgroundColor: "rgba(122, 162, 247, 0.25) !important",
        },
        ".cm-gutters": {
          backgroundColor: "var(--mdc-code-bg)",
          borderRight: "1px solid var(--mdc-border)",
          color: "var(--mdc-dim)",
        },
        ".cm-activeLine": { backgroundColor: "rgba(122, 162, 247, 0.06)" },
        ".cm-activeLineGutter": { backgroundColor: "rgba(122, 162, 247, 0.06)" },
      }),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          setDirty(pendingSaveContent !== null || u.state.doc.toString() !== lastSavedContent);
        }
      }),
    ];
  }

  function buildEditorExtensions(): Extension[] {
    return [...buildBaseExtensions(), syntaxCompartment.of(syntaxExtension)];
  }

  function reportReadyAfterMeasure(view: EditorView) {
    view.requestMeasure({
      read: () => null,
      write: () => {
        if (!alive || editorView !== view || readyReported) return;
        readyReported = true;
        onReady?.();
      },
    });
  }

  function setDirty(value: boolean) {
    dirty = value;
    setDraftDirty(draftId, value);
  }

  onMount(() => {
    alive = true;
    // Effects may run before or after onMount. Seed the identity before the
    // synchronous view creation so initial mount never replaces its own doc.
    prevFnode = fnode;
    prevSrctype = block.srctype;
    prevContent = block.content;
    lastSavedContent = block.content;
    editorExtensions = buildEditorExtensions();
    editorView = new EditorView({
      doc: block.content,
      extensions: editorExtensions,
      parent: host!,
    });
    reportReadyAfterMeasure(editorView);

    getHighlighter()
      .then((hl) => {
        if (!alive || !editorView) return;
        const lang = srctypeToLang(block.srctype);
        syntaxExtension = shikiHighlight(hl, lang, SHIKI_THEME, (error) => {
          if (alive) shikiError = errMsg(error);
        });
        editorExtensions = buildEditorExtensions();
        editorView.dispatch({
          effects: syntaxCompartment.reconfigure(syntaxExtension),
        });
      })
      .catch((e) => {
        shikiError = errMsg(e);
      });
  });

  async function save() {
    if (!editorView || !dirty || saving || deleting) return;
    const targetFnode = fnode;
    const targetSrctype = block.srctype;
    const targetRevision = revision;
    const requestIdentity = identityVersion;
    saving = true;
    setMutationPending(mutationId, true);
    error = null;
    const content = editorView.state.doc.toString();
    pendingSaveContent = content;
    setDirty(true);
    const isCurrent = () => alive && requestIdentity === identityVersion &&
      fnode === targetFnode && block.srctype === targetSrctype;

    try {
      const node = await api.putBlock(targetFnode, targetSrctype, content, targetRevision);
      if (!isCurrent() || !editorView) return;
      const updated = node.blocks.find((b) => b.srctype === targetSrctype);
      if (!updated) {
        error = `saved response is missing the ${targetSrctype} block`;
        return;
      }

      onSaved?.(node);
      lastSavedContent = updated.content;
      // A response may normalize the submitted text, but it must never replace
      // edits made while that request was in flight.
      if (editorView.state.doc.toString() === content) {
        if (content !== updated.content) {
          editorView.dispatch({
            changes: { from: 0, to: editorView.state.doc.length, insert: updated.content },
          });
        }
      }
    } catch (e) {
      if (isCurrent()) error = errMsg(e);
    } finally {
      setMutationPending(mutationId, false);
      if (isCurrent()) {
        pendingSaveContent = null;
        if (editorView) setDirty(editorView.state.doc.toString() !== lastSavedContent);
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
    const requestIdentity = identityVersion;
    error = null;
    deleting = true;
    let pending = true;
    setMutationPending(mutationId, true);
    const isCurrent = () => alive && requestIdentity === identityVersion &&
      fnode === targetFnode && block.srctype === targetSrctype;
    try {
      const node = await api.deleteBlock(targetFnode, targetSrctype, targetRevision);
      if (!isCurrent()) return;
      setDirty(false);
      setMutationPending(mutationId, false);
      pending = false;
      onDeleted?.(node, targetSrctype);
    } catch (e) {
      if (isCurrent()) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (isCurrent()) deleting = false;
    }
  }

  function toggleExpand() { expanded = !expanded; }

  onDestroy(() => {
    alive = false;
    removeDraft(draftId);
    editorView?.destroy();
    editorView = null;
  });

  // Update content when fnode, srctype, or content changes (e.g. external
  // edit picked up by the refresh button).
  $effect(() => {
    const nextFnode = fnode;
    const nextSrctype = block.srctype;
    const nextContent = block.content;
    if (nextFnode === prevFnode && nextSrctype === prevSrctype && nextContent === prevContent) return;
    const identityChanged = prevFnode !== null &&
      (nextFnode !== prevFnode || nextSrctype !== prevSrctype);
    prevFnode = nextFnode;
    prevSrctype = nextSrctype;
    prevContent = nextContent;
    identityVersion++;
    pendingSaveContent = null;
    if (identityChanged) {
      saving = false;
      deleting = false;
    }
    error = null;
    lastSavedContent = nextContent;
    if (editorView) {
      if (identityChanged) {
        // A keyed block component may be reused for the same srctype on a new
        // node. Reset state so undo history cannot cross that node boundary.
        editorView.setState(EditorState.create({
          doc: nextContent,
          extensions: editorExtensions,
        }));
      } else if (!dirty && editorView.state.doc.toString() !== nextContent) {
        editorView.dispatch({
          changes: { from: 0, to: editorView.state.doc.length, insert: nextContent },
        });
      }
    }
    setDirty(editorView?.state.doc.toString() !== lastSavedContent);
  });
</script>

<article class="block" data-srctype={block.srctype}>
  <header class="block-head">
    <span class="srctype">{block.srctype}</span>
    <span class="block-kind">Source block</span>
    <span class="spacer"></span>
    {#if dirty}<span class="dirty" title="Unsaved changes"><span></span>Unsaved</span>{/if}
    {#if saving}<span class="saving">saving…</span>{/if}
    {#if deleting}<span class="saving">deleting…</span>{/if}
    {#if error}<span class="error" title={error}><AlertTriangle size={14} strokeWidth={1.9} /></span>{/if}
    {#if shikiError}<span class="error" title={`highlight: ${shikiError}`}><Zap size={14} strokeWidth={1.9} /></span>{/if}
    <button class="icon-btn expand" onclick={toggleExpand} title={expanded ? "Collapse" : "Expand"} aria-label={expanded ? "Collapse block" : "Expand block"}>
      {#if expanded}<ChevronDown size={15} strokeWidth={1.9} />{:else}<ChevronRight size={15} strokeWidth={1.9} />{/if}
    </button>
    <button class="save" onclick={save} disabled={!dirty || saving || deleting} title="Save (Ctrl/⌘+S)"><SaveIcon size={13} strokeWidth={1.9} />Save</button>
    <button class="delete" onclick={onDelete} disabled={saving || deleting} title="Delete block" aria-label="Delete block"><Trash2 size={14} strokeWidth={1.8} /></button>
  </header>
  <div class="editor-host" class:expanded class:collapsed={!expanded} bind:this={host}>
  </div>
  {#if error}<div class="error-bar">{error}</div>{/if}
</article>

<style>
  .block {
    --block-accent: var(--mdc-accent);
    position: relative;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    background: var(--mdc-code-bg);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
    container-type: inline-size;
  }
  /* Per-srctype accent so block kinds are scannable at a glance. */
  .block[data-srctype="text"] { --block-accent: var(--mdc-accent); }
  .block[data-srctype="latex"] { --block-accent: var(--mdc-accent-up); }
  .block[data-srctype="python"] { --block-accent: var(--mdc-warning); }
  .block[data-srctype="lean"] { --block-accent: #ff9e64; }
  .block[data-srctype="rocq"] { --block-accent: var(--mdc-accent-down); }
  .block::before {
    content: "";
    position: absolute;
    inset: 0 auto 0 0;
    width: 3px;
    background: var(--block-accent);
    opacity: 0.85;
  }
  .block-head {
    display: flex;
    align-items: center;
    gap: 0.48rem;
    min-height: 42px;
    padding: 0.4rem 0.55rem 0.4rem 0.68rem;
    background: linear-gradient(180deg, var(--mdc-panel-raised), color-mix(in srgb, var(--mdc-panel-raised) 82%, var(--mdc-bg)));
    font-size: 0.72rem;
    color: var(--mdc-muted);
    border-bottom: 1px solid var(--mdc-border);
  }
  .srctype {
    min-width: 52px;
    padding: 0.22rem 0.45rem;
    color: var(--block-accent);
    background: color-mix(in srgb, var(--block-accent) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--block-accent) 26%, transparent);
    border-radius: 5px;
    font-family: var(--mdc-mono);
    font-size: 0.65rem;
    font-weight: 650;
    text-align: center;
    text-transform: uppercase;
  }
  .block-kind {
    color: var(--mdc-muted);
    font-size: 0.68rem;
  }
  .spacer { flex: 1; }
  .dirty {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    color: var(--mdc-accent-down);
    font-size: 0.64rem;
  }
  .dirty span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: currentColor;
  }
  .saving { color: var(--mdc-muted); font-family: var(--mdc-mono); font-size: 0.64rem; }
  .error { display: inline-flex; color: var(--mdc-error); cursor: help; }
  .save, .delete, .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.32rem;
    min-height: 27px;
    background: transparent;
    color: var(--mdc-fg-soft);
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    padding: 0 0.5rem;
    font-size: 0.66rem;
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms ease, border-color 120ms ease, color 120ms ease;
  }
  .icon-btn,
  .delete {
    width: 27px;
    padding: 0;
  }
  .save:disabled { opacity: 0.32; cursor: default; }
  .save:not(:disabled) {
    color: var(--block-accent);
    background: color-mix(in srgb, var(--block-accent) 10%, transparent);
    border-color: color-mix(in srgb, var(--block-accent) 30%, transparent);
  }
  .save:not(:disabled):hover {
    background: color-mix(in srgb, var(--block-accent) 18%, transparent);
    color: var(--block-accent);
    border-color: color-mix(in srgb, var(--block-accent) 45%, transparent);
  }
  .delete:hover {
    background: rgba(255, 125, 143, 0.12); color: var(--mdc-error); border-color: rgba(255, 125, 143, 0.2);
  }
  .expand:hover { background: var(--mdc-card-hover); border-color: var(--mdc-border); }
  .editor-host { background: var(--mdc-code-bg); }
  .editor-host.expanded { height: auto; }
  .editor-host.expanded :global(.cm-editor) { height: auto; }
  .editor-host.expanded :global(.cm-scroller) { overflow: hidden; }
  .editor-host.collapsed { display: none; }
  .editor-host :global(.cm-editor) {
    font-family: var(--mdc-mono); font-size: 0.8rem;
  }
  .editor-host :global(.cm-editor .cm-scroller) { font-family: var(--mdc-mono); }
  .error-bar {
    padding: 0.4rem 0.6rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    border-top: 1px solid var(--mdc-border);
  }

  @container (max-width: 420px) {
    .block-kind {
      display: none;
    }
    .dirty {
      gap: 0;
      width: 9px;
      overflow: hidden;
      color: transparent;
    }
    .dirty span {
      flex: 0 0 auto;
      color: var(--mdc-accent-down);
    }
    .save {
      width: 27px;
      padding: 0;
      overflow: hidden;
      color: transparent;
      gap: 0;
    }
    .save :global(svg) {
      flex: 0 0 auto;
      color: var(--mdc-fg-soft);
    }
  }
</style>
