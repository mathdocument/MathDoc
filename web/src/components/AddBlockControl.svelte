<script lang="ts">
  import { onDestroy } from "svelte";
  import { Braces, Plus } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import type { NodeDetail } from "../lib/types";
  import { trackMutation } from "../lib/unsaved";

  interface Props {
    fnode: string;
    revision: string;
    existingSrctypes: string[];
    onAdded?: (node: NodeDetail) => void;
  }
  let { fnode, revision, existingSrctypes, onAdded }: Props = $props();

  const ALL_SRCTYPES = ["text", "latex", "python", "lean", "rocq"] as const;

  let open = $state(false);
  let adding: string | null = $state(null);
  let error: string | null = $state(null);
  let alive = true;
  let rootEl = $state<HTMLDivElement | null>(null);
  let menuEl = $state<HTMLUListElement | null>(null);

  onDestroy(() => { alive = false; });

  let available = $derived(
    ALL_SRCTYPES.filter((s) => !existingSrctypes.includes(s)),
  );

  function toggle() {
    if (available.length === 0) return;
    open = !open;
    error = null;
  }

  function close() {
    if (adding) return;
    open = false;
    error = null;
  }

  // Close the menu on Escape or a click outside it.
  function onKeyDown(event: KeyboardEvent) {
    if (!open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "ArrowDown" && menuEl) {
      event.preventDefault();
      (menuEl.querySelector<HTMLButtonElement>("button") ?? menuEl).focus();
    }
  }

  function onPointerDown(event: PointerEvent) {
    if (!open) return;
    if (event.target instanceof Node && rootEl?.contains(event.target)) return;
    close();
  }

  async function add(srctype: string) {
    if (adding) return;
    const targetFnode = fnode;
    const targetRevision = revision;
    adding = srctype;
    const clearMutation = trackMutation();
    error = null;
    try {
      const node = await api.putBlock(targetFnode, srctype, "", targetRevision);
      clearMutation();
      if (!alive || fnode !== targetFnode) return;
      open = false;
      onAdded?.(node);
    } catch (e) {
      if (alive && fnode === targetFnode) error = errMsg(e);
    } finally {
      clearMutation();
      if (alive && fnode === targetFnode) adding = null;
    }
  }
</script>

<svelte:window onkeydown={onKeyDown} onpointerdown={onPointerDown} />

<div class="add-block" bind:this={rootEl}>
  <button
    class="add-btn"
    class:open
    onclick={toggle}
    aria-expanded={open}
    aria-haspopup="menu"
    disabled={available.length === 0}
    title={available.length === 0 ? "all srctypes already present" : "add source block"}
  ><Plus size={14} strokeWidth={2} />Add source block</button>
  {#if open}
    <ul class="menu" role="menu" bind:this={menuEl}>
      {#each available as s}
        <li role="none">
          <button
            class="item"
            role="menuitem"
            onclick={() => add(s)}
            disabled={adding !== null}
          >
            {#if adding === s}<span class="spinner">adding…</span>{:else}<Braces size={13} strokeWidth={1.8} />{s}{/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  {#if error}<div class="error-bar">{error}</div>{/if}
</div>

<style>
  .add-block {
    position: relative;
    display: inline-block;
  }
  .add-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.42rem;
    min-height: 34px;
    background: color-mix(in srgb, var(--mdc-card) 60%, transparent);
    color: var(--mdc-dim);
    border: 1px dashed var(--mdc-border-strong);
    border-radius: 8px;
    padding: 0 0.75rem;
    font-size: 0.72rem;
    font-weight: 600;
    cursor: pointer;
    font-family: inherit;
  }
  .add-btn:not(:disabled):hover {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-accent);
    color: var(--mdc-accent);
  }
  .add-btn.open {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-accent);
    color: var(--mdc-accent);
  }
  .add-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .menu {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0.35rem;
    position: absolute;
    z-index: 10;
    background: var(--mdc-panel-raised);
    border: 1px solid var(--mdc-border-strong);
    border-radius: 9px;
    box-shadow: var(--mdc-shadow-panel);
    min-width: 8rem;
    transform-origin: top left;
    animation: mdc-pop-in 150ms cubic-bezier(0.2, 0.8, 0.3, 1);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    width: 100%;
    text-align: left;
    background: transparent;
    color: var(--mdc-fg);
    border: none;
    padding: 0.45rem 0.55rem;
    font-family: var(--mdc-mono);
    font-size: 0.75rem;
    cursor: pointer;
    border-radius: var(--mdc-radius-sm);
  }
  .item:hover:not(:disabled) {
    background: var(--mdc-card-hover);
  }
  .item:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .spinner {
    color: var(--mdc-muted);
  }
  .error-bar {
    margin-top: 0.4rem;
    padding: 0.4rem 0.6rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    border-radius: var(--mdc-radius-sm);
  }
</style>
