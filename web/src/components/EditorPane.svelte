<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Check, FileText, Hash, Layers3, X } from "@lucide/svelte";
  import type { FormalCodeStatus, NodeDetail } from "../lib/types";
  import type { LoadState } from "../lib/state.svelte";
  import type { Theme } from "../lib/theme";
  import { shortFnode, errMsg } from "../lib/format";
  import AddBlockControl from "./AddBlockControl.svelte";
  import { api } from "../lib/api";
  import {
    removeDraft,
    setDraftDirty,
    setMutationPending,
  } from "../lib/unsaved";

  interface Props {
    load: LoadState;
    theme: Theme;
    active?: boolean;
    onRefresh?: (node: NodeDetail, graphChanged?: boolean) => void;
    onReady?: () => void;
  }
  let { load, theme, active = true, onRefresh, onReady }: Props = $props();

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

  const formalStatusLabels: Record<FormalCodeStatus, string> = {
    no_code: "No code",
    unverified: "Unverified",
    verified: "Verified",
  };

  function applySavedBlock(updated: NodeDetail) {
    if (load.kind !== "ready" || load.node.fnode !== updated.fnode) return;
    const updatedBlocks = new Map(updated.blocks.map((block) => [block.srctype, block]));
    onRefresh?.({
      ...load.node,
      revision: updated.revision,
      formalization: updated.formalization,
      blocks: load.node.blocks.map((block) => updatedBlocks.get(block.srctype) ?? block),
    });
  }

  function applyDeletedBlock(updated: NodeDetail, srctype: string) {
    if (load.kind !== "ready" || load.node.fnode !== updated.fnode) return;
    onRefresh?.({
      ...load.node,
      revision: updated.revision,
      formalization: updated.formalization,
      blocks: load.node.blocks.filter((block) => block.srctype !== srctype),
    });
  }

  function applyAddedBlock(updated: NodeDetail) {
    if (load.kind !== "ready" || load.node.fnode !== updated.fnode) return;
    const existing = new Set(load.node.blocks.map((block) => block.srctype));
    onRefresh?.({
      ...load.node,
      revision: updated.revision,
      formalization: updated.formalization,
      blocks: [
        ...load.node.blocks,
        ...updated.blocks.filter((block) => !existing.has(block.srctype)),
      ],
    });
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

  onMount(() => {
    if (load.kind !== "ready" || load.node.blocks.length === 0) {
      reportReady();
    }
  });

  $effect(() => {
    if (load.kind === "ready" && load.node.blocks.length > 0) ensureBlockEditorLoaded();
  });

  // Reset title editing state when the displayed node changes.
  $effect(() => {
    const fnode = load.kind === "ready" ? load.node.fnode : null;
    if (fnode === displayedFnode) return;
    displayedFnode = fnode;
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
    const mutationId = Symbol("title save");
    const isCurrent = () => request === titleRequest && load.kind === "ready" &&
      load.node.fnode === targetFnode;
    titleSaving = true;
    setMutationPending(mutationId, true);
    titleError = null;
    try {
      const updated = await api.putTitle(targetFnode, newTitle, load.node.revision);
      if (!isCurrent() || load.kind !== "ready") return;
      // A title write returns the whole node, but unrelated block drafts may
      // be newer than the blocks in that response.
      onRefresh?.({ ...load.node, title: updated.title, revision: updated.revision }, true);
      editingTitle = false;
    } catch (e) {
      if (isCurrent()) titleError = errMsg(e);
    } finally {
      setMutationPending(mutationId, false);
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
  {:else if load.kind === "loading"}
    <div class="skeleton" aria-busy="true" aria-label="loading node">
      <div class="sk sk-eyebrow"></div>
      <div class="sk sk-title"></div>
      <div class="sk sk-meta"></div>
      <div class="sk sk-block"></div>
      <div class="sk sk-block tall"></div>
    </div>
  {:else if load.kind === "error"}
    <div class="placeholder error">{load.message}</div>
  {:else}
    {@const node = load.node}
    <header class="head">
      <div class="eyebrow">Current node</div>
      <div class="title-row">
        {#if editingTitle}
          <input
            class="title-input"
            bind:this={titleInputEl}
            bind:value={titleDraft}
            onkeydown={(e) => {
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
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events, a11y_no_noninteractive_element_to_interactive_role -->
          <h1
            class="title"
            tabindex="0"
            role="button"
            onclick={startEditTitle}
            onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); startEditTitle(); } }}
            title="Click to rename"
          >{node.title}</h1>
        {/if}
      </div>
      <div class="meta">
        <code class="meta-item fnode" title={node.fnode}><Hash size={12} strokeWidth={1.8} />{shortFnode(node.fnode)}</code>
        <span class="meta-item path"><FileText size={12} strokeWidth={1.8} />{node.rel_path}</span>
        <span class="meta-item depth"><Layers3 size={12} strokeWidth={1.8} />Depth {node.depth}</span>
        {#if node.broken}<span class="meta-item broken"><X size={12} strokeWidth={2} />Broken</span>{/if}
      </div>
      <div class="formalization" aria-label="formal verification status">
        <span class="formalization-label">Formal</span>
        <span
          class="formal-status"
          data-status={node.formalization.lean}
          aria-label={`Lean: ${formalStatusLabels[node.formalization.lean]}`}
        >
          <span class="status-light" aria-hidden="true"></span>
          <span class="formal-language">Lean</span>
          <span class="status-text">{formalStatusLabels[node.formalization.lean]}</span>
        </span>
        <span
          class="formal-status"
          data-status={node.formalization.rocq}
          aria-label={`Rocq: ${formalStatusLabels[node.formalization.rocq]}`}
        >
          <span class="status-light" aria-hidden="true"></span>
          <span class="formal-language">Rocq</span>
          <span class="status-text">{formalStatusLabels[node.formalization.rocq]}</span>
        </span>
      </div>
    </header>
    <div class="blocks">
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
      {:else if !BlockEditorComponent}
        <div class="editor-loading" aria-busy="true">Loading editor…</div>
      {:else}
        {#each node.blocks as block (block.srctype)}
          <BlockEditorComponent
            fnode={node.fnode}
            revision={node.revision}
            {block}
            {theme}
            {active}
            onDeleted={applyDeletedBlock}
            onSaved={applySavedBlock}
            onReady={() => reportBlockReady(block.srctype)}
          />
        {/each}
      {/if}
      <AddBlockControl
        fnode={node.fnode}
        revision={node.revision}
        existingSrctypes={node.blocks.map((b) => b.srctype)}
        onAdded={applyAddedBlock}
      />
    </div>
  {/if}
</section>

<style>
  .center {
    flex: 1;
    min-width: 0;
    position: relative;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: color-mix(in srgb, var(--mdc-panel) 88%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    box-shadow: 0 10px 35px color-mix(in srgb, var(--mdc-fg) 12%, transparent);
  }
  .center::before {
    content: "";
    position: absolute;
    inset: 0 0 auto;
    height: 2px;
    background: linear-gradient(90deg, var(--mdc-accent) 0%, color-mix(in srgb, var(--mdc-accent) 12%, transparent) 46%, transparent 78%);
    border-radius: var(--mdc-radius-md) var(--mdc-radius-md) 0 0;
    opacity: 0.9;
    pointer-events: none;
  }
  .head {
    padding: 1.05rem 1.25rem 0.95rem;
    border-bottom: 1px solid var(--mdc-border);
    background: linear-gradient(180deg, color-mix(in srgb, var(--mdc-panel-raised) 82%, transparent), color-mix(in srgb, var(--mdc-panel) 35%, transparent));
  }
  .eyebrow {
    margin-bottom: 0.38rem;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.6rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .title-row {
    display: flex;
    align-items: center;
    min-height: 31px;
    gap: 0.35rem;
  }
  .title {
    margin: 0;
    color: var(--mdc-fg);
    font-size: 1.28rem;
    font-weight: 640;
    letter-spacing: -0.025em;
    line-height: 1.25;
    word-break: break-word;
    cursor: text;
    display: inline-block;
    border-radius: var(--mdc-radius-sm);
    padding: 0.15rem 0.3rem;
    margin: -0.15rem -0.3rem;
    transition: background 120ms ease;
  }
  .title:hover {
    background: var(--mdc-card-hover);
  }
  .title-input {
    font-size: 1.18rem;
    font-weight: 620;
    font-family: inherit;
    color: var(--mdc-code-fg);
    background: var(--mdc-code-bg);
    border: 1px solid var(--mdc-accent);
    border-radius: var(--mdc-radius-sm);
    padding: 0.35rem 0.5rem;
    width: min(70%, 680px);
  }
  .title-save,
  .title-cancel {
    display: inline-grid;
    place-items: center;
    width: 29px;
    height: 29px;
    padding: 0;
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    color: var(--mdc-fg);
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
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
    font-size: 0.72rem;
    margin-left: 0.5rem;
  }
  .meta {
    margin-top: 0.58rem;
    display: flex;
    gap: 0.42rem;
    align-items: center;
    font-size: 0.68rem;
    color: var(--mdc-muted);
    flex-wrap: wrap;
  }
  .meta-item {
    display: inline-flex;
    align-items: center;
    gap: 0.28rem;
    min-height: 22px;
    padding: 0 0.45rem;
    background: color-mix(in srgb, var(--mdc-bg) 55%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: 999px;
  }
  .fnode {
    font-family: var(--mdc-mono);
    color: var(--mdc-accent-strong);
  }
  .path {
    font-family: var(--mdc-mono);
  }
  .depth {
    font-variant-numeric: tabular-nums;
  }
  .broken {
    color: var(--mdc-error);
    font-weight: 600;
  }
  .formalization {
    margin-top: 0.5rem;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.64rem;
  }
  .formalization-label {
    margin-right: 0.08rem;
    color: var(--mdc-dim);
    font-size: 0.58rem;
    font-weight: 650;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .formal-status {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    min-height: 22px;
    padding: 0 0.48rem;
    background: color-mix(in srgb, var(--mdc-bg) 42%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: 999px;
  }
  .status-light {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
  }
  .formal-status[data-status="no_code"] .status-light {
    background: var(--mdc-error);
    box-shadow: 0 0 0 2px rgba(255, 125, 143, 0.12);
  }
  .formal-status[data-status="unverified"] .status-light {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 2px rgba(232, 184, 109, 0.12);
  }
  .formal-status[data-status="verified"] .status-light {
    background: var(--mdc-accent-down);
    box-shadow: 0 0 0 2px rgba(99, 216, 178, 0.12);
  }
  .formal-language {
    color: var(--mdc-fg-soft);
    font-weight: 650;
  }
  .status-text {
    color: var(--mdc-muted);
  }
  .blocks {
    flex: 1;
    overflow-y: auto;
    padding: 1rem 1.15rem 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
  }
  .placeholder {
    color: var(--mdc-muted);
    padding: 2rem;
    text-align: center;
  }
  .placeholder.error {
    color: var(--mdc-error);
  }
  /* Skeleton loading state (shown only while the very first node loads). */
  .skeleton {
    padding: 1.1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .sk {
    position: relative;
    overflow: hidden;
    border-radius: 6px;
    background: var(--mdc-card);
  }
  .sk::after {
    content: "";
    position: absolute;
    inset: 0;
    transform: translateX(-100%);
    background: linear-gradient(90deg, transparent, color-mix(in srgb, var(--mdc-accent) 9%, transparent), transparent);
    animation: mdc-shimmer 1.4s infinite;
  }
  .sk-eyebrow { width: 4.5rem; height: 0.6rem; }
  .sk-title { width: 55%; height: 1.45rem; border-radius: 8px; }
  .sk-meta { width: 40%; height: 0.85rem; }
  .sk-block { width: 100%; height: 7rem; border-radius: var(--mdc-radius-md); }
  .sk-block.tall { height: 12rem; }
  @keyframes mdc-shimmer {
    to { transform: translateX(100%); }
  }
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.35rem;
    padding: 2.4rem 1rem;
    color: var(--mdc-muted);
    text-align: center;
  }
  .empty-state strong {
    color: var(--mdc-fg-soft);
    font-size: 0.86rem;
    font-weight: 600;
  }
  .empty-state p {
    margin: 0;
    font-size: 0.72rem;
  }
  .editor-loading,
  .editor-load-error {
    min-height: 8rem;
    display: grid;
    place-items: center;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.72rem;
  }
  .editor-load-error { color: var(--mdc-error); }
  .editor-load-error button {
    color: inherit;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
  }
  .empty-icon {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    margin-bottom: 0.3rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    border-radius: 12px;
  }
</style>
