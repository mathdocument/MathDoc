import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import { activeGoals, activeRoute, coordination, branchRoute, branchView, branchesNeeding, goalStatement, goalStatementBlock, nextStep, openGoals, proofTree, readOnlyReason, refusal, routeCloses, solvedGoals, spentBy, timeLeft, writtenBack, type Board, type Fact, type Run, type Writeback } from './coordination';

const fact = (id: string, goal: string, premises: string[] = [], state: Fact['state'] = 'active'): Fact =>
  ({ id, goal, premises, trust: 'trusted', state, available: true });
const board = (facts: Fact[], extra: Partial<Board> = {}): Board => ({
  id: 'p', title: 'T', owner: 'alice', members: ['alice'], revision: 1, root: 'T', minimum_trust: 'audited',
  notes: '', budget: 0, spent: 0, ready: true, goals: {}, tasks: {}, attempts: {}, runs: {},
  facts: Object.fromEntries(facts.map((f) => [f.id, f])), ...extra,
});

describe('project boards in the browser', () => {
  it('closes a route only through the branch picks and active facts', () => {
    const b = board([fact('t1', 'T', ['A']), fact('t2', 'T', ['B']), fact('b', 'B')]);
    expect(routeCloses(b, {})).toBe(true);
    expect(routeCloses(b, { T: 't1' }), 'A is not proved').toBe(false);
    expect(routeCloses(b, { T: 't2' })).toBe(true);
    b.facts.b.state = 'paused';
    expect(routeCloses(b, { T: 't2' })).toBe(false);
  });
  it('shows a branch view and explains read-only boards', () => {
    const request = { database: 'd', branch: 'main', node: 'n', environment_id: 'e', bound: true, stale: false };
    const project = board([], {
      project: { database: 'd', environment_id: 'e', root_node: 'n' },
      branches: { main: { request, nodes: {}, picks: { T: 't' }, writebacks: {} } },
    });
    expect(branchView(project, 'main')?.picks).toEqual({ T: 't' });
    expect(branchView(project, 'alt')).toBeNull();
    expect(readOnlyReason(project)).toBeNull();
    expect(readOnlyReason({ ...project, project: { ...project.project!, superseded: true } })).toMatch(/environment changed/);
    const legacy = board([], { request, nodes: {} });
    expect(branchView(legacy, 'other')?.request.branch).toBe('main');
    expect(readOnlyReason(legacy)).toMatch(/read-only/);
  });
  it('marks the certificates an assembly of the branch would use, as the coordinator picks them', () => {
    // T has a complete proof and a decomposition through A; A has two complete proofs. Listed in
    // the order the choice must not follow.
    const b = board([fact('a2', 'A'), fact('a1', 'A'), fact('t-split', 'T', ['A']), fact('t-full', 'T')]);
    const route = (picks: Record<string, string>) => [...(branchRoute(b, picks) ?? [])].sort();
    // Unpicked goals take the fewest premises first, then the smallest ID.
    expect(route({})).toEqual(['t-full']);
    expect(route({ A: 'a2' }), 'a pick below an unused goal changes nothing').toEqual(['t-full']);
    expect(route({ T: 't-split' })).toEqual(['a1', 't-split']);
    expect(route({ T: 't-split', A: 'a2' })).toEqual(['a2', 't-split']);
    // A pick of a certificate that is not active closes no route.
    b.facts.a2.state = 'paused';
    expect(branchRoute(b, { T: 't-split', A: 'a2' })).toBeNull();
    expect(routeCloses(b, { T: 't-split', A: 'a2' })).toBe(false);
  });
  it('tells which runs are already written back to their branch', () => {
    const request = { database: 'd', branch: 'alt', node: 'n', environment_id: 'e', bound: true, stale: false };
    const run = (id: string): Run => ({ id, component: 'lean-worker', status: 'certified', branch: 'alt', route: { root: 't', steps: [] } });
    const batch = (id: string, r: string, status: Writeback['status']): Writeback => ({ batch_id: id, run: r, status, created: [], updated: [] });
    const b = board([], {
      project: { database: 'd', environment_id: 'e', root_node: 'n' },
      runs: { r1: run('r1'), r2: run('r2'), r3: run('r3') },
      branches: { alt: { request, nodes: {}, picks: {}, writebacks: { w1: batch('w1', 'r1', 'committed'), w2: batch('w2', 'r2', 'aborted') } } },
    });
    expect(writtenBack(b, b.runs.r1)?.batch_id).toBe('w1');
    // An aborted batch wrote nothing: the run can be accepted again.
    expect(writtenBack(b, b.runs.r2)).toBeNull();
    expect(writtenBack(b, b.runs.r3)).toBeNull();
    // A run of a branch the reader cannot see.
    expect(writtenBack(b, { ...run('r1'), branch: 'hidden' })).toBeNull();
  });
  it('keeps the premises of a proposed decomposition out of scheduling until it is active', () => {
    const b = board([fact('t', 'T', ['A']), fact('p', 'A', ['B'], 'proposed')]);
    expect([...activeGoals(b)].sort()).toEqual(['A', 'T']);
    b.facts.p.state = 'active';
    expect([...activeGoals(b)].sort()).toEqual(['A', 'B', 'T']);
    // A registered negation of an active goal can be worked on, until its goal is refuted.
    b.negations = { N: 'B' };
    expect(activeGoals(b).has('N')).toBe(true);
    b.goals.B = { key: 'B', statement: 'b', status: 'insufficient', refuted: { certificate: 'n' } };
    expect(activeGoals(b).has('N')).toBe(false);
  });
  it('names the branches whose route can need a goal, following picks', () => {
    const request = { database: 'd', branch: 'main', node: 'n', environment_id: 'e', bound: true, stale: false };
    const state = (picks: Record<string, string>) => ({ request, nodes: {}, picks, writebacks: {} });
    const b = board([fact('t1', 'T', ['A']), fact('t2', 'T', ['B'])], {
      project: { database: 'd', environment_id: 'e', root_node: 'n' },
      branches: { main: state({ T: 't1' }), alt: state({ T: 't2' }), any: state({}) },
    });
    expect(branchesNeeding(b, 'A')).toEqual(['main', 'any']);
    expect(branchesNeeding(b, 'B')).toEqual(['alt', 'any']);
    expect(branchesNeeding(b, 'C')).toEqual([]);
  });
  it('states a goal by its declaration, proposition or name', () => {
    const b = board([], {
      goals: {
        A: { key: 'A', statement: 'a', status: 'insufficient', declaration: { name: 'a', binders: '(n : Nat)', type: 'n = n' } },
        B: { key: 'B', statement: 'b', status: 'insufficient', proposition: '∀ (n : Nat), n = n' },
        C: { key: 'C', statement: 'c', status: 'insufficient' },
      },
    });
    expect(goalStatement(b, 'A')).toBe('a (n : Nat) : n = n');
    expect(goalStatement(b, 'B')).toBe('∀ (n : Nat), n = n');
    expect(goalStatement(b, 'C')).toBe('c');
    // Laid out for reading, the type goes under the name and binders.
    expect(goalStatementBlock(b, 'A')).toBe('a (n : Nat) :\n  n = n');
    expect(goalStatementBlock(b, 'B')).toBe('∀ (n : Nat), n = n');
  });
  it('adds up the budget by agent identity, closed attempts as spent and open ones as reserved', () => {
    const attempt = (id: string, actor: string, spent: number, reserved: number, outcome?: string) =>
      ({ id, task: 't', actor, epoch: 1, expires: 0, spent, reserved, ...(outcome ? { outcome } : {}) });
    const b = board([], {
      attempts: {
        a: attempt('a', 'bob/agent-a', 30, 50, 'submitted'),
        b: attempt('b', 'bob/agent-a', 0, 20),
        c: attempt('c', 'bob/agent-b', 40, 40, 'expired'),
      },
    });
    expect(spentBy(b)).toEqual([
      { actor: 'bob/agent-b', spent: 40, reserved: 0, attempts: 1 },
      { actor: 'bob/agent-a', spent: 30, reserved: 20, attempts: 2 },
    ]);
  });
});

