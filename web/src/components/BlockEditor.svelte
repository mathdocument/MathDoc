<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { EditorState } from "@codemirror/state";
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
  import { defaultKeymap, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { indentUnit } from "@codemirror/language";
  import type { Extension } from "@codemirror/state";
  import type { SrcBlock } from "../lib/types";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { shikiHighlight } from "../lib/cm-shiki";
  import { getHighlighter, srctypeToLang } from "../lib/shiki";

  interface Props {
    fnode: string;
    block: SrcBlock;
    onDeleted?: (srctype: string) => void;
  }
  let { fnode, block, onDeleted }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let editorView: EditorView | null = null;
  let dirty = $state(false);
  let saving = $state(false);
  let lastSavedContent = $state(block.content);
  let error: string | null = $state(null);
  let expanded = $state(true);
  let loading = $state(true);
  let shikiError: string | null = $state(null);
  let alive = false;

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
      keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
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
          dirty = u.state.doc.toString() !== lastSavedContent;
        }
      }),
    ];
  }

  onMount(() => {
    alive = true;
    getHighlighter()
      .then((hl) => {
        if (!alive || !host) { loading = false; return; }
        const lang = srctypeToLang(block.srctype);
        const shikiExt = shikiHighlight(hl, lang, SHIKI_THEME);
        editorView = new EditorView({
          doc: block.content,
          extensions: [...buildBaseExtensions(), shikiExt],
          parent: host,
        });
        loading = false;
      })
      .catch((e) => {
        shikiError = errMsg(e);
        if (!alive || !host) { loading = false; return; }
        editorView = new EditorView({
          doc: block.content,
          extensions: buildBaseExtensions(),
          parent: host,
        });
        loading = false;
      });
  });

  function save() {
    if (!editorView || !dirty || saving) return;
    saving = true;
    error = null;
    const content = editorView.state.doc.toString();
    api
      .putBlock(fnode, block.srctype, content)
      .then((node) => {
        const updated = node.blocks.find((b) => b.srctype === block.srctype);
        if (updated && editorView) {
          lastSavedContent = updated.content;
          if (editorView.state.doc.toString() !== updated.content) {
            editorView.dispatch({
              changes: { from: 0, to: editorView.state.doc.length, insert: updated.content },
            });
          }
          dirty = false;
        }
      })
      .catch((e: unknown) => { error = errMsg(e); })
      .finally(() => { saving = false; });
  }

  function onDelete() {
    if (!confirm(`Delete the ${block.srctype} block from this node?`)) return;
    error = null;
    api
      .deleteBlock(fnode, block.srctype)
      .then(() => { onDeleted?.(block.srctype); })
      .catch((e: unknown) => { error = errMsg(e); });
  }

  function toggleExpand() { expanded = !expanded; }

  onDestroy(() => {
    alive = false;
    editorView?.destroy();
    editorView = null;
  });

  // Update content when fnode or srctype changes.
  let prevFnode = fnode;
  let prevSrctype = block.srctype;
  $effect(() => {
    void fnode;
    void block.srctype;
    if (fnode === prevFnode && block.srctype === prevSrctype) return;
    prevFnode = fnode;
    prevSrctype = block.srctype;
    if (editorView) {
      editorView.dispatch({
        changes: { from: 0, to: editorView.state.doc.length, insert: block.content },
      });
      lastSavedContent = block.content;
      dirty = false;
    }
  });
</script>

<article class="block">
  <header class="block-head">
    <span class="srctype">@src: {block.srctype}</span>
    <span class="spacer"></span>
    {#if dirty}<span class="dirty" title="unsaved">●</span>{/if}
    {#if saving}<span class="saving">saving…</span>{/if}
    {#if error}<span class="error" title={error}>⚠</span>{/if}
    {#if shikiError}<span class="error" title={`highlight: ${shikiError}`}>⚡</span>{/if}
    <button class="icon-btn expand" onclick={toggleExpand} title={expanded ? "collapse" : "expand"}>
      {#if expanded}▾{:else}▸{/if}
    </button>
    <button class="save" onclick={save} disabled={!dirty || saving}>save</button>
    <button class="delete" onclick={onDelete} title="delete block">×</button>
  </header>
  <div class="editor-host" class:expanded class:collapsed={!expanded} bind:this={host}>
    {#if loading}
      <div class="loading">loading editor…</div>
    {/if}
  </div>
  {#if error}<div class="error-bar">{error}</div>{/if}
</article>

<style>
  .block {
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }
  .block-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.6rem;
    background: var(--mdc-panel);
    font-family: var(--mdc-mono);
    font-size: 0.8rem;
    color: var(--mdc-dim);
    border-bottom: 1px solid var(--mdc-border);
  }
  .srctype { color: var(--mdc-accent); font-weight: 600; }
  .spacer { flex: 1; }
  .dirty { color: var(--mdc-accent-down); font-size: 0.9rem; }
  .saving { color: var(--mdc-dim); font-size: 0.72rem; }
  .error { color: var(--mdc-error); cursor: help; }
  .save, .delete, .icon-btn {
    background: var(--mdc-card);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 3px;
    padding: 0.15rem 0.5rem;
    font-size: 0.72rem;
    cursor: pointer;
    font-family: inherit;
  }
  .save:disabled { opacity: 0.4; cursor: default; }
  .save:not(:disabled):hover {
    background: var(--mdc-accent); color: var(--mdc-bg); border-color: var(--mdc-accent);
  }
  .delete:hover {
    background: var(--mdc-error); color: var(--mdc-bg); border-color: var(--mdc-error);
  }
  .expand:hover { background: var(--mdc-card-hover); }
  .editor-host { background: var(--mdc-code-bg); }
  .editor-host.expanded { height: auto; }
  .editor-host.expanded :global(.cm-editor) { height: auto; }
  .editor-host.expanded :global(.cm-scroller) { overflow: hidden; }
  .editor-host.collapsed { display: none; }
  .editor-host :global(.cm-editor) {
    font-family: var(--mdc-mono); font-size: 0.82rem;
  }
  .editor-host :global(.cm-editor .cm-scroller) { font-family: var(--mdc-mono); }
  .loading {
    padding: 1rem; color: var(--mdc-dim); font-family: var(--mdc-mono); font-size: 0.8rem;
  }
  .error-bar {
    padding: 0.4rem 0.6rem;
    background: rgba(247, 118, 142, 0.12);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
    border-top: 1px solid var(--mdc-border);
  }
</style>
