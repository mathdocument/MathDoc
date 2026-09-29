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
/** A project bound to a document node (a ProofRequest); the environment comes from the branch. */
export const documentCreateSchema = z
  .object({
    title: z.string().trim().min(1).max(200).optional(),
    document: z
      .object({
        database: z.string().regex(/^[\w-]+$/),
        branch: z.string().regex(/^[\w-]+$/),
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
  statement: string;
  status: "insufficient" | "derivable" | "certified";
}
export interface Fact {
  id: string;
  goal: string;
  premises: string[];
  trust: Trust;
  state: "active" | "paused" | "retired";
  available: boolean;
}
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
  plan_id?: string;
  status: "queued" | "running" | "failed" | "certified";
  certification_id?: string;
  failure?: string;
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
  accepted?: { certification_id: string; revision: string; operation: string };
  /** Document-bound projects (stage 4): the ProofRequest, its nodes and writebacks. */
  request?: ProofRequest;
  nodes?: Record<string, NodeBinding>;
  writebacks?: Record<string, Writeback>;
}
export function member(b: Board, actor: string) {
  requireThat(b.members.includes(actor), "not_found", 404);
}
export function owner(b: Board, actor: string) {
  member(b, actor);
  requireThat(b.owner === actor, "owner_required", 403);
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
export function newDocumentBoard(
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
  },
): Board {
  return {
    id,
    title: input.title ?? title,
    owner: actor,
    members: [...new Set([actor, ...input.members])],
    revision: 0,
    base: env.base,
    context: env.context,
    options: env.options,
    // The root goal is resolved when the worker binds the request.
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
    request: {
      ...input.document,
      environment_id: env.environment_id,
      ...(env.replaces_environment_id
        ? { replaces_environment_id: env.replaces_environment_id }
        : {}),
      bound: false,
      stale: false,
    },
    nodes: {},
    writebacks: {},
  };
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
export function ingest(b: Board, raw: unknown) {
  const f = factSchema.parse(raw);
  requireThat(
    f.access.visibility === "public" ||
      (f.access.visibility === "project" && f.access.project_id === b.id),
    "fact_not_shared",
  );
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
  addGoal(b, f.goal, f.request.goal);
  for (const key of f.premises) addGoal(b, key);
  requireThat(
    !["__proto__", "constructor", "prototype"].includes(f.id),
    "invalid_fact",
  );
  b.facts[f.id] = {
    id: f.id,
    goal: f.goal,
    premises: [...new Set(f.premises)],
    trust: f.trust,
    state: b.facts[f.id]?.state ?? "active",
    available: true,
  };
  recompute(b);
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
export function activeGoals(b: Board) {
  const active = new Set([b.root]);
  let before = -1;
  while (before !== active.size) {
    before = active.size;
    for (const f of Object.values(b.facts))
      if (f.available && f.state === "active" && active.has(f.goal))
        for (const p of f.premises) active.add(p);
  }
  return active;
}
export function selectRoute(b: Board): Route {
  const solved = new Map<string, string>();
  const steps: Step[] = [];
  let before = -1;
  const facts = Object.values(b.facts)
    .filter((f) => f.available && f.state === "active")
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
      // A disconnected participant may have spent all of the reservation.
      a.outcome = "expired";
      a.spent = a.reserved;
      b.spent += a.spent;
      const t = b.tasks[a.task];
      if (t.attempt === a.id) {
        t.state = "ready";
        delete t.attempt;
      }
    }
}
const short = z.string().min(1).max(200);
const ttl = z.number().int().min(30).max(3600).default(300);
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
    })
    .strict(),
  z
    .object({
      type: z.literal("decomposition"),
      certificate: short,
      state: z.enum(["active", "paused", "retired"]),
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
    })
    .strict(),
  z
    .object({ type: z.literal("submit_node"), node: z.string().uuid() })
    .strict(),
  z.object({ type: z.literal("assemble"), component: short }).strict(),
  z.object({ type: z.literal("retry"), run: short }).strict(),
  z.object({ type: z.literal("accept"), run: short }).strict(),
]);
export type Command = z.infer<typeof commandSchema>;
/** What clients send: fields with defaults may be omitted. */
export type CommandInput = z.input<typeof commandSchema>;
export interface JobInput {
  kind: string;
  payload: Record<string, unknown>;
}
export function command(
  b: Board,
  actor: string,
  c: Command,
  now: number,
): { result: unknown; job?: JobInput } {
  member(b, actor);
  expire(b, now);
  requireThat(b.ready || c.type === "sync", "project_initializing");
  switch (c.type) {
    case "members": {
      owner(b, actor);
      b.members = [...new Set([b.owner, ...c.members])];
      b.ready = false;
      for (const a of Object.values(b.attempts))
        if (!a.outcome && !b.members.includes(a.actor)) {
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
      requireThat(t.state === "ready", "task_unavailable");
      requireThat(activeGoals(b).has(t.goal), "inactive_goal");
      requireThat(b.goals[t.goal].status !== "certified", "already_certified");
      const reserved = Object.values(b.attempts)
        .filter((a) => !a.outcome)
        .reduce((s, a) => s + a.reserved, 0);
      requireThat(
        b.spent + reserved + c.allocation <= b.budget,
        "budget_exhausted",
      );
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
      b.attempts[a.id] = a;
      return { result: a };
    }
    case "heartbeat":
    case "finish": {
      const a = Object.hasOwn(b.attempts, c.attempt)
        ? b.attempts[c.attempt]
        : undefined;
      requireThat(a, "not_found", 404);
      requireThat(
        a.actor === actor &&
          a.epoch === c.epoch &&
          !a.outcome &&
          a.expires > now &&
          b.tasks[a.task].attempt === a.id,
        "stale_lease",
      );
      if (c.type === "heartbeat") a.expires = now + c.ttl * 1000;
      else {
        requireThat(c.spent <= a.reserved, "allocation_exceeded");
        a.spent = c.spent;
        a.outcome = c.outcome;
        b.spent += a.spent;
        b.tasks[a.task].state = "ready";
        delete b.tasks[a.task].attempt;
      }
      return { result: a };
    }
    case "decomposition":
      owner(b, actor);
      requireThat(Object.hasOwn(b.facts, c.certificate), "not_found", 404);
      b.facts[c.certificate].state = c.state;
      return { result: b.facts[c.certificate] };
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
    case "submit_node": {
      requireThat(b.request?.bound, "no_document_binding");
      const n = b.nodes?.[c.node];
      requireThat(n, "not_found", 404);
      requireThat(n.role === "theorem", "not_a_theorem_node");
      return {
        result: { queued: true },
        job: { kind: "submit_node", payload: { node: c.node } },
      };
    }
    case "assemble": {
      owner(b, actor);
      requireThat(
        !Object.values(b.runs).some(
          (r) => r.status === "queued" || r.status === "running",
        ),
        "assembly_in_progress",
      );
      const id = randomUUID();
      const run: Run = {
        id,
        route: selectRoute(b),
        component: c.component,
        status: "queued",
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
      if (b.request) {
        // Plan §9: a changed statement keeps old results in history but never certifies
        // the new version. Writeback re-checks every node before committing.
        requireThat(!b.request.stale, "proof_request_stale");
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