describe('the proof tree', () => {
  const shape = (g: ReturnType<typeof proofTree>['root']): unknown =>
    g && (g.repeat ? `${g.goal}*` : { [g.goal]: g.facts.map((t) => ({ [t.fact.id]: t.premises.map(shape) })) });
  it('lists complete proofs first and expands a shared subgoal once', () => {
    // T ⇐ A, B and T ⇐ A; A proved; B split into A again; a proposed split of T comes after the active ones.
    const b = board([fact('t2', 'T', ['A']), fact('t1', 'T', ['A', 'B']), fact('a', 'A'), fact('b', 'B', ['A']), fact('tp', 'T', ['C'], 'proposed')]);
    expect(shape(proofTree(b).root)).toEqual({ T: [
      { t2: [{ A: [{ a: [] }] }] },
      { t1: ['A*', { B: [{ b: ['A*'] }] }] },
      { tp: [{ C: [] }] },
    ] });
  });
  it('stops at cycles and lists goals the root does not reach', () => {
    const b = board([fact('t', 'T', ['A']), fact('a', 'A', ['T']), fact('n', 'N'), fact('m', 'M', ['N'])]);
    const tree = proofTree(b);
    expect(shape(tree.root)).toEqual({ T: [{ t: [{ A: [{ a: ['T*'] }] }] }] });
    // M reaches N, so N is shown under M and not again on its own.
    expect(tree.others.map(shape)).toEqual([{ M: [{ m: [{ N: [{ n: [] }] }] }] }]);
  });
  it('expands a goal shared with a certificate not in use where it is used', () => {
    // T ⇐ A, B; A has a complete proof and a retired split into X, reached first; B ⇐ X is active.
    const b = board([fact('t', 'T', ['A', 'B']), fact('a1', 'A'), fact('a2', 'A', ['X'], 'retired'), fact('b', 'B', ['X']), fact('x', 'X')]);
    expect(shape(proofTree(b).root)).toEqual({ T: [{ t: [
      { A: [{ a1: [] }, { a2: ['X*'] }] },
      { B: [{ b: [{ X: [{ x: [] }] }] }] },
    ] }] });
    // Reached only through a paused certificate, a goal is still expanded there.
    b.facts.b.state = 'paused';
    expect(shape(proofTree(b).root)).toEqual({ T: [{ t: [
      { A: [{ a1: [] }, { a2: [{ X: [{ x: [] }] }] }] },
      { B: [{ b: ['X*'] }] },
    ] }] });
  });
});

