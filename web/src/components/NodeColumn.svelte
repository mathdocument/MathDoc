<script lang="ts">
  import { tick } from "svelte";
  import type { NodePreview } from "../lib/types";
  import FormalStatus from "./FormalStatus.svelte";
  import { shortFnode } from "../lib/format";
  import { ArrowDownRight, ArrowUpRight } from "@lucide/svelte";

  interface Props {
    items: NodePreview[];
    title: string;
    accent: "up" | "down";
    lastVisitedFnode: string | null;
    context: string | null;
    active: boolean;
    onSelect: (fnode: string) => void;
  }

  let {
    items,
    title,
    accent,
    lastVisitedFnode,
    context,
    active,
    onSelect,
  }: Props = $props();

  function ariaLabel(n: NodePreview): string {
    return `${n.broken ? "broken " : ""}${n.title} (${shortFnode(n.fnode)})`;
  }

  let direction = $derived(accent === "up" ? "Upstream" : "Downstream");
  let depthWidth = $derived(items.reduce((width, item) => Math.max(width, String(item.depth).length + 1), 4));
  // Large lists reserve two title lines per row, so offscreen cards need no DOM
  // or measurement. Small lists retain their natural, compact row heights.
  const ROW_HEIGHT = 76;
  let list: HTMLUListElement;
  let scrollTop = $state(0);
  let height = $state(0);
  let virtual = $derived(items.length > 100);
  let start = $derived(virtual ? Math.max(0, Math.min(items.length - 1, Math.floor(scrollTop / ROW_HEIGHT) - 5)) : 0);
  let end = $derived(virtual ? Math.min(items.length, start + Math.ceil(height / ROW_HEIGHT) + 11) : items.length);
  let visible = $derived(items.slice(start, end));

  $effect(() => {
    void context;
    scrollTop = 0;
    if (list) list.scrollTop = 0;
  });

  async function moveFocus(event: KeyboardEvent, index: number) {
    if (!virtual || event.altKey || event.ctrlKey || event.metaKey) return;
    let target = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: items.length - 1 }[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const step = event.key === "ArrowUp" || event.key === "End" ? -1 : 1;
    while (target >= 0 && target < items.length && items[target]!.broken) target += step;
    if (target < 0 || target >= items.length) return;
    // A key can arrive just after showing the column, before ResizeObserver.
    height = list.clientHeight;
    const top = target * ROW_HEIGHT;
    if (top < list.scrollTop || top + ROW_HEIGHT > list.scrollTop + height) {
      list.scrollTop = Math.max(0, top - (height - ROW_HEIGHT) / 2);
      scrollTop = list.scrollTop;
    }
    await tick();
    list.querySelector<HTMLButtonElement>(`[data-index="${target}"]`)?.focus({ preventScroll: true });
  }
</script>

<aside class="column" class:hidden={!active} data-accent={accent} aria-label={title} style:--depth-width={`${depthWidth}ch`}>
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
  <ul class="cards" class:virtual bind:this={list} bind:clientHeight={height}
    onscroll={() => scrollTop = list.scrollTop} style:--row-height={`${ROW_HEIGHT}px`}>
    {#if virtual}<li class="spacer" aria-hidden="true" style:height={`${items.length * ROW_HEIGHT}px`}></li>{/if}
    {#each visible as item, i (item.fnode)}
      <li aria-posinset={start + i + 1} aria-setsize={items.length}
        style:top={virtual ? `calc(0.5rem + ${(start + i) * ROW_HEIGHT}px)` : null}>
        <button
          class="card"
          class:broken={item.broken}
          class:last-visited={item.fnode === lastVisitedFnode}
          data-fnode={item.fnode}
          data-index={start + i}
          aria-label={ariaLabel(item)}
          onclick={() => onSelect(item.fnode)}
          onkeydown={(event) => moveFocus(event, start + i)}
          disabled={item.broken}
        >
          <span class="title">{item.title}</span>
          <span class="card-meta">
            <span class="fnode">{shortFnode(item.fnode)}</span>
            <span class="depth" title={`depth ${item.depth}`}>d{item.depth}</span>
            <span class="meta-divider" aria-hidden="true"></span>
            <FormalStatus language="Lean" status={item.formalization.lean} compact />
            <FormalStatus language="Rocq" status={item.formalization.rocq} compact />
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
  .column.hidden { display: none; }
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
  .cards.virtual { display: block; position: relative; overflow-anchor: none; }
  .virtual > li:not(.spacer) { position: absolute; left: 0.5rem; right: 0.5rem; height: var(--row-height); padding-bottom: 2px; }
  .virtual .card { height: 100%; justify-content: space-between; }
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
  .card:focus-visible {
    background: var(--mdc-card-selected);
  }
  .card:focus-visible::before,
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
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    min-width: 0;
    font-size: var(--mdc-text-2xs);
    color: var(--mdc-muted);
  }
  .fnode, .depth { font-family: var(--mdc-mono); }
  .fnode {
    flex: 0 0 auto;
    width: 8ch;
    color: var(--mdc-accent);
  }
  .depth {
    flex: 0 0 auto;
    width: var(--depth-width);
    color: var(--mdc-dim);
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
  }
  .meta-divider {
    flex: 0 0 1px;
    height: 12px;
    margin-inline-end: 0.5em;
    background: var(--mdc-border-strong);
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
