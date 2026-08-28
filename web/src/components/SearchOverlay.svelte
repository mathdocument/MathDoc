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
    class="dialog modal-dialog modal-wide"
    aria-label="search"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget && !disabled) onClose(); }}
  >
    <div class="search-field modal-search-field">
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
    <ul class="results modal-list modal-results">
      {#each results as r, i (`${r.fnode}\0${r.rel_path}`)}
        <li>
          <button
            class="row modal-row modal-result-row"
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
          <li class="empty modal-empty error" role="alert">search failed: {error}</li>
        {:else if query && !loading}
          <li class="empty modal-empty">no results</li>
        {/if}
      {/each}
    </ul>
    <div class="hint modal-search-hint"><span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>Enter</kbd> Open</span><span><kbd>Esc</kbd> Close</span></div>
  </dialog>

<style>
  .search-field {
    color: var(--mdc-muted);
  }
  .empty.error {
    color: var(--mdc-error);
  }
</style>
