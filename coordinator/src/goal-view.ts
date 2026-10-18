// Read views of a project board for `mdc goals`, `mdc goal` and `mdc review` (docs/mathdoc.md
// §2.7.7). They are computed from the board the reader may see, so a branch the reader cannot
// read never appears in them.
import {
  failureCounts,
  type Board,
  type Fact,
  type FactState,
  type Goal,
} from "./domain.js";

/** Goals each branch's route reaches: from the root through active certificates, following its picks. */
export function routeReach(b: Board): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  const usable = Object.values(b.facts).filter(
    (f) => f.available && f.state === "active",
  );
  for (const [name, branch] of Object.entries(b.branches ?? {})) {
    const reach = new Set(b.root ? [b.root] : []);
    const queue = [...reach];
    while (queue.length) {
      const g = queue.pop()!;
      for (const f of usable)
        if (
          f.goal === g &&
          (!Object.hasOwn(branch.picks, g) || branch.picks[g] === f.id)
        )
          for (const p of f.premises)
            if (!reach.has(p)) {
              reach.add(p);
              queue.push(p);
            }
    }
    out.set(name, reach);
  }
  return out;
}

/** A readable name: a node proving the goal, its declared name, else its statement. */
export function goalName(b: Board, goal: string): string {
  // A module project's contribution files are named after their paths, not their goal.
  for (const branch of Object.values(b.branches ?? {}))
    for (const n of Object.values(branch.nodes))
      if (n.goal === goal && !n.file) return n.title;
  const g = b.goals[goal];
  return g?.declaration?.name ?? (g?.statement || `${goal.slice(0, 10)}…`);
}

/** The goal as Lean states it: its declaration, else its proposition, else its label. */
export function goalStatement(g: Goal | undefined): string {
  if (g?.declaration)
    return `${g.declaration.name}${g.declaration.binders ? ` ${g.declaration.binders}` : ""} : ${g.declaration.type}`;
  return g?.proposition ?? g?.statement ?? "";
}

function needing(reach: Map<string, Set<string>>, goal: string) {
  return [...reach].filter(([, r]) => r.has(goal)).map(([name]) => name);
}

/** The open prove task of a goal, with its holder while leased. */
function taskOf(b: Board, goal: string) {
  const t = Object.values(b.tasks).find(
    (x) => x.goal === goal && x.kind === "prove" && x.state !== "cancelled",
  );
  if (!t) return null;
  const a = t.attempt ? b.attempts[t.attempt] : undefined;
  return {
    id: t.id,
    state: t.state,
    ...(a && !a.outcome ? { holder: a.actor, expires: a.expires } : {}),
  };
}

const premiseView = (b: Board, goal: string) => ({
  goal,
  name: goalName(b, goal),
  status: b.goals[goal]?.status ?? "insufficient",
  ...(b.goals[goal]?.refuted ? { refuted: true } : {}),
});

export interface GoalFilter {
  /** insufficient, derivable, certified or refuted. */
  status?: string;
  /** Only goals this branch's route reaches. */
  branch?: string;
}

/** One line per goal (`mdc goals`), the root first, then by name. */
export function goalSummaries(b: Board, filter: GoalFilter = {}) {
  const reach = routeReach(b);
  const failed = failureCounts(b);
  const certificates = new Map<string, Partial<Record<FactState, number>>>();
  for (const f of Object.values(b.facts)) {
    const c = certificates.get(f.goal) ?? {};
    c[f.state] = (c[f.state] ?? 0) + 1;
    certificates.set(f.goal, c);
  }
  return Object.values(b.goals)
    .filter(
      (g) =>
        (!filter.status ||
          (filter.status === "refuted"
            ? !!g.refuted
            : g.status === filter.status)) &&
        (!filter.branch || reach.get(filter.branch)?.has(g.key)),
    )
    .map((g) => ({
      goal: g.key,
      name: goalName(b, g.key),
      statement: goalStatement(g),
      status: g.status,
      refuted: !!g.refuted,
      negates: b.negations?.[g.key] ?? null,
      root: g.key === b.root,
      needed_by: needing(reach, g.key),
      certificates: certificates.get(g.key) ?? {},
      task: taskOf(b, g.key),
      notes: g.notes?.length ?? 0,
      failures: failed.get(g.key) ?? 0,
    }))
    .sort(
      (x, y) => Number(y.root) - Number(x.root) || x.name.localeCompare(y.name),
    );
}

