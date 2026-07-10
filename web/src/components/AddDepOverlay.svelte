<script lang="ts">
  import { onDestroy } from "svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";
  import { modal } from "../lib/modal";
  import {
    confirmDiscardDrafts,
    removeDraft,
    setDraftDirty,
    setMutationPending,
  } from "../lib/unsaved";

  interface Props {
    targetFnode: string;
    existingDepFnodes: string[];
    onAdded: () => void;
    onClose: () => void;
  }
  let { targetFnode, existingDepFnodes, onAdded, onClose }: Props = $props();

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

  function firstSelectable(items: NodeInfo[]): number {
    return items.findIndex((item) => !item.broken);
  }

  function moveSelection(direction: -1 | 1) {
    for (
      let index = selected + direction;
      index >= 0 && index < results.length;
      index += direction
    ) {
      if (!results[index]!.broken) {
        selected = index;
        return;
      }
    }
  }
  // Whether the raw search (before filtering existing deps) returned any
  // results. Used to distinguish "no matches at all" (can create new) from
  // "all matches are already deps" (should not create a duplicate).
  let rawHadMatches = $state(false);

  // Existing deps + self are excluded from search results.
  let excluded = $derived(new Set([...existingDepFnodes, targetFnode]));

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
    const excludedForRequest = excluded;
    if (q.length === 0) {
      results = [];
      rawHadMatches = false;
      selected = 0;
      loading = false;
      return;
    }
    loading = true;
    results = [];
    rawHadMatches = false;
    selected = 0;
    const handle = setTimeout(async () => {
      try {
        const all = await api.search(q, 50);
        if (request !== searchRequest) return;
        rawHadMatches = all.length > 0;
        results = all.filter((r) => !excludedForRequest.has(r.fnode));
        selected = firstSelectable(results);
      } catch {
        if (request !== searchRequest) return;
        results = [];
        rawHadMatches = false;
      } finally {
        if (request === searchRequest) loading = false;
      }
    }, 120);
    return () => {
      clearTimeout(handle);
      if (request === searchRequest) searchRequest++;
    };
  });

  $effect(() => {
    inputEl?.focus();
  });

  // When results are empty, show a "create new" option — but only if the
  // raw search had zero matches (not if all matches were already deps).
  let canCreate = $derived(query.trim().length > 0 && results.length === 0 && !loading && !rawHadMatches);

  function close() {
    if (saving || !confirmDiscardDrafts()) return;
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
      await api.addDep(targetFnode, node.fnode);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      onAdded();
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }

  function startCreate() {
    createMode = true;
  }

  async function createAndAdd() {
    if (saving || !query.trim()) return;
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
      await api.newNode(params);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      removeDraft(draftId);
      onAdded();
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        if (createMode) {
          createMode = false;
          creatingFile = "";
        } else {
          close();
        }
        break;
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
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={close} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="add dependency"
    tabindex="-1"
    use:modal
    onclick={(e) => e.stopPropagation()}
  >
    <input
      bind:this={inputEl}
      bind:value={query}
      placeholder="search dependency by title or fnode…"
      autocomplete="off"
      spellcheck="false"
    />
    <ul class="results">
      {#if createMode}
        <li class="create-form">
          <div class="create-title">✦ Create new: {query}</div>
          <input
            class="create-file-input"
            bind:value={creatingFile}
            placeholder="file path (optional, e.g. notes/lemma)"
            autocomplete="off"
            spellcheck="false"
          />
          <button class="create-confirm" onclick={() => void createAndAdd()} disabled={saving}>
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
                <span class="title create-label">✦ Create new: {query}</span>
              </button>
            </li>
          {:else if rawHadMatches}
            <li class="empty">all matches are already dependencies</li>
          {:else if query && !loading}
            <li class="empty">no results</li>
          {/if}
        {/each}
      {/if}
    </ul>
    {#if error}
      <div class="error-bar">{error}</div>
    {/if}
    <div class="hint">↑↓ navigate · Enter add · Esc cancel</div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 50;
  }
  .dialog {
    width: min(820px, 92vw);
    background: var(--mdc-panel);
    border: 1px solid var(--mdc-border-strong);
    border-radius: 6px;
    overflow: hidden;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.35);
  }
  input {
    width: 100%;
    box-sizing: border-box;
    border: none;
    border-bottom: 1px solid var(--mdc-border);
    padding: 0.7rem 0.9rem;
    font-size: 1rem;
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    font-family: inherit;
  }
  input:focus {
    outline: none;
    background: var(--mdc-card-hover);
  }
  .results {
    list-style: none;
    margin: 0;
    padding: 0.3rem;
    max-height: 50vh;
    overflow-y: auto;
  }
  .row {
    width: 100%;
    text-align: left;
    display: grid;
    grid-template-columns: 3rem 6rem 26rem minmax(0, 1fr);
    gap: 0.7rem;
    align-items: baseline;
    padding: 0.4rem 0.5rem;
    background: transparent;
    border: none;
    color: var(--mdc-fg);
    cursor: pointer;
    border-radius: 3px;
    font-family: inherit;
  }
  .row.selected {
    background: var(--mdc-card-selected);
  }
  .row:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .depth {
    color: var(--mdc-dim);
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
  }
  .fnode {
    color: var(--mdc-accent);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
  }
  .title {
    font-size: 0.92rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path {
    color: var(--mdc-dim);
    font-size: 0.78rem;
    font-family: var(--mdc-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.create .create-label {
    color: var(--mdc-accent);
    font-weight: 600;
    grid-column: 1 / -1;
  }
  .create-form {
    padding: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .create-title {
    color: var(--mdc-accent);
    font-weight: 600;
    font-size: 0.92rem;
  }
  .create-file-input {
    width: 100%;
    box-sizing: border-box;
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 3px;
    padding: 0.4rem 0.5rem;
    font-size: 0.85rem;
    font-family: var(--mdc-mono);
  }
  .create-file-input:focus {
    outline: none;
    border-color: var(--mdc-accent);
  }
  .create-confirm {
    align-self: flex-start;
    background: var(--mdc-accent);
    color: var(--mdc-bg);
    border: none;
    border-radius: 3px;
    padding: 0.3rem 0.7rem;
    font-size: 0.8rem;
    cursor: pointer;
    font-family: inherit;
  }
  .create-confirm:disabled {
    opacity: 0.5;
  }
  .empty {
    text-align: center;
    color: var(--mdc-dim);
    padding: 1.2rem;
  }
  .error-bar {
    padding: 0.5rem 0.7rem;
    background: rgba(247, 118, 142, 0.12);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
    border-top: 1px solid var(--mdc-border);
  }
  .hint {
    padding: 0.4rem 0.7rem;
    font-size: 0.72rem;
    color: var(--mdc-dim);
    border-top: 1px solid var(--mdc-border);
  }
</style>
