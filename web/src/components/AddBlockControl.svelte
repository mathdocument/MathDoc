<script lang="ts">
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";

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
    adding = srctype;
    error = null;
    try {
      await api.putBlock(fnode, srctype, "");
      open = false;
      onAdded?.();
    } catch (e) {
      error = errMsg(e);
    } finally {
      adding = null;
    }
  }
</script>

<div class="add-block">
  <button
    class="add-btn"
    onclick={toggle}
    disabled={available.length === 0}
    title={available.length === 0 ? "all srctypes already present" : "add source block"}
  >+ add source block</button>
  {#if open}
    <ul class="menu">
      {#each available as s}
        <li>
          <button
            class="item"
            onclick={() => add(s)}
            disabled={adding !== null}
          >
            {#if adding === s}<span class="spinner">adding…</span>{:else}{s}{/if}
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
    background: var(--mdc-card);
    color: var(--mdc-fg);
    border: 1px dashed var(--mdc-border-strong);
    border-radius: 4px;
    padding: 0.4rem 0.8rem;
    font-size: 0.8rem;
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
    margin: 0.3rem 0 0;
    padding: 0.3rem;
    position: absolute;
    z-index: 10;
    background: var(--mdc-panel);
    border: 1px solid var(--mdc-border-strong);
    border-radius: 4px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.3);
    min-width: 8rem;
  }
  .item {
    width: 100%;
    text-align: left;
    background: transparent;
    color: var(--mdc-fg);
    border: none;
    padding: 0.35rem 0.5rem;
    font-family: var(--mdc-mono);
    font-size: 0.85rem;
    cursor: pointer;
    border-radius: 3px;
  }
  .item:hover:not(:disabled) {
    background: var(--mdc-card-hover);
  }
  .item:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .spinner {
    color: var(--mdc-dim);
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
</style>
