<script lang="ts">
  // The proof graph as an AND-OR graph (lib/collab-layout.ts `layoutAndOr`): goal cards with a
  // status icon and a line of what is going on; under each goal its ways to prove it as
  // junctions (a check: a complete proof; a dot: a decomposition, with a cup when it needs
  // several premises), wired orthogonally. The branch's route (`activeRoute`) is solid, every way it does not take
  // dashed, and goals off the route faded. Clicking a junction previews the route through that
  // way; "Use on BRANCH" sends the picks. Clicking a card selects the goal.
  import { Maximize2, ZoomIn, ZoomOut } from "@lucide/svelte";
  import { edgePath, layoutAndOr, type GraphEdge, type GraphNode } from "../../lib/collab-layout";
  import { actorName, type Collab, type Selection } from "../../lib/collab.svelte";
  import { activeRoute, goalStatusLabel, timeLeft } from "../../lib/coordination";
  let { c, selection, onSelect }: { c: Collab; selection: Selection; onSelect: (s: Selection) => void } = $props();
  let zoom = $state(1);
  let fitted = $state(true);
  let inactive = $state(false);
  /** Goal → certificate: ways chosen here, not yet sent. */
  let draft = $state<Record<string, string>>({});
  let scroller = $state<HTMLDivElement>();
  const branch = $derived(c.branch?.branch ?? "this branch");
  const layout = $derived(c.board ? layoutAndOr(c.board, c.name, { inactive }) : null);
  const facts = $derived(c.board?.facts ?? {});
  const picks = $derived(c.view?.picks ?? {});
  const base = $derived(c.board ? activeRoute(c.board, picks) : null);
  const route = $derived(c.board ? activeRoute(c.board, picks, draft) : null);
  /** The draft's choices that change the route: goal → certificate. */
  const changes = $derived(Object.entries(draft).filter(([goal, id]) => base?.choice.get(goal) !== id && route?.choice.get(goal) === id));
  const incoming = $derived.by(() => {
    const n = new Map<string, number>();
    for (const e of layout?.edges ?? []) if (!e.back && e.to.startsWith("g:")) n.set(e.to, (n.get(e.to) ?? 0) + 1);
    return n;
  });
  const factOf = (id: string) => (id.startsWith("f:") ? id.slice(2) : undefined);
  /** Whether an edge is on the route: a goal's edge to its chosen way, a chosen way's to its premises. */
  const active = (e: GraphEdge) => {
    if (!route || e.back) return false;
    const fromFact = factOf(e.from), toFact = factOf(e.to);
    if (toFact) return route.choice.get(facts[toFact]?.goal) === toFact && route.goals.has(facts[toFact]?.goal);
    return !!fromFact && route.facts.has(fromFact);
  };
  const fit = (s: string, width: number, px: number) => {
    const max = Math.max(4, Math.floor(width / px));
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  };
  /** The card's second line: who holds the task and for how long, notes, how many ways use it. */
  const meta = (n: GraphNode) => {
    const b = c.board!;
    const parts: string[] = [];
    const task = Object.values(b.tasks).find((t) => t.goal === n.goal && t.kind === "prove" && t.attempt);
    const a = task ? b.attempts[task.attempt!] : undefined;
    // "1 h 31 min left" reads 1h31m on a card.
    const brief = (ms: number) => timeLeft(ms).replace(" left", "").replace(/ h ?/, "h").replace(" min", "m").replace(/ s$/, "s");
    if (a && !a.outcome) parts.push(`${actorName(a.actor).split(" ")[0]} · ${brief(a.expires)}`);
    const notes = b.goals[n.goal]?.notes?.length ?? 0;
    if (notes) parts.push(`${notes} ${notes === 1 ? "note" : "notes"}`);
    const uses = incoming.get(n.id) ?? 0;
    if (uses > 1) parts.push(`used ${uses}×`);
    return parts.length ? parts.join(" · ") : goalStatusLabel[c.statusOf(n.goal)];
  };
  const tone = (n: GraphNode) => (c.board?.goals[n.goal]?.refuted ? "refuted" : c.statusOf(n.goal));
  const way = (n: GraphNode) => {
    const f = facts[n.fact!];
    return f?.premises.length ? `decomposition of ${c.name(n.goal)} ⇐ ${f.premises.map(c.name).join(", ")}` : `complete proof of ${c.name(n.goal)}`;
  };
  const wayTitle = (n: GraphNode) => {
    const f = facts[n.fact!];
    const chosen = route?.choice.get(n.goal) === n.fact;
    const why = !f ? "" : f.state === "proposed" ? "awaiting review, so no route can use it yet"
      : f.state !== "active" ? `${f.state}, so no route uses it`
      : chosen ? `${branch}'s route proves ${c.name(n.goal)} this way` : `click to prove ${c.name(n.goal)} this way on ${branch}'s route`;
    return `${way(n)} — ${why}`;
  };
  function choose(n: GraphNode) {
    const f = facts[n.fact!];
    onSelect({ kind: "fact", fact: n.fact! });
    if (!f || f.state !== "active" || !f.available) return;
    if (base?.choice.get(f.goal) === f.id) delete draft[f.goal];
    else draft[f.goal] = f.id;
  }
  async function useRoute() {
    for (const [goal, certificate] of changes) {
      await c.run({ type: "pick", branch: c.branch!.branch, goal, certificate });
      if (c.refused) return;
    }
    draft = {};
  }
  const zoomBy = (d: number) => { fitted = false; zoom = Math.round(Math.min(2, Math.max(0.3, zoom + d)) * 20) / 20; };
  /** As wide as the column, never larger than life. */
  const fitWidth = () => {
    if (scroller && layout) zoom = Math.max(0.3, Math.min(1, Math.floor(((scroller.clientWidth - 4) / layout.width) * 20) / 20));
  };
  // Fit the graph to the column's width until zoomed by hand.
  $effect(() => {
    const el = scroller;
    void layout?.width;
    if (!el) return;
    const ro = new ResizeObserver(() => { if (fitted) fitWidth(); });
    ro.observe(el);
    return () => ro.disconnect();
  });
  // Drag the background to pan.
  function pan(event: PointerEvent) {
    if (!scroller || (event.target as Element).closest(".pg-card, .pg-way")) return;
    const el = scroller, x = event.clientX, y = event.clientY, left = el.scrollLeft, top = el.scrollTop;
    el.setPointerCapture(event.pointerId);
    const move = (e: PointerEvent) => { el.scrollLeft = left - (e.clientX - x); el.scrollTop = top - (e.clientY - y); };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", () => el.removeEventListener("pointermove", move), { once: true });
  }
