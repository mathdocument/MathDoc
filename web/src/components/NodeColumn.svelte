<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import type { NodePreview } from "../lib/types";
  import FormalStatus from "./FormalStatus.svelte";
  import { shortFnode } from "../lib/format";
  import { ArrowDownRight, ArrowUpRight, Search } from "@lucide/svelte";

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
    return `${n.title} (${shortFnode(n.fnode)})`;
  }

  let direction = $derived(accent === "up" ? "Upstream" : "Downstream");
  let depthWidth = $derived(items.reduce((width, item) => Math.max(width, String(item.depth).length + 1), 4));
  let query = $state("");
  let needle = $derived(query.trim().toLowerCase());
  let matches = $derived(needle ? items.filter(item =>
    item.title.toLowerCase().includes(needle) || item.fnode.toLowerCase().includes(needle)) : items);
  // Only visible cards are measured. Unvisited rows use an estimate, never a
  // fixed CSS height, so both small and large lists keep natural title wrapping.
  const ESTIMATED_HEIGHT = 58;
  const measurements = new Map<string, number>();
  let measured = $state(0);
  let list: HTMLUListElement;
  let scrollTop = $state(0);
  let height = $state(0);
  let width = $state(0);
  let virtual = $derived(matches.length > 100);
  let offsets = $derived.by(() => {
    void measured;
    const values = [0];
    for (const item of matches) values.push(values[values.length - 1]! + (measurements.get(item.fnode) ?? ESTIMATED_HEIGHT));
    return values;
  });

  function rowAt(position: number): number {
    let low = 0, high = matches.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (offsets[middle + 1]! <= position) low = middle + 1;
      else high = middle;
    }
    return Math.min(low, Math.max(0, matches.length - 1));
  }

  let start = $derived(virtual ? Math.max(0, rowAt(scrollTop) - 5) : 0);
  let end = $derived(virtual ? Math.min(matches.length, rowAt(scrollTop + height) + 6) : matches.length);
  let visible = $derived(matches.slice(start, end));

  const pendingMeasurements = new Map<HTMLLIElement, number>();
  let measureFrame = 0;
  function flushMeasurements() {
    measureFrame = 0;
    const anchor = rowAt(scrollTop);
    let correction = 0, changed = false;
    for (const [row, size] of pendingMeasurements) {
      if (!row.isConnected || size === 0) continue;
      const id = row.dataset.fnode!;
      const previous = measurements.get(id) ?? ESTIMATED_HEIGHT;
      if (Math.abs(size - previous) < 0.1) continue;
      if (Number(row.dataset.index) < anchor) correction += size - previous;
      measurements.set(id, size);
      changed = true;
    }
    pendingMeasurements.clear();
    if (changed) measured++;
    if (virtual && correction && list) {
      // Keep the first visible card in place when overscan rows are measured.
      list.scrollTop += correction;
      scrollTop = list.scrollTop;
    }
  }
  const observer = new ResizeObserver(entries => {
    for (const entry of entries) {
      const row = entry.target as HTMLLIElement;
      pendingMeasurements.set(row, entry.borderBoxSize[0]?.blockSize ?? row.getBoundingClientRect().height);
    }
    // Updating the virtual window during ResizeObserver delivery can mount
    // more observed rows and trigger WebKit's resize-loop error.
    if (!measureFrame) measureFrame = requestAnimationFrame(flushMeasurements);
  });
  onDestroy(() => { observer.disconnect(); cancelAnimationFrame(measureFrame); });
  function measure(row: HTMLLIElement) {
    observer.observe(row);
    return { destroy: () => { observer.unobserve(row); pendingMeasurements.delete(row); } };
  }

  $effect(() => {
    void context;
    query = "";
  });

  let measuredWidth = 0;
  $effect(() => {
    if (!width || width === measuredWidth) return;
    measuredWidth = width;
    untrack(() => {
      measurements.clear();
      // Retain exact heights for mounted rows even if their height did not
      // change; ResizeObserver only reports rows whose dimensions changed.
      for (const row of list.querySelectorAll<HTMLLIElement>("li[data-fnode]")) {
        measurements.set(row.dataset.fnode!, row.getBoundingClientRect().height);
      }
      measured++;
    });
  });

  $effect(() => {
    void context;
    void needle;
    scrollTop = 0;
    if (list) list.scrollTop = 0;
  });

  async function moveFocus(event: KeyboardEvent, index: number) {
    if (!virtual || event.altKey || event.ctrlKey || event.metaKey) return;
    const target = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: matches.length - 1 }[event.key];
    if (target === undefined) return;
    event.preventDefault();
    if (target < 0 || target >= matches.length) return;
    // A key can arrive just after showing the column, before ResizeObserver.
    height = list.clientHeight;
    const top = offsets[target]!;
    const rowHeight = offsets[target + 1]! - top;
    if (top < list.scrollTop || top + rowHeight > list.scrollTop + height) {
      list.scrollTop = Math.max(0, top - (height - rowHeight) / 2);
      scrollTop = list.scrollTop;
    }
    await tick();
    list.querySelector<HTMLButtonElement>(`button[data-index="${target}"]`)?.focus({ preventScroll: true });
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
    <span class="count">{needle ? `${matches.length}/${items.length}` : items.length}</span>
  </header>
  <label class="filter">
    <Search size={13} strokeWidth={1.8} aria-hidden="true" />
    <input type="search" bind:value={query} aria-label={`Filter ${title.toLowerCase()}`}
      placeholder="Filter by title or fnode..." spellcheck="false" />
  </label>
  <ul class="cards" class:virtual bind:this={list} bind:clientHeight={height} bind:clientWidth={width}
    onscroll={() => scrollTop = list.scrollTop}>
    {#if virtual}<li class="spacer" aria-hidden="true" style:height={`${offsets[matches.length]}px`}></li>{/if}
    {#each visible as item, i (item.fnode)}
      <li use:measure data-fnode={item.fnode} data-index={start + i}
        aria-posinset={start + i + 1} aria-setsize={matches.length}
        style:top={virtual ? `calc(0.5rem + ${offsets[start + i]}px)` : null}>
        <button
          class="card"
          class:last-visited={item.fnode === lastVisitedFnode}
          data-fnode={item.fnode}
          data-index={start + i}
          aria-label={ariaLabel(item)}
          onclick={() => onSelect(item.fnode)}
          onkeydown={(event) => moveFocus(event, start + i)}
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
    {#if matches.length === 0}
      <li class="empty">
        <span class="empty-rule" aria-hidden="true"></span>
        {needle ? "No matching nodes" : `No direct ${title.toLowerCase()}`}
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
  }
  .filter { display: flex; align-items: center; gap: 0.4rem; flex-shrink: 0; margin: 0.5rem 0.65rem 0; padding: 0.35rem 0.45rem; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-sm); color: var(--mdc-muted); }
  .filter:focus-within { border-color: var(--mdc-border-strong); }
  .filter input { flex: 1; min-width: 0; width: 100%; padding: 0; background: transparent; border: 0; color: var(--mdc-fg); font: inherit; font-size: var(--mdc-text-xs); }
  .filter input:focus-visible { outline: none; }
  .filter input::placeholder { color: var(--mdc-muted); }
  .cards > li[data-fnode] { flex-shrink: 0; padding-bottom: 2px; }
  .cards.virtual { display: block; position: relative; overflow-anchor: none; }
  .virtual > li[data-fnode] { position: absolute; left: 0.5rem; right: 0.5rem; }
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
