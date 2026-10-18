<script lang="ts">
  // The document graph drawn as the collaboration page draws the proof graph
  // (lib/collab-layout.ts `layoutDocument`): a card per node with its Lean standing as an icon
  // (lib/node-tone.ts; a diamond for a definition), every node above its dependencies, edges
  // orthogonal with rounded corners. The selected node's edges take the colours of the
  // Referrers (up) and Dependencies (down) columns, and its neighbours' cards are outlined.
  // Clicking a card selects its node (again: clears it), as does clicking the background;
  // dragging the background pans, ⌘/Ctrl + wheel zooms. DepthGraph keeps its canvas for graphs
  // too large to lay out this way.
  import { tick } from "svelte";
  import { edgePath, type GraphLayout } from "../lib/collab-layout";
  import { shortFnode } from "../lib/format";
  import { nodeTone, toneLabel } from "../lib/node-tone";
  import type { GraphFull } from "../lib/types";
  import ToneIcon from "./ToneIcon.svelte";

  let { data, layout, selectedFnode, onSelect }: {
    data: GraphFull; layout: GraphLayout; selectedFnode: string | null; onSelect: (fnode: string | null) => unknown;
  } = $props();

  const MIN_ZOOM = 0.2, MAX_ZOOM = 2;
  let zoom = $state(1);
  /** Fitted to the view until zoomed by hand. */
  let fitted = true;
  let scroller = $state<HTMLDivElement>();
  const items = $derived(new Map(data.nodes.map((n) => [n.fnode, n])));
  const cards = $derived(layout.nodes.filter((n) => n.kind === "node"));
  const neighbours = $derived.by(() => {
    const near = new Set<string>();
    for (const e of layout.edges) {
      if (e.from === selectedFnode) near.add(e.to);
      if (e.to === selectedFnode) near.add(e.from);
    }
    return near;
  });
  const touches = (e: GraphLayout["edges"][number]) => !!selectedFnode && (e.from === selectedFnode || e.to === selectedFnode);
  /** The selected node's edges last, so they draw over the others. */
  const edges = $derived([...layout.edges].sort((a, b) => Number(touches(a)) - Number(touches(b))));
  const fitText = (s: string, width: number, px: number) => {
    const max = Math.max(4, Math.floor(width / px));
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  };

  /** The room inside the scroller's padding. */
  function room() {
    const el = scroller!, style = getComputedStyle(el);
    return {
      w: el.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
      h: el.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom),
    };
  }
  /** The whole graph in view, never larger than life. */
  export function fit() {
    if (!scroller) return;
    fitted = true;
    const { w, h } = room();
    if (w <= 0 || h <= 0) return;
    zoom = Math.max(MIN_ZOOM, Math.min(1, w / layout.width, h / layout.height));
  }
  /** Zoom by `factor`, keeping the point `at` of the scroller (default: its centre) in place. */
  export function zoomBy(factor: number, at: { x: number; y: number } | null = null) {
    if (!scroller) return;
    const el = scroller, next = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom * factor));
    const px = at ? at.x : el.clientWidth / 2, py = at ? at.y : el.clientHeight / 2;
    const ratio = next / zoom;
    fitted = false;
    zoom = next;
    void tick().then(() => {
      el.scrollLeft = (el.scrollLeft + px) * ratio - px;
      el.scrollTop = (el.scrollTop + py) * ratio - py;
    });
  }
  function onWheel(event: WheelEvent) {
    if (!(event.ctrlKey || event.metaKey) || !scroller) return;
    event.preventDefault();
    const rect = scroller.getBoundingClientRect();
    zoomBy(Math.exp(-event.deltaY * 0.01), { x: event.clientX - rect.left, y: event.clientY - rect.top });
  }
  // Fit on first measure and whenever the view or the graph changes size, until zoomed by hand.
  $effect(() => {
    const el = scroller;
    void layout.width;
    void layout.height;
    if (!el) return;
    if (fitted) fit();
    const ro = new ResizeObserver(() => { if (fitted) fit(); });
    ro.observe(el);
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => { ro.disconnect(); el.removeEventListener("wheel", onWheel); };
  });
  // A node selected elsewhere (search, history) scrolls into view.
  $effect(() => {
    const id = selectedFnode;
    void zoom;
    if (!id || !scroller) return;
    void tick().then(() => {
      const card = scroller?.querySelector<SVGGElement>(`[data-fnode="${CSS.escape(id)}"]`);
      if (!card || !scroller) return;
      const r = card.getBoundingClientRect(), view = scroller.getBoundingClientRect();
      if (r.left >= view.left && r.right <= view.right && r.top >= view.top && r.bottom <= view.bottom) return;
      scroller.scrollBy({
        left: r.left + r.width / 2 - (view.left + view.width / 2),
        top: r.top + r.height / 2 - (view.top + view.height / 2),
        behavior: "smooth",
      });
    });
  });
  // Drag the background to pan; a click on it clears the selection.
  function pan(event: PointerEvent) {
    if (!scroller || event.button !== 0 || (event.target as Element).closest(".card")) return;
    const el = scroller, x = event.clientX, y = event.clientY, left = el.scrollLeft, top = el.scrollTop;
    let moved = false;
    el.setPointerCapture(event.pointerId);
    const move = (e: PointerEvent) => {
      moved ||= Math.hypot(e.clientX - x, e.clientY - y) > 3;
      el.scrollLeft = left - (e.clientX - x);
      el.scrollTop = top - (e.clientY - y);
    };
    const end = (e: PointerEvent) => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", end);
      el.removeEventListener("pointercancel", end);
      if (e.type === "pointerup" && !moved && selectedFnode) onSelect(null);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", end);
    el.addEventListener("pointercancel", end);
  }
  const choose = (fnode: string) => onSelect(fnode === selectedFnode ? null : fnode);
