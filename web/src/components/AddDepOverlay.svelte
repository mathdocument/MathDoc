<script lang="ts">
  import { onDestroy } from "svelte";
  import { Link2, Plus, X } from "@lucide/svelte";
  import { api, isAbortError } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { DependencyCandidatesEmpty, NodeDetail, NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";
  import { modal } from "../lib/modal";
  import {
    confirmDiscardDraft,
    removeDraft,
    setDraftDirty,
    setMutationPending,
  } from "../lib/unsaved";

  interface Props {
    disabled: boolean;
    targetFnode: string;
    targetRevision: string;
    onAdded: (node: NodeDetail, delta: { nodes: number; edges: number }) => void;
    onClose: () => void;
  }
  let { disabled, targetFnode, targetRevision, onAdded, onClose }: Props = $props();

  let query = $state("");
  let results = $state<NodeInfo[]>([]);
  let selected = $state(0);
  let loading = $state(false);
  let error: string | null = $state(null);
  let saving = $state(false);
  let inputEl = $state<HTMLInputElement | null>(null);
  let searchRequest = 0;
  let creatingFile = $state("");
  let createMode = $state(false);
  const draftId = Symbol("add dependency creation draft");
  const mutationId = Symbol("add dependency mutation");
  let alive = true;

  function moveSelection(direction: -1 | 1) {
    const index = direction === 1
      ? results.findIndex((item, index) => index > selected && !item.broken)
      : results.findLastIndex((item, index) => index < selected && !item.broken);
    if (index >= 0) selected = index;
  }
  let candidateEmpty = $state<DependencyCandidatesEmpty | null>(null);

  onDestroy(() => {
    alive = false;
    searchRequest++;
    removeDraft(draftId);
  });

  $effect(() => {
    setDraftDirty(
      draftId,
      createMode && (query.trim().length > 0 || creatingFile.trim().length > 0),
    );
  });

  $effect(() => {
    const q = query;
    const request = ++searchRequest;
    if (q.length === 0) {
      results = [];
      candidateEmpty = null;
      selected = 0;
      loading = false;
      error = null;
      return;
    }
    loading = true;
    results = [];
    candidateEmpty = null;
    selected = 0;
    error = null;
    const controller = new AbortController();
    const handle = setTimeout(async () => {
      try {
        const candidates = await api.dependencyCandidates(targetFnode, q, 50, controller.signal);
        if (request !== searchRequest) return;
        candidateEmpty = candidates.empty;
        results = candidates.nodes;
        selected = results.findIndex((item) => !item.broken);
      } catch (e) {
        if (isAbortError(e)) return;
        if (request !== searchRequest) return;
        results = [];
        candidateEmpty = null;
        error = errMsg(e);
      } finally {
        if (request === searchRequest) loading = false;
      }
    }, 120);
    return () => {
      clearTimeout(handle);
      controller.abort();
      if (request === searchRequest) searchRequest++;
    };
  });

  $effect(() => {
    inputEl?.focus();
  });

  let canCreate = $derived(
    query.trim().length > 0 &&
      results.length === 0 &&
      !loading &&
      candidateEmpty?.kind === "no_match",
  );

  function excludedMessage(empty: Extract<DependencyCandidatesEmpty, { kind: "excluded" }>) {
    if (empty.source === 0 && empty.invalid_or_duplicate === 0) {
      return "all matches are already dependencies";
    }
    if (empty.existing_dependencies === 0 && empty.invalid_or_duplicate === 0) {
      return "all matches refer to this node";
    }
    if (empty.source === 0 && empty.existing_dependencies === 0) {
      return `all matches are invalid or duplicate (${empty.invalid_or_duplicate} excluded)`;
    }
    return `matches excluded: ${empty.source} source, ${empty.existing_dependencies} existing, ${empty.invalid_or_duplicate} invalid/duplicate`;
  }

  let emptyMessage = $derived.by(() => {
    if (candidateEmpty?.kind === "excluded") return excludedMessage(candidateEmpty);
    if (candidateEmpty?.kind === "result_limit") {
      return `${candidateEmpty.available} match(es) available, but the result limit is zero`;
    }
    if (candidateEmpty?.kind === "no_match") return "no results";
    return null;
  });

  function close() {
    if (saving || !confirmDiscardDraft(draftId)) return;
    onClose();
  }

  async function submit() {
    const node = results[selected];
    if (!node || node.broken || saving) return;
    saving = true;
    let pending = true;
    setMutationPending(mutationId, true);
    error = null;
    try {
      const updated = await api.addDep(targetFnode, node.fnode, targetRevision);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      onAdded(updated, { nodes: 0, edges: 1 });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }

  function startCreate() {
    if (!canCreate) return;
    createMode = true;
  }

  async function createAndAdd() {
    if (saving || !canCreate) return;
    saving = true;
    let pending = true;
    setMutationPending(mutationId, true);
    error = null;
    try {
      const params: { title: string; parent_fnode: string; file?: string } = {
        title: query.trim(),
        parent_fnode: targetFnode,
      };
      if (creatingFile.trim().length > 0) params.file = creatingFile.trim();
      const updated = await api.newNode(params, targetRevision);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      removeDraft(draftId);
      onAdded(updated, { nodes: 1, edges: 1 });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (disabled) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn")) return;
    switch (e.key) {
      case "Enter":
        e.preventDefault();
        if (createMode) {
          void createAndAdd();
        } else if (results.length > 0) {
          void submit();
        } else if (canCreate) {
          startCreate();
        }
        break;
      case "ArrowDown":
        e.preventDefault();
        moveSelection(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        moveSelection(-1);
        break;
    }
  }

  function onCancel(event: Event) {
    event.preventDefault();
    if (disabled || saving) return;
    if (createMode) {
      createMode = false;
      creatingFile = "";
    } else {
      close();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog"
    aria-label="add dependency"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget) close(); }}
  >
    <div class="search-field">
      <Link2 size={18} strokeWidth={1.8} />
      <input
        bind:this={inputEl}
        bind:value={query}
        placeholder="Search for a dependency…"
        autocomplete="off"
        spellcheck="false"
      />
      {#if loading}<span class="loading-label">Searching</span>{/if}
      <button class="close-btn" onclick={close} title="Close" aria-label="Close add dependency"><X size={17} strokeWidth={1.8} /></button>
    </div>
    <ul class="results">
      {#if createMode}
        <li class="create-form">
          <div class="create-title"><Plus size={15} strokeWidth={2} />Create new: {query}</div>
          <input
            class="create-file-input"
            bind:value={creatingFile}
            placeholder="file path (optional, e.g. notes/lemma)"
            autocomplete="off"
            spellcheck="false"
          />
          <button
            class="create-confirm"
            onclick={() => void createAndAdd()}
            disabled={saving || !canCreate}
          >
            create &amp; add
          </button>
        </li>
      {:else}
        {#each results as r, i (r.fnode)}
          <li>
            <button
              class="row"
              class:selected={i === selected}
              onclick={() => { selected = i; void submit(); }}
              disabled={r.broken || saving}
            >
              <span class="depth">[{r.depth}]</span>
              <span class="fnode">{shortFnode(r.fnode)}</span>
              <span class="title">{r.title}</span>
              <span class="path">{r.rel_path}</span>
            </button>
          </li>
        {:else}
          {#if canCreate}
            <li>
              <button
                class="row create"
                onclick={() => startCreate()}
                disabled={saving}
              >
                <span class="title create-label"><Plus size={14} strokeWidth={2} />Create new: {query}</span>
              </button>
            </li>
          {:else if emptyMessage}
            <li class="empty">{emptyMessage}</li>
          {/if}
        {/each}
      {/if}
    </ul>
    {#if error}
      <div class="error-bar">{error}</div>
    {/if}
    <div class="hint"><span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>Enter</kbd> Add</span><span><kbd>Esc</kbd> Cancel</span></div>
  </dialog>

<style>
  .dialog {
    width: min(860px, 92vw);
  }
  .search-field {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    min-height: 60px;
    padding: 0 0.85rem 0 1rem;
    color: var(--mdc-accent-down);
    background: var(--mdc-panel-raised);
    border-bottom: 1px solid var(--mdc-border);
  }
  input {
    min-width: 0;
    flex: 1;
    border: none;
    padding: 0;
    font-size: 0.98rem;
    background: transparent;
    color: var(--mdc-fg);
  }
  input::placeholder {
    color: var(--mdc-muted);
  }
  .loading-label {
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.62rem;
  }
  .close-btn {
    display: grid;
    place-items: center;
    width: 31px;
    height: 31px;
    padding: 0;
    color: var(--mdc-muted);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
  }
  .close-btn:hover {
    color: var(--mdc-fg);
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border);
  }
  .results {
    list-style: none;
    margin: 0;
    padding: 0.45rem;
    min-height: 70px;
    max-height: 54vh;
    overflow-y: auto;
  }
  .row {
    width: 100%;
    text-align: left;
    display: grid;
    grid-template-columns: 3.5rem 6rem minmax(15rem, 1.5fr) minmax(8rem, 1fr);
    gap: 0.75rem;
    align-items: center;
    min-height: 42px;
    padding: 0.45rem 0.65rem;
    background: transparent;
    border: 1px solid transparent;
    color: var(--mdc-fg);
    cursor: pointer;
    border-radius: 7px;
    font-family: inherit;
  }
  .row.selected {
    background: var(--mdc-card-selected);
    border-color: var(--mdc-border);
  }
  .row:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .depth {
    color: var(--mdc-muted);
    font-size: 0.66rem;
    font-variant-numeric: tabular-nums;
  }
  .fnode {
    color: var(--mdc-accent);
    font-family: var(--mdc-mono);
    font-size: 0.68rem;
  }
  .title {
    color: var(--mdc-fg-soft);
    font-size: 0.8rem;
    font-weight: 560;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path {
    color: var(--mdc-muted);
    font-size: 0.66rem;
    font-family: var(--mdc-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.create .create-label {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--mdc-accent);
    font-weight: 600;
    grid-column: 1 / -1;
  }
  .create-form {
    padding: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .create-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--mdc-accent);
    font-weight: 600;
    font-size: 0.82rem;
  }
  .create-file-input {
    width: 100%;
    box-sizing: border-box;
    flex: initial;
    background: var(--mdc-code-bg);
    color: var(--mdc-code-fg);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-sm);
    padding: 0.55rem 0.65rem;
    font-size: 0.76rem;
    font-family: var(--mdc-mono);
  }
  .create-file-input:focus {
    border-color: var(--mdc-accent);
  }
  .create-confirm {
    align-self: flex-start;
    background: var(--mdc-accent);
    color: var(--mdc-on-accent);
    border: none;
    border-radius: var(--mdc-radius-sm);
    padding: 0.48rem 0.75rem;
    font-size: 0.72rem;
    cursor: pointer;
    font-family: inherit;
  }
  .create-confirm:disabled {
    opacity: 0.5;
  }
  .empty {
    text-align: center;
    color: var(--mdc-muted);
    padding: 1.5rem;
    font-size: 0.76rem;
  }
  .error-bar {
    padding: 0.5rem 0.7rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    border-top: 1px solid var(--mdc-border);
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.55rem 0.8rem;
    font-size: 0.64rem;
    color: var(--mdc-muted);
    border-top: 1px solid var(--mdc-border);
    background: color-mix(in srgb, var(--mdc-bg) 48%, transparent);
  }
  .hint span {
    display: flex;
    align-items: center;
    gap: 0.28rem;
  }
  kbd {
    min-width: 20px;
    padding: 0.13rem 0.28rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    font-family: var(--mdc-mono);
    font-size: 0.58rem;
    text-align: center;
  }
</style>
