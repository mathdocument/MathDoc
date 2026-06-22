<script lang="ts">
  import type { NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";

  interface Props {
    items: NodeInfo[];
    title: string;
    accent: "up" | "down";
    lastVisitedFnode: string | null;
    selected: number;
    onSelect: (fnode: string, index: number) => void;
    onHover?: (index: number) => void;
  }

  let {
    items,
    title,
    accent,
    lastVisitedFnode,
    selected,
    onSelect,
    onHover,
  }: Props = $props();

  function ariaLabel(n: NodeInfo): string {
    return `${n.broken ? "broken " : ""}${n.title} (${shortFnode(n.fnode)})`;
  }
</script>

<aside class="column" data-accent={accent} aria-label={title}>
  <header class="column-head">
    <span class="label">{title}</span>
      <span class="count">{items.length}</span>
  </header>
  <ul class="cards">
    {#each items as item, i (item.fnode)}
      <li>
        <button
          class="card"
          class:broken={item.broken}
          class:selected={i === selected}
          class:last-visited={item.fnode === lastVisitedFnode}
          data-fnode={item.fnode}
          aria-label={ariaLabel(item)}
          onclick={() => onSelect(item.fnode, i)}
          onmouseenter={() => onHover?.(i)}
          disabled={item.broken}
        >
          <span class="depth">[{item.depth}]</span>
          <span class="fnode">{shortFnode(item.fnode)}</span>
          <span class="title">{item.title}</span>
          <span class="path">{item.rel_path}</span>
        </button>
      </li>
    {/each}
    {#if items.length === 0}
      <li class="empty">—</li>
    {/if}
  </ul>
</aside>

<style>
  .column {
    display: flex;
    flex-direction: column;
    min-width: 220px;
    width: 22%;
    max-width: 320px;
    height: 100%;
    overflow: hidden;
    border: 1px solid var(--mdc-border);
    border-radius: 6px;
    background: var(--mdc-panel);
  }
  .column[data-accent="up"] {
    border-top: 3px solid var(--mdc-accent-up);
  }
  .column[data-accent="down"] {
    border-top: 3px solid var(--mdc-accent-down);
  }
  .column-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding: 0.5rem 0.75rem;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--mdc-dim);
    border-bottom: 1px solid var(--mdc-border);
  }
  .count {
    color: var(--mdc-dim);
    font-variant-numeric: tabular-nums;
  }
  .cards {
    list-style: none;
    margin: 0;
    padding: 0.4rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .card {
    display: flex;
    flex-direction: column;
    width: 100%;
    text-align: left;
    padding: 0.5rem 0.6rem;
    border-radius: 4px;
    border: 1px solid transparent;
    background: var(--mdc-card);
    color: var(--mdc-fg);
    cursor: pointer;
    font-family: inherit;
    transition: background 0.08s ease, border-color 0.08s ease,
      transform 0.12s ease;
  }
  .card:hover:not(:disabled) {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border-strong);
  }
  .card.selected {
    background: var(--mdc-card-selected);
    border-color: var(--mdc-accent);
    transform: translateY(-1px);
  }
  .card:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .card.broken {
    border-color: var(--mdc-error);
  }
  .card.last-visited {
    border-color: var(--mdc-accent-down);
    box-shadow: inset 3px 0 0 var(--mdc-accent-down);
  }
  .card.last-visited:hover:not(:disabled) {
    border-color: var(--mdc-accent-down);
  }
  .depth,
  .fnode {
    font-size: 0.7rem;
    color: var(--mdc-dim);
    font-variant-numeric: tabular-nums;
  }
  .title {
    font-weight: 500;
    font-size: 0.9rem;
    line-height: 1.25;
    word-break: break-word;
  }
  .path {
    font-size: 0.72rem;
    color: var(--mdc-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    text-align: center;
    color: var(--mdc-dim);
    padding: 1rem 0;
  }
</style>
