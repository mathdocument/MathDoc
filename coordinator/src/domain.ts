import { randomUUID } from "node:crypto";
import { z } from "zod";
import type { NodeBinding, ProofRequest } from "./proof.js";
import type { Writeback } from "./writeback.js";

export class Fault extends Error {
  constructor(
    public code: string,
    public status = 409,
  ) {
    super(code);
  }
}
export function requireThat(
  condition: unknown,
  code: string,
  status = 409,
): asserts condition {
  if (!condition) throw new Fault(code, status);
}
export const trustSchema = z.enum(["claimed", "audited", "trusted"]);
export type Trust = z.infer<typeof trustSchema>;
export const trustRank = { claimed: 1, audited: 2, trusted: 3 };
export const identitySchema = z
  .object({
    key: z.string().min(1),
    base_fingerprint: z.string().min(1),
    local_context_hash: z.string().min(1),
    relevant_options_hash: z.string().min(1),
  })
  .passthrough();
export type Identity = z.infer<typeof identitySchema>;
export const nodeRefSchema = z
  .object({
    database: z.string().regex(/^[\w-]+$/),
    branch: z.string().regex(/^[\w-]+$/),
    node: z.string().uuid(),
    revision: z.string().min(1),
  })
  .strict();
export type NodeRef = z.infer<typeof nodeRefSchema>;
// Branch names key board.branches: disallow special JS dictionary keys defensively.
const branchName = z
  .string()
  .regex(/^[\w-]+$/)
  .max(200)
  .refine((x) => !["__proto__", "constructor", "prototype"].includes(x), {
    message: "reserved branch name",
  });
export const createSchema = z
  .object({
    title: z.string().trim().min(1).max(200),
    proposition: z.string().min(1).max(100000),
    base_key: z.number().int().nonnegative(),
    context: z.record(z.unknown()).default({}),
    options: z.record(z.unknown()).default({}),
    members: z.array(z.string().min(1)).max(100),
    budget: z.number().int().min(0).max(1e12),
    minimum_trust: trustSchema.default("audited"),
    node_ref: nodeRefSchema.optional(),
  })
  .strict();
export type Create = z.infer<typeof createSchema>;
/**
 * Submit a branch of a database: creates the database's project board rooted at `node`, or
 * attaches the branch to the existing one. The environment comes from the database.
 */
export const documentCreateSchema = z
  .object({
    title: z.string().trim().min(1).max(200).optional(),
    document: z
      .object({
        database: z.string().regex(/^[\w-]+$/),
        branch: branchName,
        node: z.string().uuid(),
      })
      .strict(),
    members: z.array(z.string().min(1)).max(100).default([]),
    budget: z.number().int().min(0).max(1e12),
  })
  .strict();
export type DocumentCreate = z.infer<typeof documentCreateSchema>;
export interface Goal {
  key: string;
  identity?: Identity;
  /** A display label: a node title, a placeholder name or the proposition. */
  statement: string;
  status: "insufficient" | "derivable" | "certified";
  /**
   * The goal's statement as resolved (docs/mathdoc.md §4.11): the proposition, the definition
   * IDs it was resolved with and, when a node or a sketch placeholder states it, that
   * declaration. Goal drafts are written from it.
   */
  proposition?: string;
  definitions?: string[];
  declaration?: Declaration;
  /** Attempt notes, oldest first; the latest NOTES_KEPT are kept (docs/mathdoc.md §2.7.3). */
  notes?: Note[];
  /** Set when a complete proof of the goal's negation exists (§2.7.5): the goal is false. */
  refuted?: { certificate: string; at?: number };
  /** Module projects: where the goal is stated (docs/mathdoc.md §4.13). */
  module?: ModuleStatement;
}
/**
 * A goal of a module project is a constant of some Lean files: the root's statement in the root
 * node's file, a premise's placeholder in the decomposition that introduced it. Proofs restate
 * it; LeanGround connects them to its own statement source by the kernel, so this is the text
 * goal drafts start from, never what decides a proof.
 */
export interface ModuleStatement {
  /** The statement constant's full name. */
  const: string;
  /** Shared modules the stating files import (with their dependencies, LeanGround resolves). */
  shared: string[];
  /** The root: the document node holding the statement file, with the file's path. */
  node?: string;
  path?: string;
  /** A premise: the decomposition certificate whose placeholder states it. */
  certificate?: string;
  /**
   * A negation (docs/mathdoc.md §4.13): the goal it refutes, and the file of that goal's
   * statement at whose end `¬` is stated.
   */
  negates?: { goal: string; path: string };
}
/** A shared module of a module project: files registered once, imported by statements. */
export interface SharedModule {
  id: string;
  /** Lean module names, as `A.B`. */
  modules: string[];
  /** The document nodes holding its files (the root's dependencies), when it came from one. */
  nodes?: string[];
  /** The decomposition that registered it (`mdc split --shared`). */
  certificate?: string;
}
/** What someone learned working on a goal: what was tried, where it got stuck. */
export interface Note {
  actor: string;
  at: number;
  text: string;
  /** Set when the note came with `finish`. */
  attempt?: string;
  /** When its author last changed the text. */
  edited?: number;
}
export const NOTES_KEPT = 50;
export const NOTE_LENGTH = 500;
/** A theorem's name and signature, as `theorem name binders : type`. */
export interface Declaration {
  name: string;
  binders: string;
  type: string;
}
export interface Fact {
  id: string;
  goal: string;
  premises: string[];
  trust: Trust;
  state: FactState;
  available: boolean;
  /** What a decomposition does, from `submit` (`mdc split -m`). */
  description?: string;
  /** Who submitted it through MathDoc, and when; absent for certificates found by sync. */
  submitter?: string;
  submitted_at?: number;
  /** The last owner decision on it. */
  review?: Review;
  /** Retired on arrival: a premise is the goal itself (T ⇐ {T}), so it can never help. */
  retired_reason?: "premise_is_goal";
  /** For a refuted certificate: the premise proved false. */
  refuted_by?: string;
}
/**
 * docs/mathdoc.md §2.7.3: only `active` certificates take part in routes, claims and assembly.
 * A `proposed` decomposition waits for an owner's review.
 */
