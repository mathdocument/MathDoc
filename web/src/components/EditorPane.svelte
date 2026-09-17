<script lang="ts">
  import { onDestroy } from "svelte";
  import { Check, FileText, Hash, Layers3, X } from "@lucide/svelte";
  import { SOURCE_TYPES, type NodeDetail } from "../lib/types";
  import type { LoadState } from "../lib/state.svelte";
  import type { Theme } from "../lib/theme";
  import { errMsg, shortFnode } from "../lib/format";
  import FormalStatus from "./FormalStatus.svelte";
  import AddBlockControl from "./AddBlockControl.svelte";
  import LeanBlock from "./LeanBlock.svelte";
  import { api } from "../lib/api";
  import {
    removeDraft,
    setDraftDirty,
    trackMutation,
  } from "../lib/unsaved";

  interface Props {
    load: LoadState;
    theme: Theme;
    active?: boolean;
    selection?: number;
    onRefresh?: (node: NodeDetail, graphChanged?: boolean) => void;
    onReady?: () => void;
    latexTarget?: {fnode: string; label: string} | null;
    onLatexNavigate?: (fnode: string, label: string) => void;
  }
  let { load, theme, active = true, selection = 0, onRefresh, onReady, latexTarget, onLatexNavigate }: Props = $props();
  let node = $derived(load.kind === "ready" ? load.node : null);

  // Inline title editing.
  let editingTitle = $state(false);
  let titleDraft = $state("");
  let titleError: string | null = $state(null);
  let titleSaving = $state(false);
  let titleInputEl = $state<HTMLInputElement | null>(null);
  const titleDraftId = Symbol("title draft");
  let displayedFnode: string | null = null;
  let titleRequest = 0;
  let readyReported = false;
  const readyBlocks = new Set<string>();
  let BlockEditorComponent = $state<typeof import("./BlockEditor.svelte").default | null>(null);
  let blockEditorPromise: Promise<typeof import("./BlockEditor.svelte").default> | null = null;
  let editorLoadError: string | null = $state(null);
  let alive = true;

  function applyBlockUpdate(updated: NodeDetail, graphChanged = false) {
    if (load.kind !== "ready" || load.node.fnode !== updated.fnode) return;
    onRefresh?.(updated, graphChanged);
  }

  function reportReady() {
    if (readyReported) return;
    readyReported = true;
    onReady?.();
  }

  function reportBlockReady(srctype: string) {
    if (load.kind !== "ready" || readyReported) return;
    readyBlocks.add(srctype);
    if (readyBlocks.size === load.node.blocks.length) reportReady();
  }

  function ensureBlockEditorLoaded() {
    if (BlockEditorComponent || blockEditorPromise) return;
    editorLoadError = null;
    blockEditorPromise = import("./BlockEditor.svelte").then((module) => module.default);
    void blockEditorPromise.then((component) => {
      if (alive) BlockEditorComponent = component;
    }).catch((error) => {
      if (!alive) return;
      editorLoadError = errMsg(error);
      reportReady();
    }).finally(() => {
      blockEditorPromise = null;
    });
  }

  $effect(() => {
    if (load.kind === "error" || (load.kind === "ready" && load.node.blocks.length === 0)) {
      reportReady();
    }
  });

  $effect(() => {
    if (load.kind === "ready" && load.node.blocks.some(block => block.srctype !== "lean")) ensureBlockEditorLoaded();
  });

  // Reset title editing state when the displayed node changes.
  $effect.pre(() => {
    const fnode = load.kind === "ready" ? load.node.fnode : null;
    if (fnode === displayedFnode) return;
    displayedFnode = fnode;
    readyReported = false;
    readyBlocks.clear();
    titleRequest++;
    editingTitle = false;
    titleSaving = false;
    titleError = null;
  });

  $effect(() => {
    const isDirty = editingTitle && load.kind === "ready" &&
      titleDraft !== load.node.title;
    setDraftDirty(titleDraftId, isDirty);
  });

  onDestroy(() => {
    alive = false;
    titleRequest++;
    removeDraft(titleDraftId);
  });

  // Refocus input when entering edit mode.
  $effect(() => {
    if (editingTitle) titleInputEl?.focus();
  });

  function startEditTitle() {
    if (load.kind !== "ready") return;
    titleDraft = load.node.title;
    editingTitle = true;
  }

  async function saveTitle() {
    if (load.kind !== "ready" || titleSaving) return;
    const newTitle = titleDraft.trim();
    if (!newTitle) {
      titleError = "title must be non-empty";
      return;
    }
    if (newTitle === load.node.title) {
      editingTitle = false;
      titleError = null;
      return;
    }
    const targetFnode = load.node.fnode;
    const request = ++titleRequest;
    const isCurrent = () => request === titleRequest && load.kind === "ready" &&
      load.node.fnode === targetFnode;
    titleSaving = true;
    const clearMutation = trackMutation();
    titleError = null;
    try {
      const updated = await api.putTitle(targetFnode, newTitle, load.node.revision);
      if (!isCurrent() || load.kind !== "ready") return;
      onRefresh?.(updated, true);
      editingTitle = false;
    } catch (e) {
      if (isCurrent()) titleError = errMsg(e);
    } finally {
      clearMutation();
      if (isCurrent()) titleSaving = false;
    }
  }

  function cancelEditTitle() {
    editingTitle = false;
    titleError = null;
  }