describe('the active route through the AND-OR graph', () => {
  // T can be split two ways: t1 ⇐ A, B (A proved, B open) or t2 ⇐ C (C proved by c).
  const b = board([fact('t1', 'T', ['A', 'B']), fact('t2', 'T', ['C']), fact('a', 'A'), fact('c', 'C'), fact('b2', 'B', ['D'])]);
  it('takes a provable way when nothing is chosen, as an assembly would', () => {
    const r = activeRoute(b, {});
    expect([r.choice.get('T'), r.closes, r.open]).toEqual(['t2', true, []]);
    expect([...r.goals].sort()).toEqual(['C', 'T']);
  });
  it('follows a pick, and a draft over the pick, down to what is still open', () => {
    const picked = activeRoute(b, { T: 't1' });
    expect([...picked.goals].sort()).toEqual(['A', 'B', 'D', 'T']);
    expect([...picked.facts].sort()).toEqual(['a', 'b2', 't1']);
    // B is open through b2 ⇐ D, and D has no certificate at all.
    expect([picked.closes, picked.open]).toEqual([false, ['T', 'B', 'D']]);
    const drafted = activeRoute(b, { T: 't1' }, { T: 't2' });
    expect([drafted.choice.get('T'), drafted.closes]).toEqual(['t2', true]);
  });
  it('chooses only active certificates', () => {
    const paused = board([fact('t1', 'T', ['A'], 'paused'), fact('t2', 'T', ['B'])]);
    expect(activeRoute(paused, { T: 't1' }).choice.get('T')).toBe('t2');
    expect(activeRoute(paused, {}, { T: 't1' }).choice.get('T')).toBe('t2');
  });
});

