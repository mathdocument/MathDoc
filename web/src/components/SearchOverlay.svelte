<script lang="ts">
  import { api, isAbortError } from "../lib/api";
  import { Search, X } from "@lucide/svelte";
  import type { NodeInfo } from "../lib/types";
  import { errMsg, shortFnode } from "../lib/format";
  import { modal } from "../lib/modal";

  interface Props {
    disabled: boolean;
    onPick: (fnode: string) => void;
    onClose: () => void;
  }
  let { disabled, onPick, onClose }: Props = $props();

  let query = $state("");
  let results = $state<NodeInfo[]>([]);
  let selected = $state(0);
  let loading = $state(false);
  let error: string | null = $state(null);
  let inputEl = $state<HTMLInputElement | null>(null);
  let searchRequest = 0;

  function moveSelection(direction: -1 | 1) {
    const index = direction === 1
      ? results.findIndex((item, index) => index > selected && !item.broken)
      : results.findLastIndex((item, index) => index < selected && !item.broken);
    if (index >= 0) selected = index;
  }

  $effect(() => {
    const q = query;
    const request = ++searchRequest;
    if (q.length === 0) {
      results = [];
      selected = 0;
      loading = false;
      error = null;
      return;
    }
    loading = true;
    error = null;
    results = [];
    selected = 0;
    const controller = new AbortController();
    const handle = setTimeout(async () => {
      try {
        const fresh = await api.search(q, 50, controller.signal);
        if (request !== searchRequest) return;
        results = fresh;
        selected = fresh.findIndex((item) => !item.broken);
      } catch (e) {
        if (isAbortError(e)) return;
        if (request !== searchRequest) return;
        results = [];
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

  function submit() {
    const node = results[selected];
    if (node && !node.broken) {
      onPick(node.fnode);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (disabled) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn")) return;
    switch (e.key) {
      case "Enter":
        e.preventDefault();
        submit();
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
    if (!disabled) onClose();
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog"
    aria-label="search"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget && !disabled) onClose(); }}
  >
    <div class="search-field">
      <Search size={18} strokeWidth={1.8} />
      <input
        bind:this={inputEl}
        bind:value={query}
        placeholder="Search by title or fnode…"
        autocomplete="off"
        spellcheck="false"
      />
      {#if loading}<span class="loading-label">Searching</span>{/if}
      <button class="close-btn" onclick={onClose} title="Close" aria-label="Close search"><X size={17} strokeWidth={1.8} /></button>
    </div>
    <ul class="results">
      {#each results as r, i (`${r.fnode}\0${r.rel_path}`)}
        <li>
          <button
            class="row"
            class:selected={i === selected}
            onclick={() => onPick(r.fnode)}
            disabled={r.broken}
          >
            <span class="depth">[{r.depth}]</span>
            <span class="fnode">{shortFnode(r.fnode)}</span>
            <span class="title">{r.title}</span>
            <span class="path">{r.rel_path}</span>
          </button>
        </li>
      {:else}
        {#if error}
          <li class="empty error" role="alert">search failed: {error}</li>
        {:else if query && !loading}
          <li class="empty">no results</li>
        {/if}
      {/each}
    </ul>
    <div class="hint"><span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>Enter</kbd> Open</span><span><kbd>Esc</kbd> Close</span></div>
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
    color: var(--mdc-muted);
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
    border: none;
    color: var(--mdc-fg);
    cursor: pointer;
    border: 1px solid transparent;
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
  .empty {
    text-align: center;
    color: var(--mdc-muted);
    padding: 1.5rem;
    font-size: 0.76rem;
  }
  .empty.error {
    color: var(--mdc-error);
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
