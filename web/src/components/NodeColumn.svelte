<script lang="ts">
  import type { NodeInfo } from "../lib/types";
  import { shortFnode } from "../lib/format";
  import { ArrowDownRight, ArrowUpRight } from "@lucide/svelte";

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

  let direction = $derived(accent === "up" ? "Upstream" : "Downstream");
</script>

<aside class="column" data-accent={accent} aria-label={title}>
  <header class="column-head" title={`${direction} — ${title.toLowerCase()}`}>
    <span class="relation-icon" aria-hidden="true">
      {#if accent === "up"}
        <ArrowUpRight size={13} strokeWidth={2.1} />
      {:else}
        <ArrowDownRight size={13} strokeWidth={2.1} />
      {/if}
    </span>
    <strong>{title}</strong>
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
          <span class="title">{item.title}</span>
          <span class="card-meta">
            <span class="fnode">{shortFnode(item.fnode)}</span>
            <span class="path">{item.rel_path}</span>
            <span class="depth" title={`depth ${item.depth}`}>d{item.depth}</span>
          </span>
        </button>
      </li>
    {/each}
    {#if items.length === 0}
      <li class="empty">
        <span class="empty-rule" aria-hidden="true"></span>
        No direct {title.toLowerCase()}
      </li>
    {/if}
  </ul>
</aside>

<style>
  /* A quiet surface: one hairline, soft elevation, no inner boxes. */
  .column {
    display: flex;
    flex-direction: column;
    min-width: 232px;
    width: 21%;
    max-width: 330px;
    height: 100%;
    overflow: hidden;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-lg);
    background: var(--mdc-panel);
    box-shadow: var(--mdc-shadow-lg);
  }
  /* Compact single-line header: direction is carried by the arrow, not a
     second stacked label. */
  .column-head {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    flex-shrink: 0;
    min-height: 40px;
    padding: 0 0.75rem 0 0.7rem;
    border-bottom: 1px solid var(--mdc-border);
  }
  .relation-icon {
    display: grid;
    place-items: center;
    flex: 0 0 auto;
  }
  .column[data-accent="up"] .relation-icon {
    color: var(--mdc-accent-up);
  }
  .column[data-accent="down"] .relation-icon {
    color: var(--mdc-accent-down);
  }
  .column-head strong {
    flex: 1;
    min-width: 0;
    color: var(--mdc-fg-soft);
    font-size: var(--mdc-text-2xs);
    font-weight: 650;
    letter-spacing: var(--mdc-tracking-label);
    text-transform: uppercase;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    display: grid;
    place-items: center;
    min-width: 20px;
    height: 20px;
    padding-inline: 0.3rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border-radius: var(--mdc-radius-pill);
    font-family: var(--mdc-mono);
    font-size: 0.65rem;
    font-variant-numeric: tabular-nums;
  }
  .cards {
    list-style: none;
    margin: 0;
    padding: 0.5rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cards > li {
    content-visibility: auto;
    contain-intrinsic-size: auto 66px;
  }
  /* Title first, metadata second: scanning a column is a title-reading task. */
  .card {
    display: flex;
    flex-direction: column;
    width: 100%;
    text-align: left;
    position: relative;
    gap: 0.28rem;
    padding: 0.55rem 0.6rem 0.58rem 0.7rem;
    border-radius: 10px;
    border: 0;
    background: transparent;
    color: var(--mdc-fg);
    cursor: pointer;
    font-family: inherit;
    transition: background var(--mdc-dur-fast) var(--mdc-ease);
  }
  /* Directional rail, revealed on hover and kept for the active row. */
  .card::before {
    content: "";
    position: absolute;
    inset: 0.5rem auto 0.5rem 0;
    width: 2px;
    border-radius: var(--mdc-radius-pill);
    opacity: 0;
    transition: opacity var(--mdc-dur-fast) var(--mdc-ease);
  }
  .column[data-accent="up"] .card::before {
    background: var(--mdc-accent-up);
  }
  .column[data-accent="down"] .card::before {
    background: var(--mdc-accent-down);
  }
  .card:hover:not(:disabled) {
    background: var(--mdc-card-hover);
  }
  .card:hover:not(:disabled)::before {
    opacity: 0.55;
  }
  .card.selected {
    background: var(--mdc-card-selected);
  }
  .card.selected::before,
  .card.last-visited::before {
    opacity: 1;
  }
  .card:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .card.broken {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--mdc-error) 45%, transparent);
  }
  .card.last-visited {
    background: color-mix(in srgb, var(--mdc-card-selected) 45%, transparent);
  }
  .title {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    color: var(--mdc-fg);
    font-weight: 550;
    font-size: var(--mdc-text-sm);
    line-height: 1.35;
    letter-spacing: var(--mdc-tracking-tight);
    word-break: break-word;
  }
  .card-meta {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
    font-size: var(--mdc-text-2xs);
    font-family: var(--mdc-mono);
    color: var(--mdc-muted);
  }
  .fnode {
    flex: 0 0 auto;
    color: var(--mdc-accent);
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .depth {
    flex: 0 0 auto;
    color: var(--mdc-dim);
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 1.5rem 0.75rem;
    color: var(--mdc-muted);
    font-size: var(--mdc-text-xs);
  }
  .empty-rule {
    width: 26px;
    height: 1px;
    background: var(--mdc-border-strong);
  }
</style>
