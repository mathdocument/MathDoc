import { describe, expect, it } from 'vitest';
import { GRAPH, countCrossings, layoutAndOr, layoutDocument, stages, type GraphLayout } from './collab-layout';
import { type Board, type Fact, type Run, type Writeback } from './coordination';

const fact = (id: string, goal: string, premises: string[] = [], state: Fact['state'] = 'active'): Fact =>
  ({ id, goal, premises, trust: 'trusted', state, available: true });
const request = { database: 'd', branch: 'main', node: 'n', environment_id: 'e', bound: true, stale: false };
const board = (facts: Fact[], extra: Partial<Board> = {}): Board => ({
  id: 'p', title: 'T', owner: 'alice', members: ['alice'], revision: 1, root: 'T', minimum_trust: 'audited',
  notes: '', budget: 0, spent: 0, ready: true, tasks: {}, attempts: {}, runs: {},
  goals: Object.fromEntries(['T', 'A', 'B'].map((k) => [k, { key: k, statement: k, status: 'insufficient' as const }])),
  facts: Object.fromEntries(facts.map((f) => [f.id, f])),
  project: { database: 'd', environment_id: 'e', root_node: 'n' },
  branches: { main: { request, nodes: {}, picks: {}, writebacks: {} } }, ...extra,
});
const states = (b: Board) => Object.fromEntries(stages(b, 'main').map((s) => [s.key, `${s.state}: ${s.detail}`]));

describe('the stages of a branch', () => {
  it('follows a proof from the bare root to the document', () => {
    const b = board([]);
    expect(states(b)).toEqual({
      decompose: 'current: root not split yet', prove: 'todo: 1 open', review: 'done: nothing waiting',
      assemble: 'todo: not yet', writeback: 'todo: not yet',
    });
    b.facts.t = fact('t', 'T', ['A']);
    b.facts.p = fact('p', 'A', ['B'], 'proposed');
    expect(states(b)).toMatchObject({ decompose: 'done: split', prove: 'current: 1 open', review: 'todo: 1 waiting' });
    b.facts.a = fact('a', 'A');
    expect(states(b)).toMatchObject({ prove: 'done: every goal proved', review: 'current: 1 waiting' });
    delete b.facts.p;
    const run: Run = { id: 'r', component: 'lean-worker', status: 'certified', branch: 'main', route: { root: 'T', steps: [] } };
    b.runs = { r: run };
    expect(states(b)).toMatchObject({ assemble: 'done: certified', writeback: 'current: not yet' });
    const w: Writeback = { batch_id: 'w', run: 'r', status: 'committed', created: [], updated: [] };
    b.branches!.main.writebacks = { w };
    expect(stages(b, 'main').every((s) => s.state === 'done')).toBe(true);
  });
});

