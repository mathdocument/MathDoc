<script lang="ts">
  import { onDestroy } from "svelte";
  import { Search, Unlink2, X } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg, shortFnode } from "../lib/format";
  import type { NodeDetail, NodeInfo } from "../lib/types";
  import { modal } from "../lib/modal";
  import { trackMutation } from "../lib/unsaved";

  interface Props {
    disabled: boolean;
    targetFnode: string;
    targetRevision: string;
    children: NodeInfo[];
    onRemoved: (node: NodeDetail, delta: { nodes: number; edges: number }) => void;
    onClose: () => void;
  }
  let { disabled, targetFnode, targetRevision, children, onRemoved, onClose }: Props = $props();

  let selected = $state<string[]>([]);
  let selection = $derived(new Set(selected));
  let query = $state("");
  let needle = $derived(query.trim().toLowerCase());
  let page = $state(0);
  const PAGE_SIZE = 50;
  let matches = $derived(needle ? children.filter(child =>
    `${child.title} ${child.fnode}`.toLowerCase().includes(needle)) : children);
  let pages = $derived(Math.max(1, Math.ceil(matches.length / PAGE_SIZE)));
  let visible = $derived(matches.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE));
  let list = $state<HTMLUListElement>();
  let cursor = $state(0);
  let saving = $state(false);
  let error: string | null = $state(null);
  let alive = true;

  onDestroy(() => {
    alive = false;
  });

  function close() {
    if (!saving) onClose();
  }

  function toggle(fnode: string) {
    selected = selection.has(fnode)
      ? selected.filter((item) => item !== fnode)
      : [...selected, fnode];
  }

  $effect(() => {
    if (page >= pages) page = pages - 1;
    if (cursor >= visible.length) cursor = Math.max(0, visible.length - 1);
  });

  function changePage(next: number) {
    page = next;
    cursor = 0;
    if (list) list.scrollTop = 0;
  }

  function onKey(e: KeyboardEvent) {
    if (disabled || saving || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.target instanceof Element && e.target.closest("input, .close-btn, .actions")) return;
    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      cursor = Math.max(0, Math.min(cursor + 1, visible.length - 1));
    } else if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
    } else if (e.key === " " || e.key === "x") {
      e.preventDefault();
      const child = visible[cursor];
      if (child) toggle(child.fnode);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void submit();
    }
    if (["ArrowDown", "ArrowUp", "j", "k"].includes(e.key)) list?.children[cursor]?.scrollIntoView({ block: "nearest" });
  }

  function onCancel(event: Event) {
    event.preventDefault();
    if (!disabled) close();
  }

  async function submit() {
    const toRemove = children.filter((child) => selection.has(child.fnode)).map((child) => child.fnode);
    if (toRemove.length === 0 || saving) {
      close();
      return;
    }
    saving = true;
    const clearMutation = trackMutation();
    error = null;
    try {
      const updated = await api.rmDeps(targetFnode, toRemove, targetRevision);
      clearMutation();
      if (!alive) return;
      onRemoved(updated, { nodes: 0, edges: -toRemove.length });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      clearMutation();
      if (alive) saving = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog modal-wide dependency-dialog"
    aria-label="remove dependencies"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget) close(); }}
  >
    <header class="dialog-head">
      <span class="head-icon"><Unlink2 size={16} strokeWidth={1.8} /></span>
      <span><small>Current node</small><h2>Remove dependencies</h2></span>
      <button class="close-btn" onclick={close} title="Close" aria-label="Close remove dependencies"><X size={17} strokeWidth={1.8} /></button>
    </header>
    {#if children.length === 0}
      <div class="empty modal-empty">no direct dependencies to remove</div>
    {:else}
      <div class="modal-search-field">
        <Search size={18} strokeWidth={1.8} />
        <input type="search" bind:value={query} oninput={() => changePage(0)}
          aria-label="Filter dependencies" placeholder="Filter by title or fnode…" disabled={saving} />
      </div>
      <ul class="list modal-list modal-results" bind:this={list}>
        {#each visible as c, i (c.fnode)}
          <li>
            <button
              class="row modal-row"
              class:selected={i === cursor}
              class:checked={selection.has(c.fnode)}
              aria-pressed={selection.has(c.fnode)}
              onclick={() => { cursor = i; toggle(c.fnode); }}
              disabled={saving}
            >
              <span class="check">{selection.has(c.fnode) ? "✓" : " "}</span>
              <span class="depth">[{c.depth}]</span>
              <span class="fnode">{shortFnode(c.fnode)}</span>
              <span class="title">{c.title}</span>
            </button>
          </li>
        {:else}
          <li class="modal-empty">No matching dependencies</li>
        {/each}
      </ul>
      <div class="pagination">
        <span role="status">{matches.length} dependencies · {selected.length} selected</span>
        {#if pages > 1}
          <div class="actions" aria-label="Dependency pages">
            <button class="secondary" onclick={() => changePage(page - 1)} disabled={saving || page === 0}>Previous</button>
            <span>{page + 1} / {pages}</span>
            <button class="secondary" onclick={() => changePage(page + 1)} disabled={saving || page === pages - 1}>Next</button>
          </div>
        {/if}
      </div>
    {/if}
    {#if error}
      <div class="error-bar modal-error">{error}</div>
    {/if}
    <footer class="dialog-footer">
      <div class="hint"><span><kbd>Space</kbd> Toggle</span><span><kbd>Enter</kbd> Remove</span><span><kbd>Esc</kbd> Cancel</span></div>
      <div class="actions">
        <button class="secondary" onclick={close} disabled={saving}>Cancel</button>
        <button class="danger" onclick={() => void submit()} disabled={saving || selected.length === 0}>Remove selected</button>
      </div>
    </footer>
  </dialog>

<style>
  .head-icon {
    color: var(--mdc-error);
    background: color-mix(in srgb, var(--mdc-error) 12%, transparent);
  }
  .pagination { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 0.75rem; padding: 0.5rem 0.75rem; color: var(--mdc-muted); font-size: var(--mdc-text-xs); }
  .pagination .actions { align-items: center; }
  .row.checked {
    color: var(--mdc-fg);
    background: color-mix(in srgb, var(--mdc-error) 9%, transparent);
  }
  .check {
    display: grid;
    place-items: center;
    width: 17px;
    height: 17px;
    color: transparent;
    border: 1.5px solid var(--mdc-border-strong);
    border-radius: 5px;
    font-weight: 700;
    font-size: 0.66rem;
    transition: background var(--mdc-dur-fast) var(--mdc-ease),
      border-color var(--mdc-dur-fast) var(--mdc-ease);
  }
  .row.checked .check {
    color: var(--mdc-on-error);
    background: var(--mdc-error);
    border-color: var(--mdc-error);
  }
  .error-bar {
    padding: 0.55rem 0.75rem;
    border-top: 1px solid color-mix(in srgb, var(--mdc-error) 22%, transparent);
  }
  .danger {
    color: var(--mdc-on-error);
    background: var(--mdc-error);
    border: 1px solid var(--mdc-error);
  }
  .danger:hover:not(:disabled) {
    filter: brightness(1.08);
  }
</style>