export type FactState =
  "proposed" | "active" | "paused" | "retired" | "refuted";
export interface Review {
  actor: string;
  at: number;
  state: Exclude<FactState, "proposed">;
  comment?: string;
}
/**
 * Where a certificate reaches the board. Complete proofs are active at once; a sketch from a
 * document node is active because the branch's editor wrote it; other sketches are proposed
 * on project boards unless the project waives review.
 */
export type Origin = {
  source: "agent" | "document" | "sync" | "assembly";
  actor?: string;
  at?: number;
};
export interface Task {
  id: string;
  goal: string;
  kind: "prove" | "decompose" | "formalize";
  priority: number;
  state: "ready" | "leased" | "cancelled";
  epoch: number;
  attempt?: string;
}
export interface Attempt {
  id: string;
  task: string;
  actor: string;
  epoch: number;
  expires: number;
  reserved: number;
  spent: number;
  outcome?: string;
}
export interface Step {
  certificate_id: string;
  providers: Record<string, string>;
}
export interface Route {
  root: string;
  steps: Step[];
}
export interface Run {
  id: string;
  route: Route;
  component: string;
  /** Project boards: the branch whose picks chose the route and which receives the writeback. */
  branch?: string;
  plan_id?: string;
  status: "queued" | "running" | "failed" | "certified";
  certification_id?: string;
  failure?: string;
  /** LeanGround's explanation of the failure, e.g. Lean's messages. */
  details?: string;
  retryable?: boolean;
}
export interface Board {
  id: string;
  title: string;
  owner: string;
  members: string[];
  revision: number;
  base: Record<string, unknown>;
  context: Record<string, unknown>;
  options: Record<string, unknown>;
  root: string;
  minimum_trust: Trust;
  node_ref?: NodeRef;
  notes: string;
  budget: number;
  spent: number;
  cursor: number;
  ready: boolean;
  last_sync?: number;
  sync_error?: string;
  goals: Record<string, Goal>;
  facts: Record<string, Fact>;
  tasks: Record<string, Task>;
  attempts: Record<string, Attempt>;
  runs: Record<string, Run>;
  accepted?: Accepted;
  /** Legacy per-branch proof requests (stage 4), now read-only: the ProofRequest, its nodes and writebacks. */
  request?: ProofRequest;
  nodes?: Record<string, NodeBinding>;
  writebacks?: Record<string, Writeback>;
  /** Project boards (docs/mathdoc.md §2.3): one per database and environment. */
  project?: ProjectScope;
  branches?: Record<string, BranchState>;
  /** Project boards: whether new sketches wait for review (absent: they do). */
  review?: "required" | "none";
  /** Negation goal → the goal it refutes once completely proved (§2.7.5). */
  negations?: Record<string, string>;
  /** Module projects (docs/mathdoc.md §4.13); absent: declarations. */
  mode?: "modules";
  /** Module projects: the shared modules goals are stated with, by ID. */
  shared_modules?: Record<string, SharedModule>;
}
export interface Accepted {
  certification_id: string;
  revision: string;
  operation: string;
}
/** A database's proof project: one root theorem in one fixed environment. */
export interface ProjectScope {
  database: string;
  environment_id: string;
  replaces_environment_id?: string;
  /** The root theorem node; every attached branch is converted from it. */
  root_node: string;
  /** Set when the database's environment changed: the board stays readable only. */
  superseded?: boolean;
  /** Set when the database was removed; `database` then holds a tombstone name. */
  removed?: { database: string; at: number; by: string };
}
/** One branch attached to a project board: its conversion, route picks and writebacks. */
export interface BranchState {
  request: ProofRequest;
  nodes: Record<string, NodeBinding>;
  /** goal → certificate this branch's route uses; unpicked goals are chosen automatically. */
  picks: Record<string, string>;
  writebacks: Record<string, Writeback>;
  accepted?: Accepted;
}
/**
 * The person behind an actor: `alice` for `alice/codex-1`, a delegated token (docs/mathdoc.md
 * §2.7.4). Membership and ownership are the person's; leases and audit keep the full actor.
 */
