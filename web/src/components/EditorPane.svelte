<script lang="ts">
  import { onDestroy } from "svelte";
  import type { NodeDetail } from "../lib/types";
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
    load:
      | { kind: "idle" }
      | { kind: "loading" }
      | { kind: "ready"; node: NodeDetail }
      | { kind: "error"; message: string };
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
        <button class="title-save" onclick={saveTitle} disabled={titleSaving}>✓</button>
        <button class="title-cancel" onclick={cancelEditTitle} disabled={titleSaving}>×</button>
        {#if titleError}<span class="title-error">{titleError}</span>{/if}
      {:else}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events, a11y_no_noninteractive_element_to_interactive_role -->
        <h1
          class="title"
          tabindex="0"
          role="button"
          onclick={startEditTitle}
          onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); startEditTitle(); } }}
          title="click to rename"
        >{node.title}</h1>
      {/if}
      <div class="meta">
        <code class="fnode" title={node.fnode}>{shortFnode(node.fnode)}</code>
        <span class="path">{node.rel_path}</span>
        <span class="depth">depth {node.depth}</span>
        {#if node.broken}<span class="broken">✗ broken</span>{/if}
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
      <AddBlockControl
        fnode={node.fnode}
        existingSrctypes={node.blocks.map((b) => b.srctype)}
        onAdded={refreshNode}
      />
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
    background: var(--mdc-bg);
    border: 1px solid var(--mdc-border);
    border-radius: 6px;
  }
  .head {
    padding: 0.9rem 1.1rem;
    border-bottom: 1px solid var(--mdc-border);
  }
  .refresh-error {
    display: flex;
    gap: 0.5rem;
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.76rem;
    margin-bottom: 0.5rem;
  }
  .refresh-error button {
    color: inherit;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: 3px;
    cursor: pointer;
  }
  .title {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 600;
    word-break: break-word;
    cursor: text;
    display: inline-block;
    border-radius: 3px;
    padding: 0.1rem 0.3rem;
    margin: -0.1rem -0.3rem;
  }
  .title:hover {
    background: var(--mdc-card-hover);
  }
  .title-input {
    font-size: 1.15rem;
    font-weight: 600;
    font-family: inherit;
    color: var(--mdc-fg);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-accent);
    border-radius: 3px;
    padding: 0.1rem 0.3rem;
    width: 70%;
  }
  .title-input:focus {
    outline: none;
  }
  .title-save,
  .title-cancel {
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    color: var(--mdc-fg);
    border-radius: 3px;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
    margin-left: 0.3rem;
  }
  .title-save:hover {
    background: var(--mdc-accent-down);
    color: var(--mdc-bg);
    border-color: var(--mdc-accent-down);
  }
  .title-cancel:hover {
    background: var(--mdc-error);
    color: var(--mdc-bg);
    border-color: var(--mdc-error);
  }
  .title-error {
    color: var(--mdc-error);
    font-size: 0.78rem;
    margin-left: 0.5rem;
  }
  .meta {
    margin-top: 0.3rem;
    display: flex;
    gap: 0.8rem;
    align-items: baseline;
    font-size: 0.78rem;
    color: var(--mdc-dim);
    flex-wrap: wrap;
  }
  .fnode {
    font-family: var(--mdc-mono);
    color: var(--mdc-accent);
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
    padding: 0.9rem 1.1rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  .placeholder {
    color: var(--mdc-dim);
    padding: 2rem;
    text-align: center;
  }
  .placeholder.error {
    color: var(--mdc-error);
  }
</style>
