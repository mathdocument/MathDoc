<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Unlink2, X } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { NodeDetail, NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";
  import { modal } from "../lib/modal";
  import { setMutationPending } from "../lib/unsaved";

  interface Props {
    disabled: boolean;
    targetFnode: string;
    targetRevision: string;
    onRemoved: (node: NodeDetail, delta: { nodes: number; edges: number }) => void;
    onClose: () => void;
  }
  let { disabled, targetFnode, targetRevision, onRemoved, onClose }: Props = $props();

  let children = $state<NodeInfo[]>([]);
  let selected = $state<boolean[]>([]);
  let cursor = $state(0);
  let saving = $state(false);
  let loading = $state(true);
  let error: string | null = $state(null);
  let loadRequest = 0;
  let alive = true;
  const mutationId = Symbol("remove dependency mutation");

  onDestroy(() => {
    alive = false;
    loadRequest++;
  });

  onMount(() => {
    const request = ++loadRequest;
    loading = true;
    error = null;
    children = [];
    selected = [];
    cursor = 0;
    api.children(targetFnode).then((items) => {
      if (!alive || request !== loadRequest) return;
      children = items;
      selected = items.map(() => false);
      cursor = 0;
      loading = false;
    }).catch((e) => {
      if (!alive || request !== loadRequest) return;
      error = errMsg(e);
      loading = false;
    });
  });

  function close() {
    if (!saving) onClose();
  }

  function onKey(e: KeyboardEvent) {
    if (disabled) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn, .actions")) return;
    if (loading || saving) return;
    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      cursor = Math.min(cursor + 1, children.length - 1);
    } else if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
    } else if (e.key === " " || e.key === "x") {
      e.preventDefault();
      if (cursor < selected.length) selected[cursor] = !selected[cursor];
    } else if (e.key === "Enter") {
      e.preventDefault();
      void submit();
    }
  }

  function onCancel(event: Event) {
    event.preventDefault();
    if (!disabled) close();
  }

  async function submit() {
    const toRemove = children
      .filter((_, i) => selected[i])
      .map((c) => c.fnode);
    if (toRemove.length === 0 || saving) {
      close();
      return;
    }
    saving = true;
    let pending = true;
    setMutationPending(mutationId, true);
    error = null;
    try {
      const updated = await api.rmDeps(targetFnode, toRemove, targetRevision);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      onRemoved(updated, { nodes: 0, edges: -toRemove.length });
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog"
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
    {#if loading}
      <div class="empty">loading…</div>
    {:else if children.length === 0}
      <div class="empty">no direct dependencies to remove</div>
    {:else}
      <ul class="list">
        {#each children as c, i (c.fnode)}
          <li>
            <button
              class="row"
              class:cursor={i === cursor}
              class:checked={selected[i]}
              onclick={() => { cursor = i; selected[i] = !selected[i]; }}
              disabled={saving}
            >
              <span class="check">{selected[i] ? "✓" : " "}</span>
              <span class="depth">[{c.depth}]</span>
              <span class="fnode">{shortFnode(c.fnode)}</span>
              <span class="title">{c.title}</span>
              <span class="path">{c.rel_path}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if error}
      <div class="error-bar">{error}</div>
    {/if}
    <footer class="dialog-footer">
      <div class="hint"><span><kbd>Space</kbd> Toggle</span><span><kbd>Enter</kbd> Remove</span><span><kbd>Esc</kbd> Cancel</span></div>
      <div class="actions">
        <button class="secondary" onclick={close} disabled={saving}>Cancel</button>
        <button class="danger" onclick={() => void submit()} disabled={saving || selected.every((value) => !value)}>Remove selected</button>
      </div>
    </footer>
  </dialog>

<style>
  .dialog {
    width: min(860px, 92vw);
    border-color: var(--mdc-dim);
  }
  .dialog-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    min-height: 62px;
    padding: 0 0.85rem 0 1rem;
    background: var(--mdc-panel-raised);
    border-bottom: 1px solid var(--mdc-border);
  }
  .head-icon {
    display: grid;
    place-items: center;
    width: 31px;
    height: 31px;
    color: var(--mdc-error);
    background: rgba(255, 125, 143, 0.1);
    border-radius: 8px;
  }
  .dialog-head > span:nth-child(2) {
    display: flex;
    flex-direction: column;
    gap: 0.12rem;
  }
  .dialog-head small {
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.58rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  h2 {
    margin: 0;
    color: var(--mdc-fg);
    font-size: 0.9rem;
    font-weight: 630;
  }
  .close-btn {
    display: grid;
    place-items: center;
    width: 31px;
    height: 31px;
    margin-left: auto;
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
  .list {
    list-style: none;
    margin: 0;
    padding: 0.45rem;
    max-height: 54vh;
    overflow-y: auto;
  }
  .row {
    width: 100%;
    text-align: left;
    display: grid;
    grid-template-columns: 1.4rem 3.5rem 6rem minmax(15rem, 1.5fr) minmax(8rem, 1fr);
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
  .row.cursor {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border);
  }
  .row.checked {
    color: var(--mdc-fg);
    background: rgba(255, 125, 143, 0.07);
    border-color: rgba(255, 125, 143, 0.18);
  }
  .row:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .check {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    color: var(--mdc-error);
    border: 1px solid var(--mdc-border-strong);
    border-radius: 4px;
    font-weight: 700;
    font-size: 0.65rem;
  }
  .row.checked .check {
    color: var(--mdc-on-error);
    background: var(--mdc-error);
    border-color: var(--mdc-error);
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
    color: var(--mdc-muted);
    padding: 1.5rem;
    text-align: center;
    font-size: 0.76rem;
  }
  .error-bar {
    padding: 0.5rem 0.7rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    border-top: 1px solid rgba(255, 125, 143, 0.2);
  }
  .dialog-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    min-height: 58px;
    padding: 0.65rem 0.8rem;
    border-top: 1px solid var(--mdc-border);
    background: color-mix(in srgb, var(--mdc-bg) 48%, transparent);
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    font-size: 0.62rem;
    color: var(--mdc-muted);
  }
  .hint span {
    display: flex;
    align-items: center;
    gap: 0.28rem;
  }
  kbd {
    padding: 0.13rem 0.3rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    font-family: var(--mdc-mono);
    font-size: 0.57rem;
  }
  .actions {
    display: flex;
    gap: 0.45rem;
  }
  .actions button {
    min-height: 32px;
    padding: 0 0.72rem;
    border-radius: var(--mdc-radius-sm);
    font-size: 0.68rem;
    font-weight: 600;
    cursor: pointer;
  }
  .actions button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .secondary {
    color: var(--mdc-fg-soft);
    background: transparent;
    border: 1px solid var(--mdc-border);
  }
  .danger {
    color: var(--mdc-on-error);
    background: var(--mdc-error);
    border: 1px solid var(--mdc-error);
  }
</style>