export function principalOf(actor: string) {
  return actor.split("/")[0];
}
export function member(b: Board, actor: string) {
  requireThat(b.members.includes(principalOf(actor)), "not_found", 404);
}
export function owner(b: Board, actor: string) {
  member(b, actor);
  requireThat(b.owner === principalOf(actor), "owner_required", 403);
}
/** Why a board accepts no commands, if it does not. */
export function readOnly(b: Board): string | undefined {
  if (b.request) return "proof_request_read_only";
  if (b.project?.superseded) return "project_superseded";
}
export function attached(b: Board, branch: string | undefined): BranchState {
  requireThat(b.project, "not_a_project_board", 400);
  requireThat(
    branch !== undefined && b.branches && Object.hasOwn(b.branches, branch),
    "branch_not_attached",
  );
  return b.branches![branch!];
}
export function newBoard(
  id: string,
  actor: string,
  input: Create,
  base: Record<string, unknown>,
  goal: Identity,
): Board {
  const b: Board = {
    id,
    title: input.title,
    owner: actor,
    members: [...new Set([actor, ...input.members])],
    revision: 0,
    base,
    context: input.context,
    options: input.options,
    root: goal.key,
    minimum_trust: input.minimum_trust,
    node_ref: input.node_ref,
    notes: "",
    budget: input.budget,
    spent: 0,
    cursor: 0,
    ready: false,
    goals: {},
    facts: {},
    tasks: {},
    attempts: {},
    runs: {},
  };
  addGoal(b, goal.key, goal, input.proposition);
  return b;
}
/** A project board for the database; the creating branch is attached by the provision job. */
export function newProjectBoard(
  id: string,
  actor: string,
  input: DocumentCreate,
  title: string,
  env: {
    base: Record<string, unknown>;
    context: Record<string, unknown>;
    options: Record<string, unknown>;
    minimum_trust: Trust;
    environment_id: string;
    replaces_environment_id?: string;
    mode?: "declarations" | "modules";
  },
): Board {
  const b: Board = {
    id,
    title: input.title ?? title,
    owner: actor,
    members: [...new Set([actor, ...input.members])],
    revision: 0,
    base: env.base,
    context: env.context,
    options: env.options,
    // The root goal is resolved when the first branch is attached.
    root: "",
    minimum_trust: env.minimum_trust,
    notes: "",
    budget: input.budget,
    spent: 0,
    cursor: 0,
    ready: false,
    goals: {},
    facts: {},
    tasks: {},
    attempts: {},
    runs: {},
    project: {
      database: input.document.database,
      environment_id: env.environment_id,
      ...(env.replaces_environment_id
        ? { replaces_environment_id: env.replaces_environment_id }
        : {}),
      root_node: input.document.node,
    },
    branches: {},
  };
  // docs/mathdoc.md §2.6 module decision 4: decompositions take effect at once; the
  // owner can pause or retire one afterwards.
  if (env.mode === "modules") Object.assign(b, { mode: "modules", review: "none" });
  b.branches![input.document.branch] = newBranchState(b, input.document.branch);
  return b;
}
export function newBranchState(b: Board, branch: string): BranchState {
  const p = b.project!;
  return {
    request: {
      database: p.database,
      branch,
      node: p.root_node,
      environment_id: p.environment_id,
      ...(p.replaces_environment_id
        ? { replaces_environment_id: p.replaces_environment_id }
        : {}),
      bound: false,
      stale: false,
    },
    nodes: {},
    picks: {},
    writebacks: {},
  };
}
/** A branch's submitted nodes choose its route at their goals. */
export function derivePicks(b: Board, branch: BranchState) {
  for (const n of Object.values(branch.nodes))
    if (n.goal && n.certificate && b.facts[n.certificate]?.goal === n.goal)
      branch.picks[n.goal] = n.certificate;
}
export function addGoal(
  b: Board,
  key: string,
  identity?: Identity,
  statement = "",
) {
  // Keys come from LeanGround, but disallow special JS dictionary keys defensively.
  requireThat(
    !["__proto__", "constructor", "prototype"].includes(key),
    "invalid_goal",
  );
  if (!Object.hasOwn(b.goals, key)) {
    b.goals[key] = { key, identity, statement, status: "insufficient" };
    const id = randomUUID();
    b.tasks[id] = {
      id,
      goal: key,
      kind: "prove",
      priority: 0,
      state: "ready",
      epoch: 0,
    };
  } else {
    if (identity) b.goals[key].identity = identity;
    if (statement) b.goals[key].statement = statement;
  }
}
/**
 * Record how a goal is stated. Every statement resolved to this key means the same goal, so
 * the first proposition and declaration stay; definition IDs accumulate (a GoalKey depends
 * only on the definitions its statement references, which every list that elaborates it holds).
 */
export function stateGoal(
  b: Board,
  key: string,
  statement: {
    proposition: string;
    definitions: string[];
    declaration?: Declaration;
  },
) {
  if (!Object.hasOwn(b.goals, key)) return;
  const g = b.goals[key];
  g.proposition ??= statement.proposition;
  g.definitions = [
    ...new Set([...(g.definitions ?? []), ...statement.definitions]),
  ].sort();
  if (!g.declaration && statement.declaration)
    g.declaration = { ...statement.declaration };
  if (!g.statement && statement.declaration)
    g.statement = statement.declaration.name;
}
export const factSchema = z
  .object({
    id: z.string().min(1),
    goal: z.string().min(1),
    premises: z.array(z.string()),
    trust: trustSchema,
    request: z.object({ goal: identitySchema }),
    access: z.object({
      visibility: z.enum(["private", "project", "public"]),
      project_id: z.string().nullable().optional(),
    }),
  })
  .passthrough();
/** The state of a certificate new to the board. */
function initialState(
  b: Board,
  goal: string,
  premises: string[],
  origin: Origin,
): FactState {
  if (premises.includes(goal)) return "retired";
  if (!premises.length || !b.project) return "active";
  if (origin.source === "document" || origin.source === "assembly")
    return "active";
  return b.review === "none" ? "active" : "proposed";
}
/**
 * Whether a certificate can enter the board: shared with this project, in its environment and
 * trusted enough. Changes nothing, so checking needs no copy of the board (§2.7.5).
 */