</script>

<section aria-label="Proof graph">
  <div class="graph-tools">
    <button onclick={() => zoomBy(-0.1)} aria-label="Zoom out" title="Zoom out"><ZoomOut size={15} /></button>
    <span>{Math.round(zoom * 100)}%</span>
    <button onclick={() => zoomBy(0.1)} aria-label="Zoom in" title="Zoom in"><ZoomIn size={15} /></button>
    <button onclick={() => { fitted = true; fitWidth(); }} aria-label="Fit to width" title="Fit to width"><Maximize2 size={14} /></button>
    {#if route}<span class="route-state" class:closes={route.closes}>{branch}'s route {route.closes ? "closes" : `has ${route.open.length} open ${route.open.length === 1 ? "goal" : "goals"}`}</span>{/if}
    <label class="toggle" title="Paused, retired and refuted certificates"><input type="checkbox" bind:checked={inactive} /> Show ways not in use</label>
  </div>
  <div class="legend" aria-label="Legend">
    <span><i class="icon" data-tone="insufficient"></i>{goalStatusLabel.insufficient}</span>
    <span><i class="icon" data-tone="derivable"></i>{goalStatusLabel.derivable}</span>
    <span><i class="icon filled" data-tone="certified"></i>{goalStatusLabel.certified}</span>
    <span><svg width="22" height="14" aria-hidden="true"><circle cx="11" cy="4" r="3.5" class="pg-dot" /><path d="M4 7 A8 8 0 0 0 18 7" class="pg-cup" /></svg>a way to prove it; the cup: needs every premise</span>
    <span><svg width="22" height="10" aria-hidden="true"><path d="M1 5 L21 5" class="edge active" /></svg>{branch}'s route</span>
    <span><svg width="22" height="10" aria-hidden="true"><path d="M1 5 L21 5" class="edge idle" /></svg>ways it does not take</span>
  </div>
  {#if changes.length}
    <p class="route-preview" role="status" aria-label="Route preview">
      <strong>Preview: {changes.length} {changes.length === 1 ? "change" : "changes"} to {branch}'s route.</strong>
      {#if route?.closes}The route then closes: it can be assembled.{:else}The route then leaves {route?.open.length} open: {route?.open.slice(0, 4).map(c.name).join(", ")}{(route?.open.length ?? 0) > 4 ? ", …" : ""}.{/if}
      <span class="spacer"></span>
      {#if c.canPick && !c.frozen}<button class="primary" disabled={c.busy} onclick={() => void useRoute()}>Use on {branch}</button>{:else}<span class="muted">Only the owner or an editor of {branch} can change its route.</span>{/if}
      <button onclick={() => (draft = {})}>Reset</button>
    </p>
  {/if}
  {#if layout && route}
    <div class="graph-scroll" bind:this={scroller} onpointerdown={pan} role="presentation">
      <svg width={layout.width * zoom} height={layout.height * zoom} viewBox={`0 0 ${layout.width} ${layout.height}`} role="img" aria-label={`Proof graph of ${c.name(c.board!.root)}`}>
        <!-- Ways not taken first, so that a stem shared with the route shows solid. -->
        {#each [...layout.edges].sort((a, b) => Number(active(a)) - Number(active(b))) as e (`${e.from}>${e.to}`)}<path class="edge" class:active={active(e)} class:idle={!active(e)} class:back={e.back} data-from={e.from} d={edgePath(e)} />{/each}
        {#each layout.nodes as n (n.id)}
          {#if n.kind === "goal"}
            {@const t = tone(n)}
            <g class="pg-card" class:selected={selection?.kind === "goal" && selection.goal === n.goal} class:off={!route.goals.has(n.goal)}
              transform={`translate(${n.x},${n.y})`} role="button" tabindex="0" aria-label={c.name(n.goal)}
              onclick={() => onSelect({ kind: "goal", goal: n.goal })}
              onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onSelect({ kind: "goal", goal: n.goal }); } }}>
              <title>{c.name(n.goal)} · {goalStatusLabel[c.statusOf(n.goal)]}{route.open.includes(n.goal) ? ` · open on ${branch}'s route` : ""}</title>
              <rect width={n.w} height={n.h} rx="9" />
              <g transform="translate(17,16)" data-tone={t} class="pg-status">
                {#if t === "certified"}<circle r="5.5" class="filled" /><path d="M-2.6 0 L-0.6 2 L2.8 -1.8" />{:else}<circle r="5" />{/if}
              </g>
              <text class="pg-name" x="30" y="16" dominant-baseline="central">{fit(n.label, n.w - 40, 7.4)}</text>
              <text class="pg-meta" x="30" y="33" dominant-baseline="central">{fit(meta(n), n.w - 38, 5.8)}</text>
            </g>
          {:else if n.kind === "fact"}
            {@const f = facts[n.fact!]}
            {@const chosen = route.choice.get(n.goal) === n.fact && route.goals.has(n.goal)}
            {@const cx = n.w / 2}
            <g class="pg-way" class:chosen class:selected={selection?.kind === "fact" && selection.fact === n.fact}
              data-state={f?.state} data-proof={f && !f.premises.length ? "" : undefined}
              transform={`translate(${n.x},${n.y})`} role="button" tabindex="0" aria-label={way(n)} aria-pressed={chosen}
              onclick={() => choose(n)}
              onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); choose(n); } }}>
              <title>{wayTitle(n)}</title>
              <rect class="hit" x="-6" y="-8" width={n.w + 12} height={n.h + 16} />
              {#if f && !f.premises.length}
                <circle cx={cx} cy={n.h / 2} r="6" class="pg-dot proof" /><path d={`M${cx - 2.6} ${n.h / 2} l2 2 l3.4 -3.8`} class="pg-check" />
              {:else}
                <circle cx={cx} cy={n.h / 2} r={chosen ? 4.5 : 3.5} class="pg-dot" />
                {#if (f?.premises.length ?? 0) > 1}<path d={`M${cx - 8} ${n.h / 2 + 3} A9 9 0 0 0 ${cx + 8} ${n.h / 2 + 3}`} class="pg-cup" />{/if}
              {/if}
              {#if n.label}<text class="pg-label" x={cx + 10} y={n.h / 2} dominant-baseline="central">{n.label}</text>{/if}
            </g>
          {/if}
        {/each}
      </svg>
    </div>
  {:else}
    <p class="muted">The root theorem is not converted yet.</p>
  {/if}
</section>
