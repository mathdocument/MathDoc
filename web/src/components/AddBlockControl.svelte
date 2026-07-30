<script lang="ts">
  import { onDestroy } from "svelte";
  import { Braces, Plus } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { setMutationPending } from "../lib/unsaved";

  interface Props {
    fnode: string;
    existingSrctypes: string[];
    onAdded?: () => void;
  }
  let { fnode, existingSrctypes, onAdded }: Props = $props();

  const ALL_SRCTYPES = ["text", "latex", "python", "lean", "rocq"] as const;

  let open = $state(false);
  let adding: string | null = $state(null);
  let error: string | null = $state(null);
  const mutationId = Symbol("add block mutation");
  let alive = true;

  onDestroy(() => { alive = false; });

  let available = $derived(
    ALL_SRCTYPES.filter((s) => !existingSrctypes.includes(s)),
  );

  function toggle() {
    if (available.length === 0) return;
    open = !open;
    error = null;
  }

  async function add(srctype: string) {
    if (adding) return;
    const targetFnode = fnode;
    adding = srctype;
    let pending = true;
    setMutationPending(mutationId, true);
    error = null;
    try {
      await api.putBlock(targetFnode, srctype, "");
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive || fnode !== targetFnode) return;
      open = false;
      onAdded?.();
    } catch (e) {
      if (alive && fnode === targetFnode) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive && fnode === targetFnode) adding = null;
    }
  }
</script>

<div class="add-block">
  <button
    class="add-btn"
    onclick={toggle}
    disabled={available.length === 0}
    title={available.length === 0 ? "all srctypes already present" : "add source block"}
  ><Plus size={14} strokeWidth={2} />Add source block</button>
  {#if open}
    <ul class="menu">
      {#each available as s}
        <li>
          <button
            class="item"
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
    background: rgba(21, 30, 43, 0.6);
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
