// The collaboration page (docs/mathdoc.md §4.8): the stages a branch's proof goes through, and
// the proof graph laid out as an AND-OR graph. Pure functions over a board.
import { branchesNeeding, compareFacts, inUse, openGoals, routeCloses, writtenBack, type Board, type Fact } from "./coordination";

export type StageKey = "decompose" | "prove" | "review" | "assemble" | "writeback";
export interface Stage { key: StageKey; label: string; state: "done" | "current" | "todo"; detail: string }

const STAGES: [StageKey, string][] = [
  ["decompose", "Decompose"], ["prove", "Prove"], ["review", "Review"], ["assemble", "Assemble"], ["writeback", "Write back"],
];

/**
 * Where a branch's proof stands: the root split or proved, every goal on the route proved, no
 * decomposition waiting for review, the route assembled and written back. The first stage not
 * done is the current one.
 */
export function stages(board: Board, branch: string): Stage[] {
  const view = board.branches?.[branch];
  const picks = view?.picks ?? {};
  const rootFacts = Object.values(board.facts).filter((f) => f.goal === board.root && f.available && f.state === "active");
  const open = view ? openGoals(board, picks) : [board.root];
  const closes = view ? routeCloses(board, picks) : false;
  const pending = Object.values(board.facts)
    .filter((f) => f.state === "proposed" && branchesNeeding(board, f.goal).includes(branch)).length;
  const last = Object.values(board.runs).filter((r) => r.branch === branch).at(-1);
  const written = last?.status === "certified" && !!writtenBack(board, last);
  const done: Record<StageKey, boolean> = {
    decompose: rootFacts.length > 0, prove: closes, review: pending === 0,
    assemble: last?.status === "certified", writeback: written,
  };
  const detail: Record<StageKey, string> = {
    decompose: !rootFacts.length ? "root not split yet" : rootFacts.some((f) => !f.premises.length) ? "proved directly" : "split",
    prove: closes ? "every goal proved" : `${open.length} open`,
    review: pending ? `${pending} waiting` : "nothing waiting",
    assemble: !last ? "not yet" : last.status,
    writeback: written ? "in the document" : "not yet",
  };
  const current = STAGES.find(([key]) => !done[key])?.[0];
  return STAGES.map(([key, label]) => ({ key, label, detail: detail[key], state: done[key] ? "done" : key === current ? "current" : "todo" }));
}

/** A box of the AND-OR proof graph: a goal card (or: any one certificate proves it), a
 * certificate as a junction under it (and: all its premises), or a bend where a long edge
 * crosses a layer. A junction's box leaves room for its label right of the dot. The document
 * graph (`layoutDocument`) has a `node` card per document node, its fnode as `id` and `goal`. */
export interface GraphNode {
  id: string; kind: "goal" | "fact" | "node" | "bend"; goal: string; fact?: string; label: string;
  /** Left, top and size. */
  x: number; y: number; w: number; h: number; layer: number;
}
/** An edge from the bottom of `from` to the top of `to`: an orthogonal path through `points`; a `back` edge closes a cycle. */
export interface GraphEdge { from: string; to: string; points: [number, number][]; back: boolean }
export interface GraphLayout { nodes: GraphNode[]; edges: GraphEdge[]; width: number; height: number }

/** Goal cards; junctions; the gaps below a goal layer and below a junction layer. */
export const GRAPH = { goalH: 46, goalW: 168, factH: 12, dot: 12, bendW: 6, gapX: 28, gapGoal: 18, gapFact: 30, pad: 20 };
const goalWidth = (label: string) => Math.round(Math.min(260, Math.max(GRAPH.goalW, label.length * 7.4 + 44)));
/** A labelled junction is centred on its dot, so its box is twice the label wide. */
const factWidth = (label: string) => (label ? GRAPH.dot + 2 * Math.round(label.length * 6.4 + 8) : GRAPH.dot);

/** A box while it is laid out: its place in its layer and its neighbours there. */
type Box = GraphNode & { order: number; up: Box[]; down: Box[] };
/** A link from a box to one below it; `back` closes a cycle to a box above. */
interface Link { from: Box; to: Box; back: boolean }
/** A layer's least box height and the least gap below it. */
type Shape = (layer: number) => { h: number; gap: number };

