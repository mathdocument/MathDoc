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

  const VIRTUAL_THRESHOLD = 300;
  const VIRTUAL_ROW_HEIGHT = 88;
  const VIRTUAL_OVERSCAN = 6;
  let listEl = $state<HTMLUListElement | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);

  let virtualEntries = $derived.by(() => {
    const start = Math.max(0, Math.floor(scrollTop / VIRTUAL_ROW_HEIGHT) - VIRTUAL_OVERSCAN);
    const visibleCount = Math.ceil(viewportHeight / VIRTUAL_ROW_HEIGHT) + VIRTUAL_OVERSCAN * 2;
    const end = Math.min(items.length, start + Math.max(visibleCount, VIRTUAL_OVERSCAN * 2));
    return items.slice(start, end).map((item, offset) => ({ item, index: start + offset }));
  });

  $effect(() => {
    const element = listEl;
    if (!element) return;
    const updateHeight = () => { viewportHeight = element.clientHeight; };
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(element);
    return () => observer.disconnect();
  });

  function ariaLabel(n: NodeInfo): string {
    return `${n.broken ? "broken " : ""}${n.title} (${shortFnode(n.fnode)})`;
  }
</script>

{#snippet card(item: NodeInfo, i: number)}
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
    <span class="card-meta">
      <span class="fnode">{shortFnode(item.fnode)}</span>
      <span class="depth">depth {item.depth}</span>
    </span>
    <span class="title">{item.title}</span>
    <span class="path">{item.rel_path}</span>
  </button>
{/snippet}

<aside class="column" data-accent={accent} aria-label={title}>
  <header class="column-head">
    <span class="head-main">
      <span class="relation-icon">
        {#if accent === "up"}
          <ArrowUpRight size={15} strokeWidth={1.8} />
        {:else}
          <ArrowDownRight size={15} strokeWidth={1.8} />
        {/if}
      </span>
      <span>
        <small>{accent === "up" ? "Upstream" : "Downstream"}</small>
        <strong>{title}</strong>
      </span>
    </span>
    <span class="count">{items.length}</span>
  </header>
  <ul
    class="cards"
    class:virtual={items.length > VIRTUAL_THRESHOLD}
    bind:this={listEl}
    onscroll={(event) => { scrollTop = event.currentTarget.scrollTop; }}
  >
    {#if items.length > VIRTUAL_THRESHOLD}
      <li
        class="virtual-spacer"
        style={`height: ${items.length * VIRTUAL_ROW_HEIGHT}px`}
        aria-hidden="true"
      ></li>
      {#each virtualEntries as entry (entry.item.fnode)}
        {@const item = entry.item}
        {@const i = entry.index}
        <li
          class="virtual-row"
          style={`transform: translateY(${i * VIRTUAL_ROW_HEIGHT}px)`}
          aria-setsize={items.length}
          aria-posinset={i + 1}
        >
          {@render card(item, i)}
        </li>
      {/each}
    {:else}
      {#each items as item, i (item.fnode)}
        <li>
          {@render card(item, i)}
        </li>
      {/each}
    {/if}
    {#if items.length === 0}
      <li class="empty">No direct {title.toLowerCase()}</li>
    {/if}
  </ul>
</aside>

<style>
  .column {
    display: flex;
    flex-direction: column;
    min-width: 230px;
    width: 22%;
    max-width: 340px;
    height: 100%;
    overflow: hidden;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    background: color-mix(in srgb, var(--mdc-panel) 86%, transparent);
    box-shadow: 0 10px 35px color-mix(in srgb, var(--mdc-fg) 10%, transparent);
  }
  .column-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    min-height: 62px;
    padding: 0.65rem 0.8rem;
    border-bottom: 1px solid var(--mdc-border);
  }
  .head-main {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    min-width: 0;
  }
  .head-main > span:last-child {
    display: flex;
    flex-direction: column;
    gap: 0.12rem;
  }
  .relation-icon {
    display: grid;
    place-items: center;
    width: 29px;
    height: 29px;
    flex: 0 0 auto;
    border-radius: 8px;
  }
  .column[data-accent="up"] .relation-icon {
    color: var(--mdc-accent-up);
    background: rgba(182, 156, 255, 0.1);
  }
  .column[data-accent="down"] .relation-icon {
    color: var(--mdc-accent-down);
    background: rgba(99, 216, 178, 0.1);
  }
  .head-main small {
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.59rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .head-main strong {
    color: var(--mdc-fg-soft);
    font-size: 0.78rem;
    font-weight: 650;
  }
  .count {
    display: grid;
    place-items: center;
    min-width: 25px;
    height: 22px;
    padding-inline: 0.35rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    border-radius: 999px;
    font-family: var(--mdc-mono);
    font-size: 0.65rem;
    font-variant-numeric: tabular-nums;
  }
  .cards {
    list-style: none;
    margin: 0;
    padding: 0.45rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .cards > li {
    content-visibility: auto;
    contain-intrinsic-size: auto 70px;
  }
  .cards.virtual {
    position: relative;
    display: block;
  }
  .cards.virtual > .virtual-spacer {
    width: 1px;
    content-visibility: visible;
    pointer-events: none;
  }
  .virtual-row {
    position: absolute;
    top: 0.45rem;
    left: 0.45rem;
    right: 0.45rem;
    height: calc(88px - 0.25rem);
    content-visibility: visible !important;
  }
  .virtual-row .card {
    height: 100%;
    overflow: hidden;
  }
  .virtual-row .title {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }
  .card {
    display: flex;
    flex-direction: column;
    width: 100%;
    text-align: left;
    position: relative;
    gap: 0.25rem;
    padding: 0.62rem 0.68rem 0.65rem 0.78rem;
    border-radius: 7px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--mdc-fg);
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms ease, border-color 120ms ease, transform 120ms ease, box-shadow 120ms ease;
  }
  .card::before {
    content: "";
    position: absolute;
    inset: 0.58rem auto 0.58rem 0;
    width: 2px;
    border-radius: 999px;
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .column[data-accent="up"] .card::before {
    background: var(--mdc-accent-up);
  }
  .column[data-accent="down"] .card::before {
    background: var(--mdc-accent-down);
  }
  .card:hover:not(:disabled) {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border);
    transform: translateY(-1px);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--mdc-fg) 14%, transparent);
  }
  .card.selected {
    background: var(--mdc-card-selected);
    border-color: var(--mdc-border-strong);
  }
  .card.selected::before,
  .card.last-visited::before {
    opacity: 1;
  }
  .card:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .card.broken {
    border-color: var(--mdc-error);
  }
  .card.last-visited {
    background: color-mix(in srgb, var(--mdc-card-selected) 52%, transparent);
    border-color: var(--mdc-border);
  }
  .card.last-visited:hover:not(:disabled) {
    border-color: var(--mdc-border-strong);
  }
  .card-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .depth,
  .fnode {
    font-size: 0.62rem;
    color: var(--mdc-muted);
    font-variant-numeric: tabular-nums;
  }
  .fnode {
    color: var(--mdc-accent);
    font-family: var(--mdc-mono);
  }
  .title {
    color: var(--mdc-fg-soft);
    font-weight: 570;
    font-size: 0.82rem;
    line-height: 1.32;
    word-break: break-word;
  }
  .path {
    font-family: var(--mdc-mono);
    font-size: 0.61rem;
    color: var(--mdc-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    text-align: center;
    color: var(--mdc-muted);
    padding: 1.5rem 0.5rem;
    font-size: 0.72rem;
  }
</style>