describe('refused commands', () => {
  it('say when the server is older than the page', () => {
    const old = new ApiError('invalid_request', 400, { reason: 'invalid_request', issues: [{ path: ['type'], message: 'Invalid discriminator value' }] });
    expect(refusal(old, { type: 'delete_note', goal: 'G', note: { actor: 'a', at: 1 } })).toMatch(/older version of MathDoc that does not know “delete_note”/);
    // Other refusals keep the server's reason.
    expect(refusal(new ApiError('note_author_required', 403, { reason: 'note_author_required' }), { type: 'sync' })).toBe('note_author_required');
    expect(refusal(new ApiError('invalid_request', 400, { reason: 'invalid_request', issues: [{ path: ['text'] }] }), { type: 'sync' })).toBe('invalid_request');
  });
});

describe('claims', () => {
  it('reads the time left in seconds only in the last minute', () => {
    const at = (s: number) => timeLeft(1_000_000 + s * 1000, 1_000_000);
    expect([at(-5), at(42), at(1848), at(3600), at(5430)]).toEqual(['0 s left', '42 s left', '31 min left', '1 h left', '1 h 31 min left']);
  });
});

describe('the next step on a branch', () => {
  const request = { database: 'd', branch: 'main', node: 'n', environment_id: 'e', bound: true, stale: false };
  const goal = (key: string, status: Board['goals'][string]['status'] = 'insufficient') => ({ key, statement: key, status });
  const project = (facts: Fact[], extra: Partial<Board> = {}) => board(facts, {
    project: { database: 'd', environment_id: 'e', root_node: 'n' },
    goals: { T: goal('T'), A: goal('A'), B: goal('B') },
    branches: { main: { request, nodes: {}, picks: {}, writebacks: {} } },
    ...extra,
  });
  it('lists the unproved goals nobody has split yet, following picks', () => {
    const b = project([fact('t1', 'T', ['A']), fact('t2', 'T', ['B']), fact('a', 'A', ['B'])]);
    // T and A are split; B is the goal to work on.
    expect(openGoals(b, {})).toEqual(['B']);
    expect(openGoals(b, { T: 't2' })).toEqual(['B']);
    b.facts.b = fact('b', 'B');
    expect(openGoals(b, {})).toEqual([]);
    expect([...solvedGoals(b)].sort()).toEqual(['A', 'B', 'T']);
    // Only active certificates prove: a paused proof of B leaves B open again.
    b.facts.b.state = 'paused';
    expect(openGoals(b, {})).toEqual(['B']);
    // Below a goal proved some other way nothing is needed; a pick follows only its certificate.
    b.facts.c = fact('c', 'T');
    expect(openGoals(b, {})).toEqual([]);
    expect(openGoals(b, { T: 't1' })).toEqual(['B']);
    // A goal with no certificate at all is its own open goal.
    expect(openGoals(project([]), {})).toEqual(['T']);
    // A cycle: every unproved goal has a certificate, so all of them are listed.
    expect(openGoals(project([fact('t', 'T', ['A']), fact('a', 'A', ['T'])]), {}).sort()).toEqual(['A', 'T']);
  });
  it('walks from open goals to review, assembly, writeback and done', () => {
    const b = project([fact('t', 'T', ['A'])]);
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'todo', view: 'tasks', text: expect.stringMatching(/^1 goal still needs a proof: A\./) });
    // A proposed split of A waits for the owner, whom the step names.
    b.facts.p = fact('p', 'A', ['B'], 'proposed');
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'todo', view: 'decompositions', text: expect.stringMatching(/your review/) });
    expect(nextStep(b, 'main', false)).toMatchObject({ tone: 'wait', text: expect.stringMatching(/the owner's review/) });
    // A proved: the route closes, and the owner assembles it.
    b.facts.a = fact('a', 'A');
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'todo', view: 'review', text: expect.stringMatching(/^Every goal on main's route has a proof\. Assemble/) });
    const run = (id: string, status: Run['status'], failure?: string): Run =>
      ({ id, component: 'lean-worker', status, branch: 'main', route: { root: 'T', steps: [] }, ...(failure ? { failure } : {}) });
    b.runs = { r1: run('r1', 'running') };
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'wait', text: expect.stringMatching(/^Assembling/) });
    b.runs = { r1: run('r1', 'failed', 'lean_error') };
    expect(nextStep(b, 'main', true)?.text).toMatch(/failed \(lean_error\)/);
    // The latest run counts: a later certified run waits to be written back.
    b.runs.r2 = run('r2', 'certified');
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'todo', text: expect.stringMatching(/Preview what it changes/) });
    expect(nextStep(b, 'main', false)).toMatchObject({ tone: 'wait', text: expect.stringMatching(/waits for the owner/) });
    b.branches!.main.writebacks = { w: { batch_id: 'w', run: 'r2', status: 'committed', created: [], updated: [] } };
    expect(nextStep(b, 'main', true)).toMatchObject({ tone: 'done', text: expect.stringMatching(/^Proved\./) });
  });
  it('defers to the warnings and names unattached, converting and refuted branches', () => {
    const b = project([]);
    expect(nextStep(b, 'alt', true)).toMatchObject({ text: expect.stringMatching(/not attached/) });
    expect(nextStep({ ...b, ready: false }, 'main', true)).toMatchObject({ tone: 'wait', text: expect.stringMatching(/^Converting/) });
    expect(nextStep({ ...b, sync_error: 'down' }, 'main', true)).toBeNull();
    expect(nextStep({ ...b, branches: { main: { ...b.branches!.main, request: { ...request, stale: true } } } }, 'main', true)).toBeNull();
    expect(nextStep({ ...b, project: { ...b.project!, superseded: true } }, 'main', true)).toBeNull();
    expect(nextStep({ ...b, goals: { ...b.goals, T: { ...goal('T'), refuted: { certificate: 'n' } } } }, 'main', true)).toMatchObject({ tone: 'done', text: expect.stringMatching(/^Refuted/) });
  });
});

