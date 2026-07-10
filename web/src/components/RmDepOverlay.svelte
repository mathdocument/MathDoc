<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";
  import { modal } from "../lib/modal";
  import { setMutationPending } from "../lib/unsaved";

  interface Props {
    targetFnode: string;
    onRemoved: () => void;
    onClose: () => void;
  }
  let { targetFnode, onRemoved, onClose }: Props = $props();

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
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
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
      await api.rmDeps(targetFnode, toRemove);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      onRemoved();
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

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={close} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="remove dependencies"
    tabindex="-1"
    use:modal
    onclick={(e) => e.stopPropagation()}
  >
    <h2>remove dependencies</h2>
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
    <div class="hint">Space toggle · Enter remove · Esc cancel</div>
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
    padding: 0.8rem 1rem;
  }
  h2 {
    margin: 0 0 0.6rem;
    font-size: 0.95rem;
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 50vh;
    overflow-y: auto;
  }
  .row {
    width: 100%;
    text-align: left;
    display: grid;
    grid-template-columns: 1.4rem 3rem 6rem 26rem minmax(0, 1fr);
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
  .row.cursor {
    background: var(--mdc-card-hover);
  }
  .row.checked {
    color: var(--mdc-error);
  }
  .row:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .check {
    color: var(--mdc-error);
    font-weight: 700;
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
    color: var(--mdc-dim);
    padding: 1rem;
    text-align: center;
  }
  .error-bar {
    margin-top: 0.4rem;
    padding: 0.4rem 0.6rem;
    background: rgba(247, 118, 142, 0.12);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
    border-radius: 3px;
  }
  .hint {
    margin-top: 0.6rem;
    font-size: 0.72rem;
    color: var(--mdc-dim);
  }
</style>