export function checkFact(b: Board, raw: unknown) {
  const f = factSchema.parse(raw);
  requireThat(
    f.access.visibility === "public" ||
      (f.access.visibility === "project" && f.access.project_id === b.id),
    "fact_not_shared",
  );
  // A project board learns its environment identity from its first attached branch.
  requireThat(b.goals[b.root]?.identity, "project_root_unbound");
  const root = b.goals[b.root].identity!;
  for (const k of [
    "base_fingerprint",
    "local_context_hash",
    "relevant_options_hash",
  ] as const)
    requireThat(f.request.goal[k] === root[k], "environment_mismatch");
  requireThat(f.request.goal.key === f.goal, "identity_mismatch");
  requireThat(
    trustRank[f.trust] >= trustRank[b.minimum_trust],
    "trust_rejected",
  );
  for (const key of [f.id, f.goal, ...f.premises])
    requireThat(
      !["__proto__", "constructor", "prototype"].includes(key),
      "invalid_fact",
    );
  return f;
}
/** Add or refresh a certificate. A sync adding many recomputes statuses once, at its end. */
export function ingest(
  b: Board,
  raw: unknown,
  origin: Origin = { source: "agent" },
  recomputeStatus = true,
) {
  const f = checkFact(b, raw);
  addGoal(b, f.goal, f.request.goal);
  for (const key of f.premises) addGoal(b, key);
  const prior = Object.hasOwn(b.facts, f.id) ? b.facts[f.id] : undefined;
  const premises = [...new Set(f.premises)];
  // Re-ingestion (every sync) refreshes the certificate and keeps what the board added.
  b.facts[f.id] = {
    ...prior,
    id: f.id,
    goal: f.goal,
    premises,
    trust: f.trust,
    state: prior?.state ?? initialState(b, f.goal, premises, origin),
    available: true,
  };
  if (!prior) {
    const fact = b.facts[f.id];
    if (origin.actor) fact.submitter = origin.actor;
    if (origin.at !== undefined) fact.submitted_at = origin.at;
    if (premises.includes(f.goal)) fact.retired_reason = "premise_is_goal";
  }
  // A complete proof of a registered negation refutes its goal.
  if (!premises.length && b.negations && Object.hasOwn(b.negations, f.goal))
    refute(b, b.negations[f.goal], f.id, origin.at);
  if (recomputeStatus) recompute(b);
}
/**
 * docs/mathdoc.md §2.7.5: `goal` is false. Certificates using it as a premise are refuted and
 * leave every route; tasks of the goal, and of goals only those certificates reached, are
 * cancelled, ending their leases; the goals of refuted certificates get a note saying why.
 */
export function refute(
  b: Board,
  goal: string,
  certificate: string,
  at?: number,
) {
  const g = b.goals[goal];
  if (!g || g.refuted) return;
  const before = activeGoals(b);
  g.refuted = { certificate, ...(at !== undefined ? { at } : {}) };
  const refuted = Object.values(b.facts).filter(
    (f) => f.premises.includes(goal) && f.state !== "refuted",
  );
  for (const f of refuted) {
    f.state = "refuted";
    f.refuted_by = goal;
  }
  const after = activeGoals(b);
  for (const t of Object.values(b.tasks)) {
    if (t.state === "cancelled") continue;
    if (t.goal !== goal && (!before.has(t.goal) || after.has(t.goal))) continue;
    if (t.attempt && Object.hasOwn(b.attempts, t.attempt)) {
      const a = b.attempts[t.attempt];
      if (!a.outcome) {
        // As when a member leaves: the reservation may be spent.
        a.outcome = "cancelled";
        a.spent = a.reserved;
        b.spent += a.spent;
      }
    }
    t.state = "cancelled";
    delete t.attempt;
  }
  const label = g.declaration?.name ?? (g.statement || goal.slice(0, 10));
  const actor = "mathdoc";
  const note = (on: string, text: string) =>
    addNote(b, on, { actor, at: at ?? 0, text: text.slice(0, NOTE_LENGTH) });
  note(
    goal,
    `Refuted: certificate ${certificate.slice(0, 10)} proves its negation.`,
  );
  for (const f of refuted)
    note(
      f.goal,
      `Decomposition ${f.id.slice(0, 10)}${f.submitter ? ` by ${f.submitter}` : ""} is refuted: its premise ${label} is false.`,
    );
}
/** Keep a note on a goal; the oldest beyond NOTES_KEPT are dropped. */
export function addNote(b: Board, goal: string, note: Note) {
  const g = b.goals[goal];
  g.notes = [...(g.notes ?? []), note].slice(-NOTES_KEPT);
  return note;
}
export function recompute(b: Board) {
  for (const g of Object.values(b.goals)) g.status = "insufficient";
  const solved = new Set<string>();
  let changed = true;
  while (changed) {
    changed = false;
    for (const f of Object.values(b.facts).filter((f) => f.available)) {
      if (f.premises.every((p) => solved.has(p))) {
        if (!solved.has(f.goal)) {
          solved.add(f.goal);
          changed = true;
        }
        if (!f.premises.length) b.goals[f.goal].status = "certified";
        else if (b.goals[f.goal].status !== "certified")
          b.goals[f.goal].status = "derivable";
      }
    }
  }
}
/**
 * Goals worth scheduling: reachable from the root through active certificates, and the
 * registered negations of such goals (refuting one is as useful as proving it).
 */
export function activeGoals(b: Board) {
  const active = new Set([b.root]);
  let before = -1;
  while (before !== active.size) {
    before = active.size;
    for (const f of Object.values(b.facts))
      if (f.available && f.state === "active" && active.has(f.goal))
        for (const p of f.premises) active.add(p);
    for (const [negation, target] of Object.entries(b.negations ?? {}))
      if (active.has(target) && !b.goals[target]?.refuted) active.add(negation);
  }
  return active;
}
/** A goal recommended by `next` (docs/mathdoc.md §2.7.5). */
export interface Candidate {
  goal: string;
  task: string;
  score: number;
  /** Branches whose routes need the goal. */
  branches: string[];
  failures: number;
}
const FAILED = new Set(["no_progress", "execution_failed", "expired"]);
/** Failed attempts per goal: no progress, a failed execution or a lapsed lease. */
export function failureCounts(b: Board): Map<string, number> {
  const out = new Map<string, number>();
  for (const a of Object.values(b.attempts)) {
    const goal = b.tasks[a.task]?.goal;
    if (a.outcome && FAILED.has(a.outcome) && goal)
      out.set(goal, (out.get(goal) ?? 0) + 1);
  }
  return out;
}
/**
 * What to work on next: goals on the frontier of the branches' routes (needed, unsolved, with no
 * active decomposition of their own), each scored by the routes needing it, weighted by how
 * close each route is to closing (1 / its unsolved goals), divided by 1 + its failed attempts.
 * Negations count for the goal they refute. Free, unleased prove tasks only; best first.
 */