describe('the AND-OR proof graph', () => {
  const label = (g: string) => g;
  const goals = (...keys: string[]) => Object.fromEntries(keys.map((k) => [k, { key: k, statement: k, status: 'insufficient' as const }]));
  const ids = (l: GraphLayout, kind: string) => l.nodes.filter((n) => n.kind === kind).map((n) => n.id).sort();
  const node = (l: GraphLayout, id: string) => l.nodes.find((n) => n.id === id)!;
  it('draws each goal once, its certificates as alternatives and their premises below', () => {
    // T has two decompositions; both need B, which is drawn once with two incoming edges.
    const b = board([fact('t1', 'T', ['A', 'B']), fact('t2', 'T', ['B', 'C']), fact('b', 'B')], { goals: goals('T', 'A', 'B', 'C') });
    const l = layoutAndOr(b, label)!;
    expect(ids(l, 'goal')).toEqual(['g:A', 'g:B', 'g:C', 'g:T']);
    expect(ids(l, 'fact')).toEqual(['f:b', 'f:t1', 'f:t2']);
    expect(l.edges.filter((e) => e.to === 'g:B').map((e) => e.from).sort()).toEqual(['f:t1', 'f:t2']);
    // A junction is labelled with its premises when it has several.
    expect(node(l, 'f:t1').label).toBe('all of 2');
    expect(node(l, 'f:b').label).toBe('');
    // Goals and certificates alternate, every edge points down, and boxes in a layer keep apart.
    expect([node(l, 'g:T').layer, node(l, 'f:t1').layer, node(l, 'g:A').layer, node(l, 'f:b').layer]).toEqual([0, 1, 2, 3]);
    for (const e of l.edges) expect(e.points.at(-1)![1]).toBeGreaterThan(e.points[0][1]);
    const rows = Object.groupBy(l.nodes, (n) => n.layer);
    for (const row of Object.values(rows)) {
      const sorted = [...row!].sort((x, y) => x.x - y.x);
      for (let i = 1; i < sorted.length; i++) expect(sorted[i - 1].x + sorted[i - 1].w + GRAPH.gapX).toBeLessThanOrEqual(sorted[i].x + 0.5);
    }
    // Junction layers are thin: a goal layer, then a junction layer just below it.
    expect(node(l, 'f:t1').y - (node(l, 'g:T').y + GRAPH.goalH)).toBeGreaterThanOrEqual(GRAPH.gapGoal);
    expect(node(l, 'f:t1').y - (node(l, 'g:T').y + GRAPH.goalH)).toBeLessThan(GRAPH.gapFact);
    expect(node(l, 'g:A').y - (node(l, 'f:t1').y + GRAPH.factH)).toBeGreaterThanOrEqual(GRAPH.gapFact);
    expect(Math.min(...l.nodes.map((n) => n.x))).toBeCloseTo(GRAPH.pad);
    expect(Math.max(...l.nodes.map((n) => n.x + n.w))).toBeCloseTo(l.width - GRAPH.pad);
  });
  it('puts a shared goal below every certificate that needs it, bending long edges', () => {
    // T ⇐ A, X and A ⇐ X: X sits under A's decomposition; the edge from T's passes a bend.
    const b = board([fact('t', 'T', ['A', 'X']), fact('a', 'A', ['X'])], { goals: goals('T', 'A', 'X') });
    const l = layoutAndOr(b, label)!;
    expect(node(l, 'g:X').layer).toBe(4);
    const long = l.edges.find((e) => e.from === 'f:t' && e.to === 'g:X')!;
    expect(l.nodes.filter((n) => n.kind === 'bend')).toHaveLength(2);
    // A chain of single edges (A, its junction) runs straight: no sideways run between them.
    const toA = l.edges.find((e) => e.from === 'f:t' && e.to === 'g:A')!;
    const fromA = l.edges.find((e) => e.from === 'g:A')!;
    expect(fromA.points).toHaveLength(2);
    expect(toA.points.at(-1)![0]).toBeCloseTo(fromA.points[0][0]);
    // It keeps one straight lane down past A and A's junction.
    const a = node(l, 'g:A');
    expect(long.points.some(([x, y], i) => i > 0 && long.points[i - 1][0] === x && long.points[i - 1][1] <= a.y && y >= a.y + a.h)).toBe(true);
  });
  it('wires edges orthogonally: a shared bus out of each node, lanes clear of cards, ports apart', () => {
    // T ⇐ A, B, C (one decomposition); A ⇐ C and B ⇐ C, so C has three edges in.
    const b = board([fact('t', 'T', ['A', 'B', 'C']), fact('a', 'A', ['C']), fact('b', 'B', ['C']), fact('c', 'C')], { goals: goals('T', 'A', 'B', 'C') });
    const l = layoutAndOr(b, label)!;
    const cards = l.nodes.filter((n) => n.kind !== 'bend');
    for (const e of l.edges) {
      for (let i = 1; i < e.points.length; i++) {
        const [[x0, y0], [x1, y1]] = [e.points[i - 1], e.points[i]];
        expect(x0 === x1 || y0 === y1).toBe(true);
        // No vertical run passes through a card other than at its own ends.
        if (x0 === x1) for (const n of cards) {
          if (n.id === e.from || n.id === e.to) continue;
          const inside = x0 > n.x && x0 < n.x + n.w && Math.min(y0, y1) < n.y + n.h && Math.max(y0, y1) > n.y;
          expect(inside, `${e.from}>${e.to} through ${n.id}`).toBe(false);
        }
      }
    }
    // The decomposition's three edges share one bus just below it.
    const out = l.edges.filter((e) => e.from === 'f:t');
    expect(new Set(out.map((e) => e.points[1][1])).size).toBe(1);
    // C's three edges land at three ports along its top.
    const into = l.edges.filter((e) => e.to === 'g:C').map((e) => e.points.at(-1)![0]);
    expect(new Set(into).size).toBe(3);
  });
  it('counts crossings of edges from different sources, and draws planar graphs without any', () => {
    const edge = (from: string, points: [number, number][]) => ({ from, to: 'x', points, back: false });
    // A horizontal run of one edge through a vertical of another; one from the same source does not count.
    expect(countCrossings({ edges: [edge('a', [[0, 10], [100, 10]]), edge('b', [[50, 0], [50, 20]])] })).toBe(1);
    expect(countCrossings({ edges: [edge('a', [[0, 10], [100, 10]]), edge('a', [[50, 0], [50, 20]])] })).toBe(0);
    // Two decompositions whose premises interleave in the order they are found (P, Q under t;
    // P needs Y and X, Q needs X): ordering and routing leave nothing crossing.
    const b = board([fact('t', 'T', ['P', 'Q']), fact('p', 'P', ['Y', 'X']), fact('q', 'Q', ['X']), fact('y', 'Y'), fact('x', 'X')],
      { goals: goals('T', 'P', 'Q', 'X', 'Y') });
    expect(countCrossings(layoutAndOr(b, label)!)).toBe(0);
    const shared = board([fact('t', 'T', ['A', 'B', 'C']), fact('a', 'A', ['C']), fact('b', 'B', ['C']), fact('c', 'C')], { goals: goals('T', 'A', 'B', 'C') });
    expect(countCrossings(layoutAndOr(shared, label)!)).toBe(0);
  });
  it('orders layers, lanes and tracks to cut crossings', () => {
    // Two graphs from a random search (300 graphs, seed 7) where the order found first crosses:
    // without the sweeps they cross 4 and 21 times, without the track order 2 and 3.
    const of = (facts: [string, string, string[]][]) => board(facts.map(([id, goal, premises]) => fact(id, goal, premises)),
      { goals: goals('T', ...new Set(facts.flatMap(([, g, ps]) => [g, ...ps]))) });
    const first = of([['f0_0', 'T', ['G0', 'G1']], ['f1_0', 'G0', ['G1', 'G3', 'G4']], ['f2_0', 'G1', ['G3', 'G4']], ['f2_1', 'G1', ['G2']],
      ['f3_0', 'G2', ['G4']], ['f4_0', 'G3', []], ['f4_1', 'G3', []], ['f5_0', 'G4', ['G5']], ['f6_0', 'G5', []]]);
    expect(countCrossings(layoutAndOr(first, label)!)).toBe(0);
    const second = of([['f0_0', 'T', ['G1', 'G5']], ['f0_1', 'T', ['G2', 'G4', 'G5']], ['f2_0', 'G1', ['G3', 'G5']], ['f3_0', 'G2', ['G3', 'G4']],
      ['f3_1', 'G2', ['G3']], ['f4_0', 'G3', ['G4']], ['f4_1', 'G3', ['G5']], ['f5_0', 'G4', []], ['f6_0', 'G5', []]]);
    expect(countCrossings(layoutAndOr(second, label)!)).toBeLessThanOrEqual(3);
  });
  it('marks an edge back to a goal above as a cycle, and leaves certificates not in use out unless asked', () => {
    const b = board([fact('t', 'T', ['A']), fact('a', 'A', ['T']), fact('r', 'A', ['B'], 'retired')], { goals: goals('T', 'A', 'B') });
    const l = layoutAndOr(b, label)!;
    expect(l.edges.filter((e) => e.back).map((e) => `${e.from}>${e.to}`)).toEqual(['f:a>g:T']);
    expect(ids(l, 'fact')).toEqual(['f:a', 'f:t']);
    expect(ids(layoutAndOr(b, label, { inactive: true })!, 'fact')).toEqual(['f:a', 'f:r', 'f:t']);
  });
});

