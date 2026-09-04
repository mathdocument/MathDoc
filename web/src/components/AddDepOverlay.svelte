<script lang="ts">
  import { onDestroy } from "svelte";
  import { Link2, Plus, X } from "@lucide/svelte";
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
  let inputEl = $state<HTMLInputElement | null>(null);
  let creatingFile = $state("");
  let createMode = $state(false);
  const draftId = Symbol("add dependency creation draft");
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
      const params: { title: string; parent_fnode: string; file?: string } = {
        title: query.trim(),
        parent_fnode: targetFnode,
      };
      if (creatingFile.trim().length > 0) params.file = creatingFile.trim();
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
    class="dialog modal-dialog modal-wide"
    aria-label="add dependency"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget) close(); }}
  >
    <div class="search-field modal-search-field">
      <Link2 size={18} strokeWidth={1.8} />
      <input
        bind:this={inputEl}
        bind:value={query}
        placeholder="Search for a dependency…"
        autocomplete="off"
        spellcheck="false"
        disabled={saving}
      />
      {#if loading}<span class="loading-label">Searching</span>{/if}
      <button class="close-btn" onclick={close} title="Close" aria-label="Close add dependency"><X size={17} strokeWidth={1.8} /></button>
    </div>
    <ul class="results modal-list modal-results">
      {#if createMode}
        <li class="create-form">
          <div class="create-title"><Plus size={15} strokeWidth={2} />Create new: {query}</div>
          <input
            class="create-file-input"
            bind:value={creatingFile}
            placeholder="file path (optional, e.g. notes/lemma)"
            autocomplete="off"
            spellcheck="false"
            disabled={saving}
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
              class="row modal-row modal-result-row"
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
                class="row modal-row modal-result-row create"
                onclick={() => startCreate()}
                disabled={saving}
              >
                <span class="title create-label"><Plus size={14} strokeWidth={2} />Create new: {query}</span>
              </button>
            </li>
          {:else if emptyMessage}
            <li class="empty modal-empty">{emptyMessage}</li>
          {/if}
        {/each}
      {/if}
    </ul>
    {#if error}
      <div class="error-bar modal-error">{error}</div>
    {/if}
    <div class="hint modal-search-hint"><span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>Enter</kbd> Add</span><span><kbd>Esc</kbd> Cancel</span></div>
  </dialog>

<style>
  .search-field {
    color: var(--mdc-accent-down);
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
  .create-file-input {
    width: 100%;
    box-sizing: border-box;
    flex: initial;
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-sm);
    padding: 0.55rem 0.7rem;
    font-size: var(--mdc-text-sm);
    font-family: var(--mdc-mono);
    transition: border-color var(--mdc-dur-fast) var(--mdc-ease),
      box-shadow var(--mdc-dur-fast) var(--mdc-ease);
  }
  .create-file-input:focus {
    outline: none;
    border-color: var(--mdc-accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-ring) 28%, transparent);
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