function box(nodes: Map<string, Box>, id: string, kind: Box["kind"], goal: string, text: string, fact?: string): Box {
  const card = kind === "goal" || kind === "node";
  const n: Box = {
    id, kind, goal, fact, label: text, layer: 0, x: 0, y: 0, order: nodes.size, up: [], down: [],
    w: card ? goalWidth(text) : kind === "fact" ? factWidth(text) : GRAPH.bendW,
    h: card ? GRAPH.goalH : kind === "fact" ? GRAPH.factH : 0,
  };
  nodes.set(id, n);
  return n;
}

/** An edge's SVG path: orthogonal runs with rounded corners; a back edge loops out to the right
 * of both ends and into the side of the box above. */
export function edgePath(e: GraphEdge): string {
  const [first, ...rest] = e.points;
  if (e.back) {
    const [x1, y1] = first, [x2, y2] = rest.at(-1)!;
    const out = Math.max(x1, x2) + 40;
    return `M${x1},${y1} C${x1},${y1 + 30} ${out},${y1 + 30} ${out},${(y1 + y2) / 2} S${x2 + 30},${y2} ${x2},${y2}`;
  }
  const pts = e.points;
  let d = `M${first[0]},${first[1]}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const [[ax, ay], [bx, by], [cx, cy]] = [pts[i - 1], pts[i], pts[i + 1]];
    const r = Math.min(6, Math.hypot(bx - ax, by - ay) / 2, Math.hypot(cx - bx, cy - by) / 2);
    const toward = (x: number, y: number, k: number) => [bx + (Math.sign(x - bx) * k), by + (Math.sign(y - by) * k)];
    const [px, py] = toward(ax, ay, r), [qx, qy] = toward(cx, cy, r);
    d += ` L${px},${py} Q${bx},${by} ${qx},${qy}`;
  }
  const [lx, ly] = pts.at(-1)!;
  return `${d} L${lx},${ly}`;
}

/** Pairs (a, b) with a < a' and b > b' for another pair: crossings of edges between two layers. */
function inversions(pairs: [number, number][]): number {
  const bs = [...pairs].sort((p, q) => p[0] - q[0] || p[1] - q[1]).map((p) => p[1]);
  const sort = (xs: number[]): [number[], number] => {
    if (xs.length < 2) return [xs, 0];
    const [l, a] = sort(xs.slice(0, xs.length >> 1)), [r, b] = sort(xs.slice(xs.length >> 1));
    const out: number[] = [];
    let n = a + b, i = 0, j = 0;
    while (i < l.length || j < r.length) {
      if (j >= r.length || (i < l.length && l[i] <= r[j])) out.push(l[i++]);
      else { n += l.length - i; out.push(r[j++]); }
    }
    return [out, n];
  };
  return sort(bs)[1];
}

/** Where edges from different sources cross: a horizontal run of one through a vertical of another. */
export function countCrossings(layout: Pick<GraphLayout, "edges">): number {
  const segs = layout.edges.filter((e) => !e.back).flatMap((e) =>
    e.points.slice(1).map((b, i) => ({ from: e.from, a: e.points[i], b, flat: Math.abs(e.points[i][1] - b[1]) < 0.01 })));
  let n = 0;
  for (const h of segs) {
    if (!h.flat) continue;
    const [x1, x2] = [Math.min(h.a[0], h.b[0]), Math.max(h.a[0], h.b[0])];
    for (const v of segs) {
      if (v.flat || v.from === h.from) continue;
      const [y1, y2] = [Math.min(v.a[1], v.b[1]), Math.max(v.a[1], v.b[1])];
      if (v.a[0] > x1 + 0.5 && v.a[0] < x2 - 0.5 && h.a[1] > y1 + 0.5 && h.a[1] < y2 - 0.5) n++;
    }
  }
  return n;
}

/**
 * The proof graph as an AND-OR graph from the root: each goal once, under it its certificates
 * (`inUse`, or all with `inactive`), under each certificate its premises. A goal shared by
 * certificates is one box with several incoming edges. Layers follow the longest path from the
 * root, so every edge points down; an edge back to a goal above (a cycle) is marked `back`.
 * Within layers, nodes are ordered by barycentres and placed over their neighbours.
 */
export function layoutAndOr(board: Board, label: (goal: string) => string, options: { inactive?: boolean } = {}): GraphLayout | null {
  if (!board.root) return null;
  const byGoal = new Map<string, Fact[]>();
  for (const f of Object.values(board.facts)) {
    if (!options.inactive && !inUse(f)) continue;
    byGoal.set(f.goal, [...(byGoal.get(f.goal) ?? []), f]);
  }
  const nodes = new Map<string, Box>();
  const make = (id: string, kind: Box["kind"], goal: string, text: string, fact?: string) => box(nodes, id, kind, goal, text, fact);
  // Depth-first from the root: the graph, its back edges, and a postorder for layering.
  const links: Link[] = [];
  const onStack = new Set<string>(), post: Box[] = [];
  const visit = (goal: string): Box => {
    const g = make(`g:${goal}`, "goal", goal, label(goal));
    onStack.add(goal);
    for (const f of (byGoal.get(goal) ?? []).sort(compareFacts)) {
      const premises = [...new Set(f.premises)];
      const n = make(`f:${f.id}`, "fact", goal, premises.length > 1 ? `all of ${premises.length}` : "", f.id);
      links.push({ from: g, to: n, back: false });
      for (const p of premises) {
        if (onStack.has(p)) links.push({ from: n, to: nodes.get(`g:${p}`)!, back: true });
        else links.push({ from: n, to: nodes.get(`g:${p}`) ?? visit(p), back: false });
      }
      post.push(n);
    }
    onStack.delete(goal);
    post.push(g);
    return g;
  };
  visit(board.root);
  // Goal layers (even) and junction layers (odd) alternate.
  return arrange(nodes, links, post, (k) => (k % 2 ? { h: GRAPH.factH, gap: GRAPH.gapFact } : { h: GRAPH.goalH, gap: GRAPH.gapGoal }));
}

/**
 * The document graph drawn as the proof graph is: a card per node, each above its dependencies
 * (`edges` run from a node to a dependency, as indices into `nodes`), from the nodes nothing
 * depends on, by label; nodes only a cycle reaches follow. Every layer is a layer of cards.
 * Null when there are more than `maxBoxes` boxes, bends included: ordering costs about their
 * square, some 150 ms at 450.
 */
export function layoutDocument(nodes: { id: string; label: string }[], edges: [number, number][], maxBoxes = Infinity): GraphLayout | null {
  if (!nodes.length) return null;
  const out = nodes.map(() => [] as number[]);
  const referred = nodes.map(() => false);
  const seen = new Set<string>();
  for (const [from, to] of edges) {
    if (from === to || seen.has(`${from}>${to}`)) continue;
    seen.add(`${from}>${to}`);
    out[from].push(to);
    referred[to] = true;
  }
  const boxes = new Map<string, Box>();
  const links: Link[] = [];
  const post: Box[] = [];
  const state = new Map<number, "open" | "done">();
  const visit = (i: number): Box => {
    const n = box(boxes, nodes[i].id, "node", nodes[i].id, nodes[i].label);
    state.set(i, "open");
    for (const d of out[i]) {
      if (state.get(d) === "open") links.push({ from: n, to: boxes.get(nodes[d].id)!, back: true });
      else links.push({ from: n, to: state.has(d) ? boxes.get(nodes[d].id)! : visit(d), back: false });
    }
    state.set(i, "done");
    post.push(n);
    return n;
  };
  const byLabel = nodes.map((_, i) => i)
    .sort((a, b) => nodes[a].label.localeCompare(nodes[b].label) || (nodes[a].id < nodes[b].id ? -1 : 1));
  for (const i of byLabel) if (!referred[i] && !state.has(i)) visit(i);
  for (const i of byLabel) if (!state.has(i)) visit(i);
  return arrange(boxes, links, post, () => ({ h: GRAPH.goalH, gap: GRAPH.gapFact }), maxBoxes);
}

/**
 * Lay out boxes linked depth-first (`post`: their postorder; links to a box on the stack are
 * `back`). Layers follow the longest path from the top, so every edge points down; within
 * layers, nodes are ordered by barycentres and placed over their neighbours; edges are wired
 * orthogonally. Shared by the proof graph and the document graph. Null past `limit` boxes.
 */
function arrange(nodes: Map<string, Box>, links: Link[], post: Box[], shape: Shape, limit = Infinity): GraphLayout | null {
  type N = Box;
  // Longest path from the top: a box sits below every box that links to it.
  for (const n of post.reverse())
    for (const l of links) if (l.from === n && !l.back) l.to.layer = Math.max(l.to.layer, n.layer + 1);
  // Long edges bend through every layer they cross, so that ordering keeps them clear of boxes.
  const chains = links.map((l) => {
    const chain = [l.from];
    if (!l.back) {
      for (let layer = l.from.layer + 1; layer < l.to.layer; layer++) {
        const b = box(nodes, `b:${l.from.id}>${l.to.id}:${layer}`, "bend", l.to.goal, "");
        b.layer = layer;
        b.order = l.from.order + 0.5;
        chain.push(b);
      }
      chain.push(l.to);
      for (let i = 1; i < chain.length; i++) { chain[i - 1].down.push(chain[i]); chain[i].up.push(chain[i - 1]); }
    } else chain.push(l.to);
    return { link: l, chain };
  });
  if (nodes.size > limit) return null;
  const layers: N[][] = [];
  for (const n of nodes.values()) (layers[n.layer] ??= []).push(n);
  for (const row of layers) row.sort((a, b) => a.order - b.order);
  // Order within layers to cut crossings: barycentre sweeps, down then up, each followed by
  // swapping neighbours while that cuts the crossings around their layer; the order with the
  // fewest crossings seen is kept.
  const index = (row: N[]) => row.forEach((n, i) => (n.order = i));
  layers.forEach(index);
  const between = (k: number) => {
    const pairs: [number, number][] = [];
    for (const u of layers[k]) for (const v of u.down) pairs.push([u.order, v.order]);
    return inversions(pairs);
  };
  const around = (k: number) => (k > 0 ? between(k - 1) : 0) + (k + 1 < layers.length ? between(k) : 0);
  const total = () => layers.slice(0, -1).reduce((sum, _, k) => sum + between(k), 0);
  const snapshot = () => layers.map((row) => [...row]);
  // Candidate orders: the one found, the best after each sweep; the drawing decides among them.
  const candidates: { order: N[][]; count: number }[] = [{ order: snapshot(), count: total() }];
  let best = snapshot(), fewest = total();
  for (let sweep = 0; sweep < 12 && fewest > 0; sweep++) {
    const down = sweep % 2 === 0;
    for (let k = 1; k < layers.length; k++) {
      const row = layers[down ? k : layers.length - 1 - k];
      const key = (n: N) => {
        const near = down ? n.up : n.down;
        return near.length ? near.reduce((s, m) => s + m.order, 0) / near.length : n.order;
      };
      const keys = new Map(row.map((n) => [n, key(n)]));
      row.sort((a, b) => keys.get(a)! - keys.get(b)!);
      index(row);
    }
    for (let pass = 0, swapped = true; swapped && pass < 4; pass++) {
      swapped = false;
      layers.forEach((row, k) => {
        if (row.length > 80) return;
        for (let i = 0; i + 1 < row.length; i++) {
          const before = around(k);
          [row[i], row[i + 1]] = [row[i + 1], row[i]];
          index(row);
          if (around(k) < before) swapped = true;
          else { [row[i], row[i + 1]] = [row[i + 1], row[i]]; index(row); }
        }
      });
    }
    const count = total();
    if (count < fewest) { best = snapshot(); fewest = count; candidates.push({ order: best, count }); }
  }
  // Lay out and wire each candidate (the fewest crossings between layers first, at most 4, one
  // for large graphs); keep the drawing whose edges cross least.
  const tried = candidates.sort((a, b) => a.count - b.count).slice(0, nodes.size > 150 ? 1 : 4);
  let drawn: GraphLayout | null = null, crossings = Infinity;
  for (const { order } of tried) {
    order.forEach((row, k) => { layers[k] = [...row]; index(layers[k]); });
    const layout = draw();
    const n = countCrossings(layout);
    if (n < crossings) { drawn = layout; crossings = n; }
    if (!n) break;
  }
  // There is always a candidate: the order found first.
  return drawn!;

  /** Place, wire and measure the nodes in their current order. */
  function draw(): GraphLayout {
  // x: start packed, then pull each node over its neighbours and push overlapping ones apart.
  const centre = (n: N) => n.x + n.w / 2;
  for (const row of layers) {
    let at = 0;
    for (const n of row) { n.x = at; at += n.w + GRAPH.gapX; }
  }
  // The closest placement to the wanted one that keeps the order and the gaps: with x_i = y_i +
  // (widths and gaps before i), the gaps say y never decreases, and the nearest non-decreasing y
  // is the isotonic regression, found by pooling adjacent violators.
  const settle = (row: N[], want: (n: N) => number) => {
    const offset: number[] = [];
    let at = 0;
    for (const n of row) { offset.push(at); at += n.w + GRAPH.gapX; }
    const blocks: { sum: number; size: number }[] = [];
    row.forEach((n, i) => {
      blocks.push({ sum: want(n) - n.w / 2 - offset[i], size: 1 });
      while (blocks.length > 1 && blocks.at(-2)!.sum / blocks.at(-2)!.size > blocks.at(-1)!.sum / blocks.at(-1)!.size) {
        const last = blocks.pop()!;
        blocks.at(-1)!.sum += last.sum;
        blocks.at(-1)!.size += last.size;
      }
    });
    let i = 0;
    for (const b of blocks) for (let k = 0; k < b.size; k++, i++) row[i].x = b.sum / b.size + offset[i];
  };
  for (let sweep = 0; sweep < 12; sweep++) {
    const down = sweep % 2 === 0;
    for (let k = 1; k < layers.length; k++) {
      const row = layers[down ? k : layers.length - 1 - k];
      settle(row, (n) => {
        const near = [...n.up, ...n.down];
        return near.length ? near.reduce((s, m) => s + centre(m), 0) / near.length : centre(n);
      });
    }
    settle(layers[0], (n) => (n.down.length ? n.down.reduce((s, m) => s + centre(m), 0) / n.down.length : centre(n)));
  }
  // A junction sits right under its goal, and a goal a little off its only parent right under
  // it, where the neighbours leave room: chains of single edges run straight, not in small steps.
  for (let k = 1; k < layers.length; k++) {
    const row = layers[k];
    row.forEach((n, i) => {
      if (n.up.length !== 1) return;
      const d = centre(n.up[0]) - centre(n);
      if (Math.abs(d) < 0.5 || (n.kind !== "fact" && Math.abs(d) > 24)) return;
      const lo = i > 0 ? row[i - 1].x + row[i - 1].w + GRAPH.gapX : -Infinity;
      const hi = i + 1 < row.length ? row[i + 1].x - GRAPH.gapX - n.w : Infinity;
      if (n.x + d >= lo && n.x + d <= hi) n.x += d;
    });
  }
  // Edges run orthogonally. Every node's edges leave down one stem onto a bus in the channel
  // below it; a long edge keeps one straight lane down through the layers it crosses when one
  // is free, else jogs in the channels between; edges into a goal land at ports spread along
  // its top, in the order they arrive from. Horizontal runs that share a channel and overlap
  // get tracks of their own, and a channel is as tall as its tracks need.
  const real = (n: N) => n.kind !== "bend";
  // Lanes by layer, with the node whose edges run in them: a source's long edges share a lane
  // (bundled) and part in the channel above each target; other lanes keep 8 apart.
  const lanes = new Map<number, { x: number; source: string }[]>();
  const free = (x: number, layer: number, source: string) =>
    layers[layer].every((n) => !real(n) || x < n.x - 8 || x > n.x + n.w + 8) &&
    (lanes.get(layer) ?? []).every((l) => (l.source === source ? Math.abs(l.x - x) < 0.5 || Math.abs(l.x - x) >= 8 : Math.abs(l.x - x) >= 8));
  const routes = chains.filter((r) => !r.link.back);
  /** Whether the bus below `u` (from its centre to its children's) spans `x`. */
  const span = (u: N, x: number) => {
    const xs = [centre(u), ...u.down.map(centre)];
    return u.down.length > 0 && x > Math.min(...xs) + 1 && x < Math.max(...xs) - 1;
  };
  for (const { chain } of [...routes].sort((a, b) => b.chain.length - a.chain.length)) {
    const bends = chain.slice(1, -1);
    if (!bends.length) continue;
    const source = chain[0].id;
    const xs = bends.map(centre).sort((a, b) => a - b);
    const shared = new Set([...lanes.values()].flat().filter((l) => l.source === source).map((l) => l.x));
    // The free lane with the least sideways run from the source and into the target; sharing a
    // lane with the source's other edges counts as 40 less.
    const [sx, tx] = [centre(chain[0]), centre(chain.at(-1)!)];
    // A lane down through a channel (below each bend but the first, and above the target)
    // crosses every other source's bus spanning its x there.
    const channels = [...bends.slice(1).map((b) => b.layer - 1), chain.at(-1)!.layer - 1];
    const crossed = (x: number) =>
      channels.reduce((n, c) => n + layers[c].filter((u) => u.id !== source && real(u) && span(u, x)).length, 0);
    const cost = (x: number) => Math.abs(x - sx) + Math.abs(x - tx) - (shared.has(x) ? 40 : 0) + 60 * crossed(x);
    const lane = [...shared, tx, sx, ...xs, xs[xs.length >> 1]]
      .filter((x) => bends.every((b) => free(x, b.layer, source)))
      .sort((a, b) => cost(a) - cost(b))[0];
    for (const b of bends) {
      if (lane !== undefined) b.x = lane - b.w / 2;
      (lanes.get(b.layer) ?? lanes.set(b.layer, []).get(b.layer)!).push({ x: centre(b), source });
    }
  }
  const port = new Map<(typeof routes)[number], number>();
  for (const target of new Set(routes.map((r) => r.chain.at(-1)!))) {
    const into = routes.filter((r) => r.chain.at(-1) === target);
    const from = (r: (typeof into)[number]) => centre(r.chain.at(-2)!);
    into.sort((a, b) => from(a) - from(b));
    const step = into.length > 1 ? Math.min(16, (target.w * 0.6) / (into.length - 1)) : 0;
    into.forEach((r, i) => port.set(r, centre(target) + (i - (into.length - 1) / 2) * step));
  }
  // The x where an edge leaves each layer it is at: its source, its bends, then its port.
  const xsOf = (r: (typeof routes)[number]) => [...r.chain.slice(0, -1).map(centre), port.get(r)!];
  // A horizontal run in a channel, with the x where it meets a vertical from above and below.
  type Run = { channel: number; owner: string; from: number; to: number; up: number[]; down: number[]; track?: number };
  const runs: Run[] = [];
  const run = (channel: number, owner: string, a: number, b: number) => {
    if (Math.abs(a - b) < 0.5) return;
    const old = runs.find((r) => r.channel === channel && r.owner === owner);
    if (old) { old.from = Math.min(old.from, a, b); old.to = Math.max(old.to, a, b); old.down.push(b); }
    else runs.push({ channel, owner, from: Math.min(a, b), to: Math.max(a, b), up: [a], down: [b] });
  };
  for (const r of routes) {
    const xs = xsOf(r);
    run(r.chain[0].layer, `bus ${r.chain[0].id}`, xs[0], xs[1]);
    for (let i = 1; i + 1 < xs.length; i++) run(r.chain[i].layer, `jog ${r.link.from.id}>${r.link.to.id} ${i}`, xs[i], xs[i + 1]);
  }
  // Tracks: of two overlapping runs, the one above is crossed by the other's verticals from
  // above inside its span, and crosses with its own verticals down inside the other's; runs
  // are stacked in the order that costs fewer crossings, and runs apart share a track.
  const inside = (xs: number[], r: Run) => xs.filter((x) => x > r.from + 0.5 && x < r.to - 0.5).length;
  const above = (r: Run, t: Run) => inside(t.up, r) + inside(r.down, t);
  const tracks = new Map<number, number>();
  for (const channel of new Set(runs.map((r) => r.channel))) {
    const here = runs.filter((r) => r.channel === channel).sort((a, b) => a.from - b.from);
    const order: Run[] = [];
    for (const r of here) {
      let at = order.length;
      while (at > 0 && above(r, order[at - 1]) < above(order[at - 1], r)) at--;
      order.splice(at, 0, r);
    }
    let count = 0;
    order.forEach((r, i) => {
      const overlapping = order.slice(0, i).filter((t) => t.from < r.to + 10 && r.from < t.to + 10);
      r.track = overlapping.length ? Math.max(...overlapping.map((t) => t.track!)) + 1 : 0;
      count = Math.max(count, r.track + 1);
    });
    tracks.set(channel, count);
  }
  const TRACK = 7;
  // y: layers stacked, each as tall as its tallest box and at least its shape; boxes centred in
  // their layer; channels as tall as their tracks need.
  let top = GRAPH.pad;
  const channelTop: number[] = [], channelH: number[] = [];
  layers.forEach((row, k) => {
    const h = Math.max(shape(k).h, ...row.map((n) => n.h));
    for (const n of row) n.y = top + (h - n.h) / 2;
    channelTop[k] = top + h;
    channelH[k] = Math.max(shape(k).gap, 12 + (tracks.get(k) ?? 1) * TRACK);
    top += h + channelH[k];
  });
  const trackY = (channel: number, owner: string) => {
    const r = runs.find((r) => r.channel === channel && r.owner === owner);
    const n = tracks.get(channel) ?? 1;
    return channelTop[channel] + channelH[channel] / 2 + ((r?.track ?? 0) - (n - 1) / 2) * TRACK;
  };
  const left = Math.min(...[...nodes.values()].map((n) => n.x));
  const shift = GRAPH.pad - left;
  for (const n of nodes.values()) n.x += shift;
  for (const [r, x] of port) port.set(r, x + shift);
  const width = Math.max(...[...nodes.values()].map((n) => n.x + n.w)) + GRAPH.pad;
  const height = top - channelH[layers.length - 1] + GRAPH.pad;
  const edges: GraphEdge[] = chains.map((r) => {
    const { link, chain } = r;
    const from = chain[0], to = chain.at(-1)!;
    if (link.back) return { from: from.id, to: to.id, points: [[centre(from), from.y + from.h], [to.x + to.w, to.y + to.h / 2]], back: true };
    const xs = xsOf(r);
    const points: [number, number][] = [[xs[0], from.y + from.h]];
    for (let i = 0; i + 1 < xs.length; i++) {
      const y = trackY(chain[i].layer, i ? `jog ${link.from.id}>${link.to.id} ${i}` : `bus ${from.id}`);
      points.push([xs[i], y], [xs[i + 1], y]);
    }
    points.push([xs.at(-1)!, to.y]);
    // Drop the corners of runs with no length.
    const kept = points.filter((p, i) => i === 0 || i === points.length - 1 || !(p[0] === points[i - 1][0] && p[1] === points[i - 1][1]));
    const lean = kept.filter((p, i) => i === 0 || i === kept.length - 1 ||
      !((kept[i - 1][0] === p[0] && p[0] === kept[i + 1][0]) || (kept[i - 1][1] === p[1] && p[1] === kept[i + 1][1])));
    return { from: from.id, to: to.id, points: lean, back: false };
  });
  const out = [...nodes.values()].map(({ up: _u, down: _d, order: _o, ...n }) => ({ ...n }));
  return { nodes: out, edges, width, height };
  }
}