</script>

<section
  class="center"
  aria-label="current node"
>
  {#if load.kind === "idle"}
    <div class="placeholder">no node selected</div>
  {:else if load.kind === "error"}
    <div class="placeholder error">{load.message}</div>
  {:else if node}
    <header class="head">
      <div class="title-row">
        {#if editingTitle}
          <input
            class="title-input"
            aria-label="Node title"
            bind:this={titleInputEl}
            bind:value={titleDraft}
            onkeydown={(e) => {
              if (e.isComposing) return;
              if (e.key === "Enter") { e.preventDefault(); void saveTitle(); }
              else if (e.key === "Escape") { e.preventDefault(); cancelEditTitle(); }
            }}
            disabled={titleSaving}
          />
          <button class="title-save" onclick={saveTitle} disabled={titleSaving} title="Save title" aria-label="Save title">
            <Check size={15} strokeWidth={2} />
          </button>
          <button class="title-cancel" onclick={cancelEditTitle} disabled={titleSaving} title="Cancel rename" aria-label="Cancel rename">
            <X size={15} strokeWidth={2} />
          </button>
          {#if titleError}<span class="title-error">{titleError}</span>{/if}
        {:else}
          <h1 class="title">
            <button onclick={startEditTitle} title="Click to rename">{node.title}</button>
          </h1>
        {/if}
      </div>
      <!-- One metadata line: identity, depth, verification. Separated
           by rhythm and colour rather than by an outline around each value. -->
      <div class="meta" aria-label="node metadata">
        <code class="meta-item fnode" title={node.fnode}><Hash size={12} strokeWidth={2} />{shortFnode(node.fnode)}</code>
        <span class="meta-item depth"><Layers3 size={12} strokeWidth={1.8} />Depth {node.depth}</span>
        {#if node.broken}<span class="meta-item broken"><X size={12} strokeWidth={2.2} />Broken</span>{/if}
        <span class="meta-sep" aria-hidden="true"></span>
        <FormalStatus language="Lean" status={node.formalization.lean} />
        <FormalStatus language="Rocq" status={node.formalization.rocq} />
      </div>
    </header>
  {/if}
    <div class="blocks" class:hidden={!node}>
      {#if node}
      {#if node.blocks.length === 0}
        <div class="empty-state">
          <span class="empty-icon"><FileText size={20} strokeWidth={1.6} /></span>
          <strong>No source blocks yet</strong>
          <p>Attach code, LaTeX, or prose with “Add source block” below.</p>
        </div>
      {:else if editorLoadError}
        <div class="editor-load-error" role="alert">
          <span>editor failed to load: {editorLoadError}</span>
          <button onclick={ensureBlockEditorLoaded}>retry</button>
        </div>
      {:else if !BlockEditorComponent && node.blocks.some(block => block.srctype !== "lean")}
        <div class="editor-loading" aria-busy="true">Loading editor…</div>
      {/if}
      {/if}
      {#each SOURCE_TYPES as srctype (srctype)}
        {@const block = node?.blocks.find(block => block.srctype === srctype)}
        {#if srctype === "lean"}
          <!-- Keep Lean mounted across node changes to preserve its editor runtime. -->
          <LeanBlock fnode={node?.fnode ?? ""} revision={node?.revision ?? ""} module={node?.module} {block} {theme} {active} {selection}
            onDeleted={(updated) => applyBlockUpdate(updated, true)} onSaved={(updated) => applyBlockUpdate(updated, true)} onReady={() => reportBlockReady("lean")} />
        {:else if node && block && BlockEditorComponent && !editorLoadError}
          {#key `${node.fnode}:${srctype}`}
          <BlockEditorComponent
            fnode={node.fnode}
            revision={node.revision}
            {block}
            {theme}
            {active}
            focusLabel={latexTarget?.fnode === node.fnode ? latexTarget.label : undefined}
            {onLatexNavigate}
            onDeleted={(updated) => applyBlockUpdate(updated)}
            onSaved={applyBlockUpdate}
            onReady={() => reportBlockReady(block.srctype)}
          />
          {/key}
        {/if}
      {/each}
      {#if node}
      {#key node.fnode}
      <AddBlockControl
        fnode={node.fnode}
        revision={node.revision}
        existingSrctypes={node.blocks.map((b) => b.srctype)}
        onAdded={(updated) => applyBlockUpdate(updated, true)}
      />
      {/key}
      {/if}
    </div>
</section>

<style>
  .hidden { display:none !important; }
  .center {
    flex: 1;
    min-width: 0;
    position: relative;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--mdc-panel);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-lg);
    box-shadow: var(--mdc-shadow-lg);
  }
  /* The focused surface earns a hairline of accent along its top edge; that is
     the whole "you are here" signal, no eyebrow label needed. */
  .center::before {
    content: "";
    position: absolute;
    inset: 0 0 auto;
    height: 2px;
    background: linear-gradient(
      90deg,
      var(--mdc-accent) 0%,
      color-mix(in srgb, var(--mdc-accent) 30%, transparent) 34%,
      transparent 70%
    );
    pointer-events: none;
    z-index: 1;
  }
  .head {
    flex-shrink: 0;
    padding: 1.25rem 1.25rem 1rem;
    border-bottom: 1px solid var(--mdc-border);
  }
  .title-row {
    display: flex;
    align-items: center;
    min-height: 34px;
    gap: 0.4rem;
  }
  .title {
    color: var(--mdc-fg);
    font-size: var(--mdc-text-xl);
    font-weight: 660;
    letter-spacing: -0.03em;
    line-height: 1.2;
    word-break: break-word;
    cursor: text;
    display: inline-block;
    border-radius: var(--mdc-radius-sm);
    padding: 0.15rem 0.35rem;
    margin: -0.15rem -0.35rem;
    transition: background var(--mdc-dur-fast) var(--mdc-ease);
  }
  .title:hover {
    background: var(--mdc-card-hover);
  }
  .title button {
    padding: 0;
    color: inherit;
    background: none;
    border: 0;
    font: inherit;
    letter-spacing: inherit;
    text-align: left;
    cursor: text;
  }
  .title-input {
    font-size: 1.2rem;
    font-weight: 620;
    font-family: inherit;
    letter-spacing: -0.025em;
    color: var(--mdc-fg);
    background: var(--mdc-bg);
    border: 1px solid var(--mdc-accent);
    border-radius: var(--mdc-radius-sm);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-ring) 35%, transparent);
    padding: 0.3rem 0.55rem;
    width: min(70%, 680px);
  }
  .title-input:focus-visible {
    outline: none;
  }
  .title-save,
  .title-cancel {
    display: inline-grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    color: var(--mdc-fg-soft);
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
    transition: background var(--mdc-dur-fast) var(--mdc-ease),
      color var(--mdc-dur-fast) var(--mdc-ease),
      border-color var(--mdc-dur-fast) var(--mdc-ease);
  }
  .title-save:hover {
    background: var(--mdc-accent-down);
    color: var(--mdc-on-success);
    border-color: var(--mdc-accent-down);
  }
  .title-cancel:hover {
    background: var(--mdc-error);
    color: var(--mdc-on-error);
    border-color: var(--mdc-error);
  }
  .title-error {
    color: var(--mdc-error);
    font-size: var(--mdc-text-xs);
    margin-left: 0.5rem;
  }
  .meta {
    margin-top: 0.65rem;
    display: flex;
    gap: 0.85rem;
    align-items: center;
    font-size: var(--mdc-text-xs);
    color: var(--mdc-muted);
    flex-wrap: wrap;
  }
  .meta-item {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
  }
  .meta-sep {
    width: 1px;
    height: 11px;
    background: var(--mdc-border-strong);
  }
  .fnode {
    font-family: var(--mdc-mono);
    color: var(--mdc-accent);
    font-size: var(--mdc-text-xs);
  }
  .depth {
    font-variant-numeric: tabular-nums;
  }
  .broken {
    color: var(--mdc-error);
    font-weight: 620;
  }
  .blocks {
    position: relative;
    flex: 1;
    min-height: 0;
    /* Blocks use this visible content height, including in the graph sidebar. */
    container-type: size;
    overflow-y: auto;
    scrollbar-width: none;
    /* End the scroll chain here, including gestures that start at an edge. */
    overscroll-behavior-y: contain;
    padding: 1rem 1.25rem 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .blocks::-webkit-scrollbar { display: none; }
  /* WebKit needs an actual scroll range to rubber-band short or collapsed content. */
  .blocks::after {
    content: "";
    position: absolute;
    top: 100%;
    width: 1px;
    height: 1px;
    pointer-events: none;
  }
  .placeholder {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--mdc-muted);
    padding: 2rem;
    text-align: center;
    font-size: var(--mdc-text-sm);
  }
  .placeholder.error {
    color: var(--mdc-error);
  }
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.4rem;
    padding: 3rem 1rem;
    color: var(--mdc-muted);
    text-align: center;
  }
  .empty-state strong {
    color: var(--mdc-fg);
    font-size: var(--mdc-text-md);
    font-weight: 620;
    letter-spacing: var(--mdc-tracking-tight);
  }
  .empty-state p {
    margin: 0;
    max-width: 34ch;
    font-size: var(--mdc-text-sm);
    line-height: 1.55;
  }
  .editor-loading,
  .editor-load-error {
    min-height: 8rem;
    display: grid;
    place-items: center;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
  }
  .editor-load-error {
    gap: 0.6rem;
    color: var(--mdc-error);
  }
  .editor-load-error button {
    min-height: 26px;
    padding: 0 0.6rem;
    color: inherit;
    background: color-mix(in srgb, var(--mdc-error) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--mdc-error) 40%, transparent);
    border-radius: 6px;
    cursor: pointer;
  }
  .empty-icon {
    display: grid;
    place-items: center;
    width: 46px;
    height: 46px;
    margin-bottom: 0.5rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border-radius: var(--mdc-radius-md);
  }

  @media (max-width: 900px) {
    .head {
      padding: 1rem 1rem 0.75rem;
    }
    .blocks {
      padding: 0.75rem 0.75rem 1.25rem;
    }
  }
</style>