export function rankGoals(b: Board, branch?: string): Candidate[] {
  const usable = Object.values(b.facts).filter(
    (f) => f.available && f.state === "active",
  );
  const solved = new Set<string>();
  for (let changed = true; changed;) {
    changed = false;
    for (const f of usable)
      if (!solved.has(f.goal) && f.premises.every((p) => solved.has(p))) {
        solved.add(f.goal);
        changed = true;
      }
  }
  const routes: [string, Record<string, string>][] = b.project
    ? Object.entries(b.branches ?? {})
        .filter(([name, s]) => s.request.bound && (!branch || name === branch))
        .map(([name, s]) => [name, s.picks])
    : [["board", {}]];
  const score = new Map<string, number>();
  const needers = new Map<string, string[]>();
  for (const [name, picks] of routes) {
    // The unsolved goals this route can still need, following its picks.
    const needed = new Set<string>();
    const queue = [b.root];
    while (queue.length) {
      const g = queue.pop()!;
      if (needed.has(g) || solved.has(g) || b.goals[g]?.refuted) continue;
      needed.add(g);
      for (const f of usable)
        if (f.goal === g && (!Object.hasOwn(picks, g) || picks[g] === f.id))
          queue.push(...f.premises);
    }
    const weight = needed.size ? 1 / needed.size : 0;
    for (const g of needed) {
      score.set(g, (score.get(g) ?? 0) + weight);
      needers.set(g, [...(needers.get(g) ?? []), name]);
    }
  }
  for (const [negation, target] of Object.entries(b.negations ?? {}))
    if (score.has(target)) {
      score.set(negation, score.get(target)!);
      needers.set(negation, needers.get(target)!);
    }
  const decomposed = new Set(
    usable.filter((f) => f.premises.length).map((f) => f.goal),
  );
  const failed = failureCounts(b);
  const out: Candidate[] = [];
  for (const t of Object.values(b.tasks)) {
    const g = b.goals[t.goal];
    if (
      t.kind !== "prove" ||
      t.state !== "ready" ||
      !g ||
      g.status === "certified" ||
      g.refuted ||
      decomposed.has(t.goal) ||
      !score.has(t.goal)
    )
      continue;
    const failures = failed.get(t.goal) ?? 0;
    out.push({
      goal: t.goal,
      task: t.id,
      score: Math.round((score.get(t.goal)! / (1 + failures)) * 1e6) / 1e6,
      branches: needers.get(t.goal)!,
      failures,
    });
  }
  return out.sort(
    (x, y) =>
      y.score - x.score ||
      x.failures - y.failures ||
      x.goal.localeCompare(y.goal),
  );
}
/** A route through active facts; a picked goal is proved only by its picked certificate. */
export function selectRoute(
  b: Board,
  picks: Record<string, string> = {},
): Route {
  const solved = new Map<string, string>();
  const steps: Step[] = [];
  let before = -1;
  const facts = Object.values(b.facts)
    .filter(
      (f) =>
        f.available &&
        f.state === "active" &&
        (!Object.hasOwn(picks, f.goal) || picks[f.goal] === f.id),
    )
    .sort(
      (a, c) =>
        a.premises.length - c.premises.length || a.id.localeCompare(c.id),
    );
  while (before !== solved.size) {
    before = solved.size;
    for (const f of facts)
      if (!solved.has(f.goal) && f.premises.every((p) => solved.has(p))) {
        steps.push({
          certificate_id: f.id,
          providers: Object.fromEntries(
            f.premises.map((p) => [p, solved.get(p)!]),
          ),
        });
        solved.set(f.goal, f.id);
      }
    if (solved.has(b.root)) {
      const root = solved.get(b.root)!;
      const required = new Set([root]);
      for (const s of [...steps].reverse())
        if (required.has(s.certificate_id))
          for (const p of Object.values(s.providers)) required.add(p);
      return {
        root,
        steps: steps.filter((s) => required.has(s.certificate_id)),
      };
    }
  }
  throw new Fault("no_active_route");
}
export function expire(b: Board, now: number) {
  for (const a of Object.values(b.attempts))
    if (!a.outcome && a.expires <= now) {
      expireLease(a, b.tasks[a.task]);
      b.spent += a.spent;
    }
}
/**
 * The lease rules (§4.8), shared by the board path and the lease path of the store, which
 * keeps tasks and attempts in their own rows (§2.7.5). Each mutates only what it is given.
 */
