<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { Unlink2, X } from "@lucide/svelte";
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

  let selected = $state(untrack(() => children.map(() => false)));
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

  function onKey(e: KeyboardEvent) {
    if (disabled) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn, .actions")) return;
    if (saving) return;
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
    class="dialog modal-dialog modal-wide"
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
      <ul class="list modal-list">
        {#each children as c, i (c.fnode)}
          <li>
            <button
              class="row modal-row"
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
      <div class="error-bar modal-error">{error}</div>
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
    border-color: var(--mdc-dim);
  }
  .dialog-head {
    min-height: 62px;
  }
  .head-icon {
    color: var(--mdc-error);
    background: rgba(255, 125, 143, 0.1);
  }
  .close-btn {
    margin-left: auto;
  }
  .row {
    grid-template-columns: 1.4rem 3.5rem 6rem minmax(15rem, 1.5fr) minmax(8rem, 1fr);
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
  .error-bar {
    padding: 0.5rem 0.7rem;
    border-top: 1px solid rgba(255, 125, 143, 0.2);
  }
  .dialog-footer {
    min-height: 58px;
  }
  .hint {
    gap: 0.8rem;
    font-size: 0.62rem;
  }
  kbd {
    padding: 0.13rem 0.3rem;
    font-size: 0.57rem;
  }
  .danger {
    color: var(--mdc-on-error);
    background: var(--mdc-error);
    border: 1px solid var(--mdc-error);
  }
</style>
