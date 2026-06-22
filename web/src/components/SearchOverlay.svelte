<script lang="ts">
  import { api } from "../lib/api";
  import type { NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";

  interface Props {
    onPick: (fnode: string) => void;
    onClose: () => void;
  }
  let { onPick, onClose }: Props = $props();

  let query = $state("");
  let results = $state<NodeInfo[]>([]);
  let selected = $state(0);
  let loading = $state(false);
  let inputEl = $state<HTMLInputElement | null>(null);

  $effect(() => {
    const q = query;
    if (q.length === 0) {
      results = [];
      selected = 0;
      return;
    }
    loading = true;
    const handle = setTimeout(async () => {
      try {
        results = await api.search(q, 50);
        selected = 0;
      } catch {
        results = [];
      } finally {
        loading = false;
      }
    }, 120);
    return () => clearTimeout(handle);
  });

  $effect(() => {
    inputEl?.focus();
  });

  function submit() {
    const node = results[selected];
    if (node) {
      onPick(node.fnode);
    }
  }

  function onKey(e: KeyboardEvent) {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        onClose();
        break;
      case "Enter":
        e.preventDefault();
        submit();
        break;
      case "ArrowDown":
        e.preventDefault();
        if (selected + 1 < results.length) selected += 1;
        break;
      case "ArrowUp":
        e.preventDefault();
        selected = Math.max(0, selected - 1);
        break;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={onClose}
  role="presentation"
>
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    role="dialog"
    aria-label="search"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <input
      bind:this={inputEl}
      bind:value={query}
      placeholder="search by title or fnode…"
      autocomplete="off"
      spellcheck="false"
    />
    <ul class="results">
      {#each results as r, i (r.fnode)}
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
        {#if query && !loading}
          <li class="empty">no results</li>
        {/if}
      {/each}
    </ul>
    <div class="hint">↑↓ navigate · Enter open · Esc close</div>
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
  .empty {
    text-align: center;
    color: var(--mdc-dim);
    padding: 1.2rem;
  }
  .hint {
    padding: 0.4rem 0.7rem;
    font-size: 0.72rem;
    color: var(--mdc-dim);
    border-top: 1px solid var(--mdc-border);
  }
</style>