describe('sending commands', () => {
  afterEach(() => vi.unstubAllGlobals());
  const headers = (call: unknown[]) => new Headers((call[1] as RequestInit).headers);
  it('sends an outdated revision again at the latest one, once the board says which', async () => {
    const fetchMock = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(Response.json({ reason: 'revision_conflict' }, { status: 409 }))
      .mockResolvedValueOnce(Response.json({ id: 'p', revision: 7 }))
      .mockResolvedValueOnce(Response.json({ revision: 8, result: { id: 'run' } }));
    vi.stubGlobal('fetch', fetchMock);
    const reply = await coordination.command({ id: 'p', revision: 5 }, { type: 'assemble', component: 'lean-worker', branch: 'main' });
    expect(reply.revision).toBe(8);
    expect(headers(fetchMock.mock.calls[0]!).get('if-match')).toBe('"5"');
    expect(headers(fetchMock.mock.calls[2]!).get('if-match')).toBe('"7"');
    // The same request, so a lost reply would replay rather than run twice.
    expect(headers(fetchMock.mock.calls[2]!).get('idempotency-key')).toBe(headers(fetchMock.mock.calls[0]!).get('idempotency-key'));
  });
  it('gives up after three conflicts and passes other refusals on at once', async () => {
    const conflict = () => Promise.resolve(Response.json({ reason: 'revision_conflict' }, { status: 409 }));
    const board = () => Promise.resolve(Response.json({ id: 'p', revision: 1 }));
    const fetchMock = vi.fn<typeof fetch>((input) => (String(input).endsWith('/commands') ? conflict() : board()));
    vi.stubGlobal('fetch', fetchMock);
    await expect(coordination.command({ id: 'p', revision: 0 }, { type: 'accept', run: 'r' })).rejects.toMatchObject({ message: 'revision_conflict' });
    expect(fetchMock.mock.calls.filter((c) => String(c[0]).endsWith('/commands'))).toHaveLength(4);
    fetchMock.mockImplementation(() => Promise.resolve(Response.json({ reason: 'owner_required' }, { status: 403 })));
    fetchMock.mockClear();
    await expect(coordination.command({ id: 'p', revision: 0 }, { type: 'accept', run: 'r' })).rejects.toMatchObject({ message: 'owner_required' });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
  it('sends commands checked against the current board with If-Match *', async () => {
    const fetchMock = vi.fn<typeof fetch>(() => Promise.resolve(Response.json({ revision: 3, result: {} })));
    vi.stubGlobal('fetch', fetchMock);
    await coordination.command({ id: 'p', revision: 3 }, { type: 'note', goal: 'G', text: 'tried induction' });
    expect(headers(fetchMock.mock.calls[0]!).get('if-match')).toBe('*');
  });
});
