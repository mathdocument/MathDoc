<script lang="ts">
  import { onDestroy } from "svelte";
  import { Check, FileText, Hash, Layers3, X } from "@lucide/svelte";
  import type { NodeDetail } from "../lib/types";
  import type { LoadState } from "../lib/state.svelte";
  import { shortFnode, errMsg } from "../lib/format";
  import BlockEditor from "./BlockEditor.svelte";
  import AddBlockControl from "./AddBlockControl.svelte";
  import { api } from "../lib/api";
  import {
    confirmDiscardDrafts,
    removeDraft,
    settlePendingMutations,
    setDraftDirty,
    setMutationPending,
    unsavedDraftRevision,
  } from "../lib/unsaved";

  interface Props {
    load: LoadState;
    onRefresh?: (node: NodeDetail) => void;
  }
  let { load, onRefresh }: Props = $props();

  // Inline title editing.
  let editingTitle = $state(false);
  let titleDraft = $state("");
  let titleError: string | null = $state(null);
  let titleSaving = $state(false);
  let titleInputEl = $state<HTMLInputElement | null>(null);
  let refreshError: string | null = $state(null);
  const titleDraftId = Symbol("title draft");
  const titleMutationId = Symbol("title mutation");
  let displayedFnode: string | null = null;
  let titleRequest = 0;
  let refreshRequest = 0;
  let editorResetRevision = $state(0);

  // Reset title editing state when the displayed node changes.
  $effect(() => {
    const fnode = load.kind === "ready" ? load.node.fnode : null;
    if (fnode === displayedFnode) return;
    displayedFnode = fnode;
    titleRequest++;
    refreshRequest++;
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
    titleRequest++;
    refreshRequest++;
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
    if (load.kind !== "ready") return;
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
    setMutationPending(titleMutationId, true);
    titleError = null;
    try {
      const updated = await api.putTitle(targetFnode, newTitle);
      if (!isCurrent() || load.kind !== "ready") return;
      // A title write returns the whole node, but unrelated block drafts may
      // be newer than the blocks in that response.
      onRefresh?.({ ...load.node, title: updated.title });
      editingTitle = false;
    } catch (e) {
      if (isCurrent()) titleError = errMsg(e);
    } finally {
      setMutationPending(titleMutationId, false);
      if (isCurrent()) titleSaving = false;
    }
  }

  function cancelEditTitle() {
    editingTitle = false;
    titleError = null;
  }

  async function refreshNode() {
    if (load.kind !== "ready") return;
    if (!confirmDiscardDrafts()) return;
    if (!await settlePendingMutations()) return;
    if (load.kind !== "ready") return;
    const confirmedDraftRevision = unsavedDraftRevision();
    const targetFnode = load.node.fnode;
    const request = ++refreshRequest;
    refreshError = null;
    try {
      const fresh = await api.node(targetFnode);
      if (request !== refreshRequest || load.kind !== "ready" ||
        load.node.fnode !== targetFnode) return;
      if (unsavedDraftRevision() !== confirmedDraftRevision) return;
      titleRequest++;
      editingTitle = false;
      titleSaving = false;
      titleError = null;
      titleDraft = fresh.title;
      editorResetRevision++;
      onRefresh?.(fresh);
    } catch (e) {
      if (request === refreshRequest) refreshError = errMsg(e);
    }
  }
</script>

<section
  class="center"
  aria-label="current node"
>
  {#if load.kind === "idle"}
    <div class="placeholder">no node selected</div>
  {:else if load.kind === "loading"}
    <div class="placeholder">loading…</div>
  {:else if load.kind === "error"}
    <div class="placeholder error">{load.message}</div>
  {:else}
    {@const node = load.node}
    <header class="head">
      {#if refreshError}
        <div class="refresh-error" role="alert">
          <span>refresh failed: {refreshError}</span>
          <button onclick={() => void refreshNode()}>retry</button>
        </div>
      {/if}
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
    </header>
    {#key `${node.fnode}:${editorResetRevision}`}
    <div class="blocks">
      {#if node.blocks.length === 0}
        <div class="placeholder">no source blocks</div>
      {:else}
        {#each node.blocks as block (block.srctype)}
          <BlockEditor
            fnode={node.fnode}
            {block}
            onDeleted={refreshNode}
          />
        {/each}
      {/if}
      {#key node.fnode}
        <AddBlockControl
          fnode={node.fnode}
          existingSrctypes={node.blocks.map((b) => b.srctype)}
          onAdded={refreshNode}
        />
      {/key}
    </div>
    {/key}
  {/if}
</section>

<style>
  .center {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: rgba(15, 21, 31, 0.72);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    box-shadow: 0 10px 35px rgba(0, 0, 0, 0.16);
  }
  .head {
    padding: 1.05rem 1.25rem 0.95rem;
    border-bottom: 1px solid var(--mdc-border);
    background: linear-gradient(180deg, rgba(20, 28, 40, 0.7), rgba(15, 21, 31, 0.2));
  }
  .refresh-error {
    display: flex;
    gap: 0.5rem;
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    margin-bottom: 0.5rem;
  }
  .refresh-error button {
    color: inherit;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
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
    color: var(--mdc-fg);
    background: var(--mdc-code-bg);
    border: 1px solid var(--mdc-accent);
    border-radius: var(--mdc-radius-sm);
    padding: 0.35rem 0.5rem;
    width: min(70%, 680px);
  }
  .title-input:focus {
    outline: none;
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
    color: #07110e;
    border-color: var(--mdc-accent-down);
  }
  .title-cancel:hover {
    background: var(--mdc-error);
    color: #16090c;
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
    background: rgba(9, 13, 20, 0.48);
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
</style>
