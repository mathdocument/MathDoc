<script lang="ts">
  import { onDestroy } from "svelte";
  import { Link2, Plus, Search, X } from "@lucide/svelte";
  import { api, isAbortError } from "../lib/api";
  import { errMsg, shortFnode } from "../lib/format";
  import type { DependencyCandidatesEmpty, NodeDetail, NodeInfo } from "../lib/types";
  import { modal } from "../lib/modal";
  import {
    confirmDiscardDraft,
    removeDraft,
    setDraftDirty,
    trackMutation,
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
  let list = $state<HTMLUListElement>();
  let createMode = $state(false);
  const draftId = Symbol("add dependency creation draft");
  let alive = true;

  function moveSelection(direction: -1 | 1) {
    const index = direction === 1
      ? results.findIndex((item, index) => index > selected && !item.broken)
      : results.findLastIndex((item, index) => index < selected && !item.broken);
    if (index >= 0) {
      selected = index;
      list?.children[index]?.scrollIntoView({ block: "nearest" });
    }
  }
  let candidateEmpty = $state<DependencyCandidatesEmpty | null>(null);

  onDestroy(() => {
    alive = false;
    removeDraft(draftId);
  });

  $effect(() => {
    setDraftDirty(
      draftId,
      createMode && (query.trim().length > 0),
    );
  });

  $effect(() => {
    const q = query;
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
        if (controller.signal.aborted) return;
        candidateEmpty = candidates.empty;
        results = candidates.nodes;
        selected = results.findIndex((item) => !item.broken);
      } catch (e) {
        if (controller.signal.aborted || isAbortError(e)) return;
        results = [];
        candidateEmpty = null;
        error = errMsg(e);
      } finally {
        if (!controller.signal.aborted) loading = false;
      }
    }, 120);
    return () => {
      clearTimeout(handle);
      controller.abort();
    };
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
    const clearMutation = trackMutation();
    error = null;
    try {
      const updated = await api.addDep(targetFnode, node.fnode, targetRevision);
      clearMutation();
      if (!alive) return;
      onAdded(updated, { nodes: 0, edges: 1 });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      clearMutation();
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
    const clearMutation = trackMutation();
    error = null;
    try {
      const params: { title: string; parent_fnode: string } = {
        title: query.trim(),
        parent_fnode: targetFnode,
      };
      const updated = await api.newNode(params, targetRevision);
      clearMutation();
      if (!alive) return;
      removeDraft(draftId);
      onAdded(updated, { nodes: 1, edges: 1 });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      clearMutation();
      if (alive) saving = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (disabled || saving || e.isComposing) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn, .actions, .create-confirm")) return;
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
    close();
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog modal-wide node-dialog"
    aria-label="add dependency"
    use:modal={"input"}
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget) close(); }}
  >
    <header class="dialog-head">
      <span class="head-icon"><Link2 size={16} strokeWidth={1.8} /></span>
      <span><small>Current node</small><h2>Add dependency</h2></span>
      <button class="close-btn" onclick={close} title="Close" aria-label="Close add dependency"><X size={17} strokeWidth={1.8} /></button>
    </header>
    <div class="modal-search-field">
      <Search size={18} strokeWidth={1.8} />
      <input
        type="search"
        bind:value={query}
        aria-label="Search dependencies"
        placeholder="Search for a dependency..."
        autocomplete="off"
        spellcheck="false"
        disabled={saving}
      />
      {#if loading}<span class="loading-label">Searching</span>{/if}
    </div>
    <ul class="results modal-list modal-results" bind:this={list}>
      {#if createMode}
        <li class="create-form">
          <div class="create-title"><Plus size={15} strokeWidth={2} />Create new: {query}</div>
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
              class="row modal-row"
              class:selected={i === selected}
              onclick={() => { selected = i; void submit(); }}
              disabled={r.broken || saving}
            >
              <span class="choice"><Plus size={15} strokeWidth={1.8} /></span>
              <span class="depth">[{r.depth}]</span>
              <span class="fnode">{shortFnode(r.fnode)}</span>
              <span class="title">{r.title}</span>
            </button>
          </li>
        {:else}
          {#if canCreate}
            <li>
              <button
                class="row modal-row create"
                onclick={() => startCreate()}
                disabled={saving}
              >
                <span class="title create-label"><Plus size={14} strokeWidth={2} />Create new: {query}</span>
              </button>
            </li>
          {:else if emptyMessage}
            <li class="empty modal-empty">{emptyMessage}</li>
          {:else}
            <li class="empty modal-empty">{loading ? "Searching..." : "Search by title or fnode"}</li>
          {/if}
        {/each}
      {/if}
    </ul>
    {#if error}
      <div class="error-bar modal-error">{error}</div>
    {/if}
    <footer class="dialog-footer">
      <div class="hint"><span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>Enter</kbd> Add</span><span><kbd>Esc</kbd> Cancel</span></div>
      <div class="actions">
        <button class="secondary" onclick={close} disabled={saving}>Cancel</button>
        {#if !createMode}
          <button class="primary" onclick={() => void submit()} disabled={saving || !results[selected] || results[selected]?.broken}>Add selected</button>
        {/if}
      </div>
    </footer>
  </dialog>

<style>
  .head-icon {
    color: var(--mdc-accent-down);
    background: color-mix(in srgb, var(--mdc-accent-down) 12%, transparent);
  }
  .choice { display: grid; place-items: center; color: var(--mdc-muted); }
  .primary { color: var(--mdc-on-accent); background: var(--mdc-accent); border: 1px solid var(--mdc-accent); }
  .row.create .create-label {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--mdc-accent);
    font-weight: 600;
    grid-column: 1 / -1;
  }
  .create-form {
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .create-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--mdc-fg);
    font-weight: 620;
    font-size: var(--mdc-text-md);
    letter-spacing: var(--mdc-tracking-tight);
  }
  .create-confirm {
    align-self: flex-start;
    min-height: 32px;
    background: var(--mdc-accent);
    color: var(--mdc-on-accent);
    border: none;
    border-radius: var(--mdc-radius-sm);
    padding: 0 0.8rem;
    font-size: var(--mdc-text-sm);
    font-weight: 620;
    cursor: pointer;
    font-family: inherit;
    transition: background var(--mdc-dur-fast) var(--mdc-ease);
  }
  .create-confirm:hover:not(:disabled) {
    background: var(--mdc-accent-strong);
  }
  .create-confirm:disabled {
    opacity: 0.5;
  }
  .error-bar {
    padding: 0.55rem 0.75rem;
    border-top: 1px solid var(--mdc-border);
  }
</style>