export function takeLease(
  t: Task,
  actor: string,
  c: { allocation: number; ttl: number },
  now: number,
  ctx: {
    active: boolean;
    certified: boolean;
    committed: number;
    budget: number;
  },
): Attempt {
  requireThat(t.state === "ready", "task_unavailable");
  requireThat(ctx.active, "inactive_goal");
  requireThat(!ctx.certified, "already_certified");
  requireThat(ctx.committed + c.allocation <= ctx.budget, "budget_exhausted");
  const a: Attempt = {
    id: randomUUID(),
    task: t.id,
    actor,
    epoch: ++t.epoch,
    expires: now + c.ttl * 1000,
    reserved: c.allocation,
    spent: 0,
  };
  t.state = "leased";
  t.attempt = a.id;
  return a;
}
function holdsLease(
  a: Attempt,
  t: Task,
  actor: string,
  epoch: number,
  now: number,
) {
  requireThat(
    a.actor === actor &&
      a.epoch === epoch &&
      !a.outcome &&
      a.expires > now &&
      t.attempt === a.id,
    "stale_lease",
  );
}
export function renewLease(
  a: Attempt,
  t: Task,
  actor: string,
  c: { epoch: number; ttl: number },
  now: number,
) {
  holdsLease(a, t, actor, c.epoch, now);
  a.expires = now + c.ttl * 1000;
}
export function endLease(
  a: Attempt,
  t: Task,
  actor: string,
  c: { epoch: number; spent: number; outcome: string },
  now: number,
) {
  holdsLease(a, t, actor, c.epoch, now);
  requireThat(c.spent <= a.reserved, "allocation_exceeded");
  a.spent = c.spent;
  a.outcome = c.outcome;
  t.state = "ready";
  delete t.attempt;
}
/** A disconnected participant may have spent all of the reservation. */
export function expireLease(a: Attempt, t: Task) {
  a.outcome = "expired";
  a.spent = a.reserved;
  if (t.attempt === a.id) {
    t.state = "ready";
    delete t.attempt;
  }
}
const short = z.string().min(1).max(200);
const noteKey = z.object({ actor: short, at: z.number().int().nonnegative() }).strict();
const ttl = z.number().int().min(30).max(3600).default(300);
/** A contribution of a module project: Lean files by path, e.g. `Work/Main.lean` (LeanGround's limits). */
export const MODULE_PATH = /^(?:[A-Za-z_][A-Za-z0-9_]*\/)*[A-Za-z_][A-Za-z0-9_]*\.lean$/;
export const moduleFiles = z
  .record(z.string().regex(MODULE_PATH).max(500), z.string().max(2_000_000))
  .refine((f) => Object.keys(f).length >= 1 && Object.keys(f).length <= 64, {
    message: "1 to 64 files",
  });