describe('the document graph', () => {
  // Edges run from a node to a dependency, as /api/graph/full sends them.
  const doc = (labels: string[], edges: [number, number][]) => layoutDocument(labels.map((l) => ({ id: l, label: l })), edges);
  const at = (l: GraphLayout, id: string) => l.nodes.find((n) => n.id === id)!;
  it('puts each node above its dependencies, from the nodes nothing depends on, by label', () => {
    // root ⇒ lemma, def; lemma ⇒ def; other stands alone.
    const l = doc(['def', 'lemma', 'root', 'other'], [[2, 1], [2, 0], [1, 0]])!;
    expect(l.nodes.filter((n) => n.kind === 'node').map((n) => [n.id, n.layer])).toEqual(
      expect.arrayContaining([['root', 0], ['other', 0], ['lemma', 1], ['def', 2]]));
    expect(at(l, 'def').y).toBeGreaterThan(at(l, 'lemma').y);
    expect(at(l, 'root').h).toBe(GRAPH.goalH);
    // The long edge root ⇒ def bends through the lemma's layer; nothing crosses.
    expect(l.nodes.filter((n) => n.kind === 'bend')).toHaveLength(1);
    expect(countCrossings(l)).toBe(0);
  });
  it('lays out a cycle with an edge back, and duplicate or self edges once', () => {
    const l = doc(['a', 'b'], [[0, 1], [1, 0], [0, 1], [0, 0]])!;
    expect(l.edges.map((e) => `${e.from}>${e.to}${e.back ? ' back' : ''}`).sort()).toEqual(['a>b', 'b>a back']);
  });
  it('gives up past the box limit, bends included', () => {
    // A chain of five, the first three also needing the last: 3 + 2 + 1 bends and 5 cards.
    const edges: [number, number][] = [[0, 1], [1, 2], [2, 3], [3, 4], [0, 4], [1, 4], [2, 4]];
    const labels = ['n0', 'n1', 'n2', 'n3', 'n4'];
    expect(layoutDocument(labels.map((l) => ({ id: l, label: l })), edges, 10)).toBeNull();
    expect(layoutDocument(labels.map((l) => ({ id: l, label: l })), edges, 11)).not.toBeNull();
    expect(doc([], [])).toBeNull();
  });
});