</script>

<div class="doc-graph" bind:this={scroller} onpointerdown={pan} role="presentation">
  <svg width={layout.width * zoom} height={layout.height * zoom} viewBox={`0 0 ${layout.width} ${layout.height}`}
    role="group" aria-label="Document graph: each node above its dependencies">
    {#each edges as e (`${e.from}>${e.to}`)}
      <path class="edge" class:down={selectedFnode !== null && e.from === selectedFnode}
        class:up={selectedFnode !== null && e.to === selectedFnode} class:back={e.back} d={edgePath(e)} />
    {/each}
    {#each cards as n (n.id)}
      {@const item = items.get(n.id)}
      {@const tone = item ? nodeTone(item) : "no_code"}
      <g class="card" class:selected={n.id === selectedFnode} class:near={neighbours.has(n.id)} data-fnode={n.id}
        transform={`translate(${n.x},${n.y})`} role="button" tabindex="0" aria-pressed={n.id === selectedFnode}
        aria-label={`${n.label}, ${toneLabel[tone]}`}
        onclick={() => choose(n.id)}
        onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); choose(n.id); } }}>
        <title>{n.label} · {shortFnode(n.id)} · {toneLabel[tone]}</title>
        <rect width={n.w} height={n.h} rx="9" />
        <g transform="translate(17,16)"><ToneIcon {tone} /></g>
        <text class="name" x="30" y="16" dominant-baseline="central">{fitText(n.label, n.w - 40, 7.4)}</text>
        <text class="meta" x="30" y="33" dominant-baseline="central">{fitText(toneLabel[tone], n.w - 38, 5.8)}</text>
      </g>
    {/each}
  </svg>
</div>

<style>
  /* Room for the legend above and the zoom controls below. */
  .doc-graph { position: absolute; inset: 0; display: flex; overflow: auto; padding: 3.75rem 1rem 3.5rem; cursor: grab; touch-action: none; }
  .doc-graph:active { cursor: grabbing; }
  svg { display: block; flex: 0 0 auto; margin: auto; overflow: visible; }
  .edge { fill: none; stroke: var(--mdc-border-strong); stroke-width: 1.3; stroke-linecap: round; transition: stroke var(--mdc-dur-fast) var(--mdc-ease); }
  .edge.back { stroke-dasharray: 4 3; }
  .edge.down { stroke: var(--mdc-accent-down); stroke-width: 2; }
  .edge.up { stroke: var(--mdc-accent-up); stroke-width: 2; }
  .card { cursor: pointer; outline: none; }
  .card rect { fill: var(--mdc-panel); stroke: var(--mdc-border-strong); stroke-width: 1; transition: stroke var(--mdc-dur-fast) var(--mdc-ease); }
  .card:hover rect { stroke: var(--mdc-dim); }
  .card.near rect { stroke: var(--mdc-fg-soft); }
  .card.selected rect, .card:focus-visible rect { stroke: var(--mdc-accent); stroke-width: 2; }
  .name { fill: var(--mdc-fg); font: 600 13px var(--mdc-font); }
  .meta { fill: var(--mdc-muted); font: 11px var(--mdc-font); }
</style>