export const commandSchema = z.discriminatedUnion("type", [
  z
    .object({
      type: z.literal("members"),
      members: z.array(z.string().regex(/^[A-Za-z0-9_-]+$/)).max(100),
    })
    .strict(),
  z
    .object({
      type: z.literal("resolve"),
      goal: short,
      proposition: z.string().min(1).max(100000),
      definitions: z.array(short).max(1000).default([]),
    })
    .strict(),
  z
    .object({ type: z.literal("notes"), notes: z.string().max(100000) })
    .strict(),
  z
    .object({
      type: z.literal("task"),
      goal: short,
      kind: z.enum(["prove", "decompose", "formalize"]),
      priority: z.number().int().min(-100).max(100).default(0),
    })
    .strict(),
  z
    .object({
      type: z.literal("claim"),
      task: short,
      allocation: z.number().int().min(0).max(1e12),
      ttl,
    })
    .strict(),
  z
    .object({
      type: z.literal("heartbeat"),
      attempt: short,
      epoch: z.number().int().min(1),
      ttl,
    })
    .strict(),
  z
    .object({
      type: z.literal("finish"),
      attempt: short,
      epoch: z.number().int().min(1),
      spent: z.number().int().min(0).max(1e12),
      outcome: z.enum([
        "no_progress",
        "submitted",
        "execution_failed",
        "released",
      ]),
      // What was learned, kept on the task's goal (§2.7.3).
      note: z.string().trim().min(1).max(NOTE_LENGTH).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal("note"),
      goal: short,
      text: z.string().trim().min(1).max(NOTE_LENGTH),
    })
    .strict(),
  // A note is named by its author and time, which no two notes on a goal share.
  z
    .object({
      type: z.literal("edit_note"),
      goal: short,
      note: noteKey,
      text: z.string().trim().min(1).max(NOTE_LENGTH),
    })
    .strict(),
  z
    .object({
      type: z.literal("delete_note"),
      goal: short,
      note: noteKey,
    })
    .strict(),
  z
    .object({
      type: z.literal("negate"),
      goal: short,
    })
    .strict(),
  z
    .object({
      type: z.literal("policy"),
      review: z.enum(["required", "none"]),
    })
    .strict(),
  z
    .object({
      type: z.literal("decomposition"),
      certificate: short,
      // Reviewing a proposed decomposition is setting it active or retired (§2.7.3).
      state: z.enum(["active", "paused", "retired"]),
      comment: z.string().trim().min(1).max(2000).optional(),
    })
    .strict(),
  z.object({ type: z.literal("sync") }).strict(),
  z.object({ type: z.literal("import"), certificate: short }).strict(),
  z
    .object({
      type: z.literal("submit"),
      // Omitted only when the conclusion uses a definition registered by this submission.
      goal: short.optional(),
      source: z.string().min(1).max(2000000),
      expected_root: short,
      component: z.enum(["lean-worker", "lean-worker-sketch"]),
      mode: z.enum(["leaf", "sketch"]),
      definitions: z.array(short).max(1000).default([]),
      new_definitions: z.array(short).max(100).default([]),
      // A decomposition's purpose, kept on the certificate for review (§2.7.2).
      description: z.string().trim().min(1).max(2000).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal("submit_modules"),
      goal: short,
      files: moduleFiles,
      root: short,
      // Shared modules the files import; `new_shared` files are registered as one more first.
      shared: z.array(short).max(100).default([]),
      new_shared: moduleFiles.optional(),
      mode: z.enum(["leaf", "sketch"]),
      description: z.string().trim().min(1).max(2000).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal("submit_node"),
      node: z.string().uuid(),
      branch: branchName.optional(),
    })
    .strict(),
  z.object({ type: z.literal("attach"), branch: branchName }).strict(),
  z
    .object({
      type: z.literal("pick"),
      branch: branchName,
      goal: short,
      certificate: short.nullable(),
    })
    .strict(),
  z
    .object({
      type: z.literal("assemble"),
      component: short,
      branch: branchName.optional(),
    })
    .strict(),
  z.object({ type: z.literal("retry"), run: short }).strict(),
  z.object({ type: z.literal("accept"), run: short }).strict(),
]);
export type Command = z.infer<typeof commandSchema>;
/** Commands checked against the current state rather than the revision the client read. */
export const UNCHECKED = new Set<Command["type"]>([
  "claim",
  "heartbeat",
  "finish",
  "submit",
  "submit_modules",
  "resolve",
  "import",
  "sync",
  "submit_node",
  "note",
  "edit_note",
  "delete_note",
  "negate",
]);
/** What clients send: fields with defaults may be omitted. */
export type CommandInput = z.input<typeof commandSchema>;
export interface JobInput {
  kind: string;
  payload: Record<string, unknown>;
}
/** Rights the HTTP layer established from document roles (docs/mathdoc.md §4.3). */
export interface Grants {
  /** The actor may write the branch the command names (§2.7.7: editors pick their route). */
  editsBranch?: boolean;
  /** The actor's token may run owner commands (`maintain`): an owner removes others' notes and releases their leases. */
  maintains?: boolean;
}
export function command(
  b: Board,
  actor: string,
  c: Command,
  now: number,
  grants: Grants = {},
): { result: unknown; job?: JobInput } {
  member(b, actor);
  const frozen = readOnly(b);
  requireThat(!frozen, frozen!);
  expire(b, now);
  requireThat(b.ready || c.type === "sync", "project_initializing");
  switch (c.type) {
    case "members": {
      owner(b, actor);
      b.members = [...new Set([b.owner, ...c.members])];
      b.ready = false;
      for (const a of Object.values(b.attempts))
        // A delegated token keeps its lease while its person stays a member.
        if (!a.outcome && !b.members.includes(principalOf(a.actor))) {
          a.outcome = "revoked";
          a.spent = a.reserved;
          b.spent += a.spent;
          const t = b.tasks[a.task];
          if (t.attempt === a.id) {
            t.state = "ready";
            delete t.attempt;
          }
        }
      return {
        result: { queued: true },
        job: { kind: "provision", payload: {} },
      };
    }
    case "resolve":
      requireThat(b.mode !== "modules", "not_in_module_mode");
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      return {
        result: { queued: true },
        job: {
          kind: "resolve",
          payload: {
            goal: c.goal,
            proposition: c.proposition,
            definitions: c.definitions,
          },
        },
      };
    case "notes":
      b.notes = c.notes;
      return { result: { saved: true } };
    case "note":
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      return { result: addNote(b, c.goal, { actor, at: now, text: c.text }) };
    case "edit_note":
    case "delete_note": {
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      const notes = b.goals[c.goal].notes ?? [];
      const i = notes.findIndex(
        (n) => n.actor === c.note.actor && n.at === c.note.at,
      );
      requireThat(i >= 0, "unknown_note", 404);
      // Authors change their notes; a person also those of their delegated tokens.
      const n = notes[i];
      const own =
        n.actor === actor ||
        (!actor.includes("/") && principalOf(n.actor) === actor);
      if (c.type === "edit_note") {
        requireThat(own, "note_author_required", 403);
        notes[i] = { ...n, text: c.text, edited: now };
        return { result: notes[i] };
      }
      // The owner also removes a note that misleads, e.g. one written by mistake.
      requireThat(
        own || (b.owner === principalOf(actor) && grants.maintains),
        "note_author_required",
        403,
      );
      notes.splice(i, 1);
      return { result: { deleted: true } };
    }
    case "negate": {
      // §2.7.5: the negation becomes a goal of its own; proving it refutes this one. A module
      // goal's negation is stated at the end of its statement file (docs/mathdoc.md §4.13).
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      const g = b.goals[c.goal];
      const known = Object.entries(b.negations ?? {}).find(
        ([, t]) => t === c.goal,
      );
      if (known) return { result: { goal: c.goal, negation: known[0] } };
      requireThat(
        g.identity && (b.mode === "modules" ? g.module : g.proposition),
        "goal_unresolved",
      );
      requireThat(!g.refuted, "already_refuted");
      return {
        result: { queued: true },
        job: { kind: "negate", payload: { goal: c.goal } },
      };
    }
    case "policy":
      owner(b, actor);
      requireThat(b.project, "not_a_project_board", 400);
      b.review = c.review;
      return { result: { review: b.review } };
    case "task": {
      owner(b, actor);
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      requireThat(
        !Object.values(b.tasks).some(
          (t) =>
            t.goal === c.goal && t.kind === c.kind && t.state !== "cancelled",
        ),
        "duplicate_task",
      );
      const id = randomUUID();
      b.tasks[id] = {
        id,
        goal: c.goal,
        kind: c.kind,
        priority: c.priority,
        state: "ready",
        epoch: 0,
      };
      return { result: b.tasks[id] };
    }
    case "claim": {
      const t = Object.hasOwn(b.tasks, c.task) ? b.tasks[c.task] : undefined;
      requireThat(t, "not_found", 404);
      const reserved = Object.values(b.attempts)
        .filter((a) => !a.outcome)
        .reduce((s, a) => s + a.reserved, 0);
      const a = takeLease(t, actor, c, now, {
        active: activeGoals(b).has(t.goal),
        certified: b.goals[t.goal].status === "certified",
        committed: b.spent + reserved,
        budget: b.budget,
      });
      b.attempts[a.id] = a;
      return { result: a };
    }
    case "heartbeat":
    case "finish": {
      const a = Object.hasOwn(b.attempts, c.attempt)
        ? b.attempts[c.attempt]
        : undefined;
      requireThat(a, "not_found", 404);
      const t = b.tasks[a.task];
      if (c.type === "heartbeat") renewLease(a, t, actor, c, now);
      else {
        // A session that stopped without `mdc done` leaves its lease until it expires (an hour by
        // default). Besides its holder, the person behind the token, or the owner with
        // `maintain`, may release it, as they may remove a note; only a release spending
        // nothing, so nobody reports another's spending or outcome.
        const releaser =
          actor !== a.actor &&
          c.outcome === "released" &&
          c.spent === 0 &&
          ((!actor.includes("/") && principalOf(a.actor) === actor) ||
            (b.owner === principalOf(actor) && grants.maintains === true));
        endLease(a, t, releaser ? a.actor : actor, c, now);
        b.spent += a.spent;
        if (c.note)
          addNote(b, b.tasks[a.task].goal, {
            actor,
            at: now,
            text: c.note,
            attempt: a.id,
          });
      }
      return { result: a };
    }
    case "decomposition": {
      owner(b, actor);
      requireThat(Object.hasOwn(b.facts, c.certificate), "not_found", 404);
      const f = b.facts[c.certificate];
      // A false premise stays false.
      requireThat(f.state !== "refuted", "certificate_refuted");
      f.state = c.state;
      f.review = {
        actor,
        at: now,
        state: c.state,
        ...(c.comment ? { comment: c.comment } : {}),
      };
      return { result: f };
    }
    case "sync":
      return {
        result: { queued: true },
        job: { kind: b.ready ? "sync" : "provision", payload: {} },
      };
    case "import":
      return {
        result: { queued: true },
        job: { kind: "import", payload: { certificate: c.certificate } },
      };
    case "submit":
      // A module project takes whole files (`submit_modules`).
      requireThat(b.mode !== "modules", "not_in_module_mode");
      // Components and modes are paired: sketches only through lean-worker-sketch.
      requireThat(
        (c.component === "lean-worker-sketch") === (c.mode === "sketch"),
        "component_mode_mismatch",
        400,
      );
      if (c.goal === undefined)
        requireThat(c.new_definitions.length, "goal_required", 400);
      else
        requireThat(
          Object.hasOwn(b.goals, c.goal) && b.goals[c.goal].identity,
          "goal_identity_unavailable",
        );
      return {
        result: { queued: true },
        job: { kind: "submit", payload: { ...c } },
      };
    case "submit_modules":
      requireThat(b.mode === "modules", "not_a_module_project");
      requireThat(
        Object.hasOwn(b.goals, c.goal) && b.goals[c.goal].identity,
        "goal_identity_unavailable",
      );
      return {
        result: { queued: true },
        job: { kind: "submit_modules", payload: { ...c } },
      };
    case "submit_node": {
      // A module project's nodes are files; contributions come from `mdc prove` / `mdc split`.
      requireThat(b.mode !== "modules", "not_in_module_mode");
      const branch = attached(b, c.branch);
      requireThat(branch.request.bound, "branch_not_bound");
      const n = Object.hasOwn(branch.nodes, c.node)
        ? branch.nodes[c.node]
        : undefined;
      requireThat(n, "not_found", 404);
      requireThat(n.role === "theorem", "not_a_theorem_node");
      return {
        result: { queued: true },
        job: {
          kind: "submit_node",
          payload: { node: c.node, branch: c.branch },
        },
      };
    }
    case "attach": {
      // Document roles are checked by the HTTP layer; re-attaching converts the branch again.
      requireThat(b.project, "not_a_project_board", 400);
      b.branches ??= {};
      if (!Object.hasOwn(b.branches, c.branch))
        b.branches[c.branch] = newBranchState(b, c.branch);
      return {
        result: { queued: true },
        job: { kind: "attach", payload: { branch: c.branch } },
      };
    }
    case "pick": {
      // The owner, or a member who may write the branch: its route is part of its content.
      if (!grants.editsBranch) owner(b, actor);
      const branch = attached(b, c.branch);
      requireThat(Object.hasOwn(b.goals, c.goal), "unknown_goal");
      if (c.certificate === null) delete branch.picks[c.goal];
      else {
        const f = Object.hasOwn(b.facts, c.certificate)
          ? b.facts[c.certificate]
          : undefined;
        requireThat(f, "not_found", 404);
        requireThat(f.goal === c.goal, "certificate_goal_mismatch");
        branch.picks[c.goal] = c.certificate;
      }
      return { result: branch.picks };
    }
    case "assemble": {
      owner(b, actor);
      requireThat(
        !Object.values(b.runs).some(
          (r) => r.status === "queued" || r.status === "running",
        ),
        "assembly_in_progress",
      );
      let picks: Record<string, string> = {};
      if (b.project) {
        const branch = attached(b, c.branch);
        requireThat(branch.request.bound, "branch_not_bound");
        picks = branch.picks;
      }
      const id = randomUUID();
      const run: Run = {
        id,
        route: selectRoute(b, picks),
        // Module certificates are assembled at the constant level by their own component.
        component: b.mode === "modules" ? "lean-modules" : c.component,
        status: "queued",
        ...(b.project ? { branch: c.branch } : {}),
      };
      b.runs[id] = run;
      return { result: run, job: { kind: "assemble", payload: { run: id } } };
    }
    case "retry": {
      owner(b, actor);
      const old = b.runs[c.run];
      requireThat(old?.status === "failed" && old.retryable, "not_retryable");
      requireThat(
        !Object.values(b.runs).some(
          (r) => r.status === "queued" || r.status === "running",
        ),
        "assembly_in_progress",
      );
      const id = randomUUID();
      b.runs[id] = {
        ...old,
        id,
        status: "queued",
        failure: undefined,
        certification_id: undefined,
      };
      return {
        result: b.runs[id],
        job: { kind: "assemble", payload: { run: id } },
      };
    }
    case "accept":
      owner(b, actor);
      requireThat(b.runs[c.run]?.status === "certified", "not_certified");
      if (b.project) {
        // docs/mathdoc.md §4.9: a changed statement keeps old results in history but never certifies
        // the new version. Writeback re-checks every node before committing.
        requireThat(
          !attached(b, b.runs[c.run].branch).request.stale,
          "proof_request_stale",
        );
        return {
          result: { queued: true },
          job: { kind: "writeback", payload: { run: c.run } },
        };
      }
      requireThat(b.node_ref, "no_document_binding");
      return {
        result: { queued: true },
        job: { kind: "accept", payload: { run: c.run } },
      };
  }
}