function certificateView(b: Board, f: Fact, picks: Map<string, string[]>) {
  return {
    id: f.id,
    state: f.state,
    trust: f.trust,
    available: f.available,
    complete: f.premises.length === 0,
    premises: f.premises.map((p) => premiseView(b, p)),
    picked_by: picks.get(f.id) ?? [],
    ...(f.description ? { description: f.description } : {}),
    ...(f.submitter
      ? { submitter: f.submitter, submitted_at: f.submitted_at }
      : {}),
    ...(f.review ? { review: f.review } : {}),
    ...(f.retired_reason ? { retired_reason: f.retired_reason } : {}),
    ...(f.refuted_by ? { refuted_by: premiseView(b, f.refuted_by) } : {}),
  };
}

/** Which branches pick each certificate. */
function pickers(b: Board) {
  const out = new Map<string, string[]>();
  for (const [name, branch] of Object.entries(b.branches ?? {}))
    for (const cert of Object.values(branch.picks))
      out.set(cert, [...(out.get(cert) ?? []), name]);
  return out;
}

/** Everything about one goal (`mdc goal`). */
export function goalDetail(b: Board, goal: string) {
  const g = b.goals[goal];
  const reach = routeReach(b);
  const picks = pickers(b);
  const target = b.negations?.[goal];
  const negation = Object.entries(b.negations ?? {}).find(
    ([, t]) => t === goal,
  )?.[0];
  return {
    goal,
    name: goalName(b, goal),
    statement: goalStatement(g),
    proposition: g.proposition ?? null,
    declaration: g.declaration ?? null,
    definitions: g.definitions ?? [],
    status: g.status,
    root: goal === b.root,
    refuted: g.refuted ?? null,
    ...(target ? { negates: premiseView(b, target) } : {}),
    ...(negation ? { negated_by: premiseView(b, negation) } : {}),
    needed_by: needing(reach, goal),
    picks: Object.fromEntries(
      Object.entries(b.branches ?? {})
        .filter(([, s]) => Object.hasOwn(s.picks, goal))
        .map(([name, s]) => [name, s.picks[goal]]),
    ),
    certificates: Object.values(b.facts)
      .filter((f) => f.goal === goal)
      .map((f) => certificateView(b, f, picks)),
    premise_of: Object.values(b.facts)
      .filter((f) => f.premises.includes(goal))
      .map((f) => ({
        certificate: f.id,
        state: f.state,
        goal: premiseView(b, f.goal),
      })),
    tasks: Object.values(b.tasks)
      .filter((t) => t.goal === goal)
      .map((t) => ({
        ...t,
        attempts: Object.values(b.attempts).filter((a) => a.task === t.id),
      })),
    failures: failureCounts(b).get(goal) ?? 0,
    notes: g.notes ?? [],
  };
}

/** Decompositions in a state, oldest submission first (`mdc review` lists `proposed`). */
export function decompositions(b: Board, state?: FactState) {
  const reach = routeReach(b);
  const picks = pickers(b);
  return Object.values(b.facts)
    .filter((f) => f.premises.length && (!state || f.state === state))
    .sort(
      (x, y) =>
        (x.submitted_at ?? 0) - (y.submitted_at ?? 0) ||
        x.id.localeCompare(y.id),
    )
    .map((f) => ({
      ...certificateView(b, f, picks),
      goal: {
        ...premiseView(b, f.goal),
        statement: goalStatement(b.goals[f.goal]),
      },
      needed_by: needing(reach, f.goal),
    }));
}
