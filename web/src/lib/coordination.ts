// Collaboration API (docs/mathdoc.md §4.3): the same /api/coordination endpoints serve people and
// agents. Commands carry the project revision (If-Match) and an idempotency key.
import { ApiError, fetchJson } from "./api";
import { projectName } from "./project-path";

export type Trust = "claimed" | "audited" | "trusted";
/** `modules`: every Lean node is a whole Lean file (docs/mathdoc.md §4.13). */
export type ProofMode = "declarations" | "modules";
export interface EnvironmentInput {
  base_key: number;
  context: Record<string, string[]>;
  options: Record<string, string | number | boolean>;
  minimum_trust: Trust;
  /** Absent in environments stored before modes existed: declarations. */
  mode?: ProofMode;
}
export interface Environment extends EnvironmentInput {
  base: Record<string, unknown>;
  environment_id: string;
  replaces_environment_id?: string;
  updated_by: string;
}
export interface Note { actor: string; at: number; text: string; attempt?: string; edited?: number }
export interface Goal {
  key: string; statement: string; status: "insufficient" | "derivable" | "certified"; identity?: unknown;
  /** How the goal is stated (docs/mathdoc.md §4.11). */
  proposition?: string; declaration?: { name: string; binders: string; type: string };
  /** Attempt notes, oldest first (§2.7.3). */
  notes?: Note[];
  /** Set once a complete proof of the goal's negation exists (§2.7.5). */
  refuted?: { certificate: string; at?: number };
  /** Module projects: the constant stating the goal, in the root node or a decomposition. */
  module?: { const: string; shared: string[]; node?: string; path?: string; certificate?: string };
}
/** `proposed` decompositions wait for the owner's review and schedule nothing (§2.7.3). */
export type FactState = "proposed" | "active" | "paused" | "retired" | "refuted";
export interface Fact {
  id: string; goal: string; premises: string[]; trust: Trust; state: FactState; available: boolean;
  description?: string; submitter?: string; submitted_at?: number;
  review?: { actor: string; at: number; state: Exclude<FactState, "proposed">; comment?: string };
  retired_reason?: "premise_is_goal";
  /** For a refuted certificate: its premise proved false. */
  refuted_by?: string;
}
export interface Task { id: string; goal: string; kind: "prove" | "decompose" | "formalize"; priority: number; state: "ready" | "leased" | "cancelled"; epoch: number; attempt?: string }
export interface Attempt { id: string; task: string; actor: string; epoch: number; expires: number; reserved: number; spent: number; outcome?: string }
export interface Run {
  id: string; component: string; plan_id?: string; status: "queued" | "running" | "failed" | "certified";
  /** Project boards: the branch whose route was assembled and which receives the writeback. */
  branch?: string;
  certification_id?: string; failure?: string; retryable?: boolean;
  /** LeanGround's explanation of the failure, e.g. Lean's messages. */
  details?: string;
  route: { root: string; steps: { certificate_id: string; providers: Record<string, string> }[] };
}
export interface NodeBinding {
  node: string; title: string; role: "definition" | "theorem" | "unsupported";
  state: "open" | "submitted" | "registered" | "rejected" | "pending" | "certified";
  goal?: string; conclusion?: string; premises?: string[]; definition_ids?: string[];
  certificate?: string; reason?: string; details?: string;
  /** Module projects: a file a writeback created (docs/mathdoc.md §4.13). */
  file?: { path: string; certificate?: string; root?: boolean; shared?: string };
}
export interface Writeback {
  batch_id: string; run: string; status: "committed" | "aborted"; version?: string;
  reason?: string; details?: string; created: string[]; updated: string[];
  contract?: { operations: { kind: string; node_id: string; certification_id?: string; definition_id?: string }[] };
}
/** One change accepting an assembly would make, with the node's Lean text before and after. */
export interface PlannedChange {
  kind: "update_lean_block" | "create_goal_node" | "create_definition_node";
  node: string; title: string; before: string | null; after: string;
}
/** What accepting an assembly writes (§4.9): the batch and the assembled source; or why it would abort; or the batch written. */
export type WritebackPreview =
  | { status: "ready"; assembled: string | null; version: string; changes: PlannedChange[] }
  | { status: "conflict"; assembled: string | null; version: string; reason: string; details: string }
  | { status: "written"; writeback: Writeback };
/** A certificate as LeanGround keeps it, with the definitions it uses; sketches lose new definitions to them. */
export interface CertificateDetail {
  id: string; goal: string; premises: string[]; source?: string;
  definition_sources: { id: string; name: string; source: string }[];
}
export interface ProofRequest {
  database: string; branch: string; node: string; environment_id: string; bound: boolean;
  stale: boolean; identity?: string; error?: string; checked_version?: string;
}
/** One branch attached to a project board. */
export interface BranchState {
  request: ProofRequest;
  nodes: Record<string, NodeBinding>;
  /** goal → certificate this branch's route uses. */
  picks: Record<string, string>;
  writebacks: Record<string, Writeback>;
}
export interface Board {
  id: string; title: string; owner: string; members: string[]; revision: number;
  root: string; minimum_trust: Trust; notes: string; budget: number; spent: number;
  ready: boolean; last_sync?: number; sync_error?: string;
  goals: Record<string, Goal>; facts: Record<string, Fact>; tasks: Record<string, Task>;
  attempts: Record<string, Attempt>; runs: Record<string, Run>;
  /** A database's project: one root theorem, branches as routes over shared goals. */
  project?: { database: string; environment_id: string; root_node: string; superseded?: boolean };
  /** Project boards: whether new sketches wait for review (absent: they do). */
  review?: "required" | "none";
  /** Module projects: Lean nodes are whole files; contributions are sets of files. */
  mode?: "modules";
  shared_modules?: Record<string, { id: string; modules: string[]; nodes?: string[]; certificate?: string }>;
  /** Negation goal → the goal it refutes once proved (§2.7.5). */
  negations?: Record<string, string>;
  /** Branches the reader can read. */
  branches?: Record<string, BranchState>;
  /** Stage 4 per-branch proof requests (read-only). */
  request?: ProofRequest;
  nodes?: Record<string, NodeBinding>;
  writebacks?: Record<string, Writeback>;
}
type CommandReply = { revision: number; result: unknown; job?: string };
/** Commands the coordinator checks against the current board (coordinator/src/domain.ts UNCHECKED). */
const UNCHECKED = new Set(["claim", "heartbeat", "finish", "submit", "submit_modules", "resolve", "import", "sync", "submit_node", "note", "negate"]);
export type Command =
  | { type: "sync" }
  | { type: "claim"; task: string; allocation: number; ttl: number }
  | { type: "heartbeat"; attempt: string; epoch: number; ttl: number }
  | { type: "finish"; attempt: string; epoch: number; spent: number; outcome: "no_progress" | "submitted" | "execution_failed" | "released"; note?: string }
  | { type: "task"; goal: string; kind: "prove" | "decompose" | "formalize"; priority: number }
  | { type: "decomposition"; certificate: string; state: "active" | "paused" | "retired"; comment?: string }
  | { type: "note"; goal: string; text: string }
  // A note is named by its author and time.
  | { type: "edit_note"; goal: string; note: { actor: string; at: number }; text: string }
  | { type: "delete_note"; goal: string; note: { actor: string; at: number } }
  | { type: "negate"; goal: string }
  | { type: "policy"; review: "required" | "none" }
  | { type: "attach"; branch: string }
  | { type: "pick"; branch: string; goal: string; certificate: string | null }
  | { type: "submit_node"; node: string; branch?: string }
  | { type: "assemble"; component: "lean-worker"; branch?: string }
  | { type: "retry"; run: string }
  | { type: "accept"; run: string }
  | { type: "notes"; notes: string };

const base = "/api/coordination";
const json = (method: string, body: unknown, headers: Record<string, string> = {}) => ({
  method, headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body),
});
export function currentBranch(): { database: string; branch: string } | null {
  const name = projectName();
  if (!name) return null;
  const [database, branch] = name.split("/");
  return { database, branch };
}

export const coordination = {
  /** `actor` is `person/name` for a delegated token, whose `scopes` limit it (§2.7.4). */
  me: () =>
    fetchJson<{ actor: string; principal?: string; admin: boolean; scopes?: string[] | null }>("/api/me"),
  environment: (database: string) =>
    fetchJson<Environment>(`${base}/environments/${database}`),
  putEnvironment: (database: string, input: EnvironmentInput) =>
    fetchJson<Environment>(`${base}/environments/${database}`, json("PUT", input)),
  projects: (database: string, branch: string, node?: string) =>
    fetchJson<Board[]>(`${base}/projects?${new URLSearchParams({ database, branch, ...(node ? { node } : {}) })}`),
  project: (id: string) => fetchJson<Board>(`${base}/projects/${encodeURIComponent(id)}`),
  create: (input: { document: { database: string; branch: string; node: string }; members: string[]; budget: number; title?: string }) =>
    fetchJson<Board>(`${base}/projects`, json("POST", input, { "idempotency-key": crypto.randomUUID() })),
  /**
   * Send one command. Commands checked against the current board go with `If-Match: *`; the
   * others carry the revision shown, and when it is outdated (a job wrote the board meanwhile)
   * they are sent again at the latest revision, as `mdc` does (docs/mathdoc.md §2.7.7).
   */
  command: async (board: Pick<Board, "id" | "revision">, command: Command) => {
    const path = `${base}/projects/${encodeURIComponent(board.id)}/commands`;
    const key = crypto.randomUUID();
    if (UNCHECKED.has(command.type))
      return fetchJson<CommandReply>(path, json("POST", command, { "if-match": "*", "idempotency-key": key }));
    let revision = board.revision;
    for (let attempt = 0; ; attempt++) {
      try {
        return await fetchJson<CommandReply>(path, json("POST", command, { "if-match": `"${revision}"`, "idempotency-key": key }));
      } catch (e) {
        // A conflicting revision means the command did not run.
        if (!(e instanceof ApiError && e.message === "revision_conflict" && attempt < 3)) throw e;
        revision = (await fetchJson<Board>(`${base}/projects/${encodeURIComponent(board.id)}`)).revision;
      }
    }
  },
  /** The goals most worth working on, best first (§2.7.5). */
  next: (id: string, limit = 200) =>
    fetchJson<{ revision: number; candidates: { goal: string; task: string; score: number; branches: string[]; failures: number }[] }>(
      `${base}/projects/${encodeURIComponent(id)}/next?limit=${limit}`,
    ),
  /** A certificate with its source and the definitions it uses (for review). */
  fact: (id: string, certificate: string) =>
    fetchJson<CertificateDetail>(`${base}/projects/${encodeURIComponent(id)}/facts/${encodeURIComponent(certificate)}`),
  preview: (id: string, run: string) =>
    fetchJson<WritebackPreview>(`${base}/projects/${encodeURIComponent(id)}/runs/${encodeURIComponent(run)}/preview`),
  jobs: (id: string) =>
    fetchJson<{ id: string; actor: string; kind: string; status: string; tries: number; error: string | null; details: string | null; created_at: string }[]>(
      `${base}/projects/${encodeURIComponent(id)}/jobs`,
    ),
};

/** A branch's view of a board: its own binding on a project board, the request itself on a stage 4 board. */
export function branchView(board: Board, branch: string): BranchState | null {
  if (board.project) return board.branches?.[branch] ?? null;
  if (!board.request) return null;
  return { request: board.request, nodes: board.nodes ?? {}, picks: {}, writebacks: board.writebacks ?? {} };
}
/**
 * The certificates an assembly of a branch would use, chosen as the coordinator's selectRoute
 * (coordinator/src/domain.ts) does: active facts, a picked goal only through its pick, fewest
 * premises first, then by ID. Null while the route does not reach complete proofs.
 */
export function branchRoute(board: Board, picks: Record<string, string>): Set<string> | null {
  const facts = Object.values(board.facts)
    .filter((f) => f.available && f.state === "active" && (!Object.hasOwn(picks, f.goal) || picks[f.goal] === f.id))
    .sort((a, c) => a.premises.length - c.premises.length || a.id.localeCompare(c.id));
  const solved = new Map<string, Fact>();
  for (let before = -1; before !== solved.size; ) {
    before = solved.size;
    for (const f of facts)
      if (!solved.has(f.goal) && f.premises.every((p) => solved.has(p))) solved.set(f.goal, f);
  }
  if (!solved.has(board.root)) return null;
  // Only the certificates the root's proof reaches.
  const used = new Set<string>();
  const visit = (goal: string) => {
    const f = solved.get(goal)!;
    if (used.has(f.id)) return;
    used.add(f.id);
    f.premises.forEach(visit);
  };
  visit(board.root);
  return used;
}
/** The way a branch proves its root, chosen goal by goal: what `activeRoute` returns. */
export interface ActiveRoute {
  /** For each goal reached, the certificate it uses (absent when it has none to use). */
  choice: Map<string, string>;
  /** The goals and certificates reached from the root through the choices. */
  goals: Set<string>; facts: Set<string>;
  /** Goals reached whose chosen certificate does not lead to complete proofs yet, in reach order. */
  open: string[];
  /** Whether the root is proved through the choices: an assembly can run. */
  closes: boolean;
}
/**
 * The route a branch takes through the AND-OR graph, with `draft` choices over its picks (a
 * preview of other picks). A goal uses its draft choice, else its pick, else, as an assembly
 * would (`branchRoute`), the certificate that proves it with the fewest premises, else its
 * first active certificate (fewest premises first), which shows what is still missing. Only
 * active certificates are chosen.
 */
export function activeRoute(board: Board, picks: Record<string, string>, draft: Record<string, string> = {}): ActiveRoute {
  const usable = (id: string | undefined) => {
    const f = id ? board.facts[id] : undefined;
    return f && f.available && f.state === "active" ? f : undefined;
  };
  const fixed = new Map<string, string>();
  for (const [goal, id] of [...Object.entries(picks), ...Object.entries(draft)]) if (usable(id)?.goal === goal) fixed.set(goal, id);
  const facts = Object.values(board.facts)
    .filter((f) => usable(f.id) && (!fixed.has(f.goal) || fixed.get(f.goal) === f.id))
    .sort((a, c) => a.premises.length - c.premises.length || a.id.localeCompare(c.id));
  const solved = new Map<string, Fact>();
  for (let before = -1; before !== solved.size; ) {
    before = solved.size;
    for (const f of facts)
      if (!solved.has(f.goal) && f.premises.every((p) => solved.has(p))) solved.set(f.goal, f);
  }
  const first = new Map<string, string>();
  for (const f of facts) if (!first.has(f.goal)) first.set(f.goal, f.id);
  const route: ActiveRoute = { choice: new Map(), goals: new Set(), facts: new Set(), open: [], closes: solved.has(board.root) };
  const visit = (goal: string) => {
    if (route.goals.has(goal)) return;
    route.goals.add(goal);
    const id = fixed.get(goal) ?? solved.get(goal)?.id ?? first.get(goal);
    if (!solved.has(goal)) route.open.push(goal);
    if (!id) return;
    route.choice.set(goal, id);
    route.facts.add(id);
    board.facts[id].premises.forEach(visit);
  };
  visit(board.root);
  return route;
}
/**
 * Whether a branch's route reaches complete proofs. A goal's own status is shared by all
 * branches; this is the branch's.
 */
export function routeCloses(board: Board, picks: Record<string, string>): boolean {
  return branchRoute(board, picks) !== null;
}
/** The committed writeback of a run, if any: accepting the run again would write nothing. */
export function writtenBack(board: Board, run: Run): Writeback | null {
  const records = run.branch ? board.branches?.[run.branch]?.writebacks : board.writebacks;
  return Object.values(records ?? {}).find((w) => w.run === run.id && w.status === "committed") ?? null;
}
/** Why the board takes no commands, as shown to people; null when it does. */
export function readOnlyReason(board: Board): string | null {
  if (board.request)
    return "This per-branch proof request predates project boards and is read-only. Submit the root theorem to attach the branch to the database's project.";
  if (board.project?.superseded)
    return "The database's proof environment changed, so this project board is read-only. Submit the root theorem again to start a new one.";
  return null;
}
/** A readable name for a goal: a document node proving it, else its statement, else a short key. */
export function goalName(board: Board, goal: string): string {
  const views = [board.nodes ?? {}, ...Object.values(board.branches ?? {}).map((b) => b.nodes)];
  for (const nodes of views) {
    const node = Object.values(nodes).find((n) => n.goal === goal);
    if (node) return node.title;
  }
  return board.goals[goal]?.statement || `${goal.slice(0, 10)}...`;
}

/**
 * Goals whose tasks can be claimed: reachable from the root through active facts (the
 * coordinator's `activeGoals`). Premises of a proposed decomposition are not, until it is active.
 */
export function activeGoals(board: Board): Set<string> {
  const active = new Set([board.root]);
  for (let before = -1; before !== active.size; ) {
    before = active.size;
    for (const f of Object.values(board.facts))
      if (f.available && f.state === "active" && active.has(f.goal))
        for (const p of f.premises) active.add(p);
    // A goal's registered negation is worth working on too.
    for (const [negation, target] of Object.entries(board.negations ?? {}))
      if (active.has(target) && !board.goals[target]?.refuted) active.add(negation);
  }
  return active;
}
/**
 * The branches whose route can need a goal: reachable from the root through active facts,
 * where a picked goal follows only its pick. A review of a goal's decomposition matters to them.
 */
export function branchesNeeding(board: Board, goal: string): string[] {
  return Object.entries(board.branches ?? {})
    .filter(([, state]) => {
      const reach = new Set([board.root]);
      for (let before = -1; before !== reach.size; ) {
        before = reach.size;
        for (const f of Object.values(board.facts))
          if (
            f.available && f.state === "active" && reach.has(f.goal) &&
            (!Object.hasOwn(state.picks, f.goal) || state.picks[f.goal] === f.id)
          )
            for (const p of f.premises) reach.add(p);
      }
      return reach.has(goal);
    })
    .map(([name]) => name);
}
/** A goal as a statement: its declaration, else its proposition, else its name. */
export function goalStatement(board: Board, goal: string): string {
  const g = board.goals[goal];
  if (g?.declaration)
    return `${g.declaration.name}${g.declaration.binders ? ` ${g.declaration.binders}` : ""} : ${g.declaration.type}`;
  return g?.proposition ?? goalName(board, goal);
}
/** The statement laid out for reading: the declaration's name and binders, its type indented below. */
export function goalStatementBlock(board: Board, goal: string): string {
  const d = board.goals[goal]?.declaration;
  return d ? `${d.name}${d.binders ? ` ${d.binders}` : ""} :\n  ${d.type}` : goalStatement(board, goal);
}

/** Budget by participant: each agent's own identity, spent (closed attempts) and reserved (open). */
export function spentBy(board: Board): { actor: string; spent: number; reserved: number; attempts: number }[] {
  const out = new Map<string, { actor: string; spent: number; reserved: number; attempts: number }>();
  for (const a of Object.values(board.attempts)) {
    const row = out.get(a.actor) ?? { actor: a.actor, spent: 0, reserved: 0, attempts: 0 };
    row.attempts++;
    if (a.outcome) row.spent += a.spent;
    else row.reserved += a.reserved;
    out.set(a.actor, row);
  }
  return [...out.values()].sort((x, y) => y.spent - x.spent || x.actor.localeCompare(y.actor));
}

/** A goal status as the page shows it; the API's words stay in `data-status`. */
export const goalStatusLabel: Record<Goal["status"], string> = {
  certified: "proved",
  derivable: "provable by assembly",
  insufficient: "open",
};
/** What a goal status means, for people (coordinator/src/domain.ts `recompute`). */
export const goalStatusHelp: Record<Goal["status"], string> = {
  certified: "Proved: one certificate proves it with no premises left.",
  derivable: "Proved by combining certificates: a decomposition whose premises are proved. Assembling turns it into one complete proof.",
  insufficient: "Not proved yet: a goal it depends on still has no proof.",
};
/**
 * Why the server refused a command, for people. A server older than this page answers an
 * unknown command type with `invalid_request` on `type`: say so, since only a restart helps.
 */
export function refusal(e: unknown, command: Command): string {
  const body = e instanceof ApiError && e.status === 400 ? (e.body as { reason?: string; issues?: { path?: unknown[] }[] } | null) : null;
  if (body?.reason === "invalid_request" && body.issues?.some((i) => i.path?.[0] === "type"))
    return `The server runs an older version of MathDoc that does not know “${command.type}”. Restart it with this version, then try again.`;
  return e instanceof Error ? e.message : String(e);
}
/** A claim's remaining time, as coarse as it is read: seconds only in its last minute. */
export function timeLeft(expires: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((expires - now) / 1000));
  if (s < 60) return `${s} s left`;
  const m = Math.round(s / 60);
  return m < 60 ? `${m} min left` : `${Math.floor(m / 60)} h${m % 60 ? ` ${m % 60} min` : ""} left`;
}
/** What a converted node's state means, for people (coordinator/src/proof.ts, worker.ts). */
export const nodeStateHelp: Record<NodeBinding["state"], string> = {
  pending: "Converted, waiting to be submitted to LeanGround.",
  open: "Its proof is still sorry: the node is a goal waiting for a proof.",
  submitted: "LeanGround checked its proof and issued a certificate: a complete proof, or one from the premises it uses.",
  registered: "A definition registered with LeanGround, so statements can use it.",
  rejected: "Refused by the conversion or by LeanGround; the reason follows.",
  certified: "An accepted assembly wrote a certified proof into this node.",
};

/**
 * Goals proved through active certificates, as the coordinator's `rankGoals` counts them; with
 * `picks`, a picked goal only through its pick. A goal's status also counts certificates not
 * active (proposed, paused), which no route uses.
 */
export function solvedGoals(board: Board, picks: Record<string, string> = {}): Set<string> {
  const usable = Object.values(board.facts).filter(
    (f) => f.available && f.state === "active" && (!Object.hasOwn(picks, f.goal) || picks[f.goal] === f.id),
  );
  const solved = new Set<string>();
  for (let before = -1; before !== solved.size; ) {
    before = solved.size;
    for (const f of usable) if (f.premises.every((p) => solved.has(p))) solved.add(f.goal);
  }
  return solved;
}
/**
 * The goals a branch's proof still waits for: reachable from the root through active
 * certificates (a picked goal only through its pick), not proved that way, and with no active
 * certificate of their own. Where every unproved goal has one (a cycle), all unproved goals.
 */
export function openGoals(board: Board, picks: Record<string, string>): string[] {
  const usable = Object.values(board.facts).filter(
    (f) => f.available && f.state === "active" && (!Object.hasOwn(picks, f.goal) || picks[f.goal] === f.id),
  );
  const solved = solvedGoals(board, picks);
  const reach = new Set([board.root]);
  for (let before = -1; before !== reach.size; ) {
    before = reach.size;
    for (const f of usable) if (reach.has(f.goal) && !solved.has(f.goal)) for (const p of f.premises) reach.add(p);
  }
  const unproved = [...reach].filter((g) => !solved.has(g));
  const bare = unproved.filter((g) => !usable.some((f) => f.goal === g));
  return bare.length ? bare : unproved;
}

/** The collaboration dialog's views. */
export type CollabView = "overview" | "tasks" | "decompositions" | "review";
/** What a branch's proof waits for, in plain words, and the view that does it. */
export interface NextStep { tone: "done" | "todo" | "wait"; text: string; view?: CollabView }
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
/**
 * The next step on a project board's branch for the reader (`owner`: the board's owner).
 * Null where the dialog shows a warning instead: read-only boards, failed conversions or
 * syncs, and a branch whose document changed after it was converted.
 */
export function nextStep(board: Board, branch: string, owner: boolean): NextStep | null {
  if (!board.project || readOnlyReason(board)) return null;
  const view = board.branches?.[branch];
  if (!view) return { tone: "todo", text: "This branch is not attached to the project yet. Attach it to prove its document here.", view: "overview" };
  if (view.request.error || board.sync_error || view.request.stale) return null;
  if (!board.ready || !view.request.bound)
    return { tone: "wait", text: "Converting this branch's nodes and submitting them to LeanGround…" };
  if (board.goals[board.root]?.refuted)
    return { tone: "done", text: `Refuted: the negation of ${goalName(board, board.root)} has a complete proof, so the theorem is false.` };
  const last = Object.values(board.runs).filter((r) => r.branch === branch).at(-1);
  if (last?.status === "queued" || last?.status === "running")
    return { tone: "wait", text: "Assembling: LeanGround is checking the whole proof as one Lean file.", view: "review" };
  if (last?.status === "certified" && !writtenBack(board, last))
    return owner
      ? { tone: "todo", text: "The assembled proof is certified. Preview what it changes in the document, then accept it to write it back.", view: "review" }
      : { tone: "wait", text: "The assembled proof is certified and waits for the owner to write it back into the document.", view: "review" };
  if (last?.status === "certified")
    return { tone: "done", text: "Proved. LeanGround certified the whole proof as one Lean file, and it is written into the document." };
  if (last?.status === "failed")
    return { tone: "todo", text: `The last assembly failed (${last.failure ?? "no reason given"}). Read LeanGround's messages, then assemble again.`, view: "review" };
  if (routeCloses(board, view.picks))
    return owner
      ? { tone: "todo", text: `Every goal on ${branch}'s route has a proof. Assemble it: LeanGround checks the whole proof as one Lean file.`, view: "review" }
      : { tone: "wait", text: `Every goal on ${branch}'s route has a proof; the owner can assemble it.`, view: "review" };
  const pending = Object.values(board.facts).filter((f) => f.state === "proposed" && branchesNeeding(board, f.goal).includes(branch));
  if (pending.length)
    return {
      tone: owner ? "todo" : "wait",
      text: `${plural(pending.length, "proposed decomposition waits", "proposed decompositions wait")} for ${owner ? "your" : "the owner's"} review; until then nobody can work on ${pending.length === 1 ? "its" : "their"} premises.`,
      view: "decompositions",
    };
  const open = openGoals(board, view.picks);
  const names = open.slice(0, 3).map((g) => goalName(board, g)).join(", ") + (open.length > 3 ? ", …" : "");
  return { tone: "todo", text: `${plural(open.length, "goal still needs", "goals still need")} a proof: ${names}. Agents take open goals with mdc pull; you can also claim one on the task board.`, view: "tasks" };
}

/** A goal in the proof tree; `repeat` marks a goal already expanded higher up (shared, or a cycle). */
export interface TreeGoal { goal: string; repeat: boolean; facts: TreeFact[] }
export interface TreeFact { fact: Fact; premises: TreeGoal[] }
const factOrder = (f: Fact) => (f.state === "active" ? 0 : f.state === "proposed" ? 1 : 2);
/** A goal's certificates in the order they are shown: active first, then fewer premises, then by ID. */
export const compareFacts = (a: Fact, b: Fact) =>
  factOrder(a) - factOrder(b) || a.premises.length - b.premises.length || a.id.localeCompare(b.id);
/** Certificates no route uses and no review waits for: paused, retired or refuted. */
export const inUse = (f: Fact) => f.state === "active" || f.state === "proposed";
/**
 * The project's certificates as a tree from the root: each goal lists its certificates (active
 * first, complete proofs before decompositions), each decomposition its premises as goals. A goal
 * is expanded once; later occurrences are repeats. Certificates not in use expand their premises
 * only after all others, so a goal shared with one is expanded where it is used. `others`: goals
 * with certificates the root does not reach, e.g. negations.
 */
export function proofTree(board: Board): { root: TreeGoal | null; others: TreeGoal[] } {
  const facts = Object.values(board.facts);
  const seen = new Set<string>();
  const unused: TreeFact[] = [];
  const node = (goal: string): TreeGoal => {
    if (seen.has(goal)) return { goal, repeat: true, facts: [] };
    seen.add(goal);
    const mine = facts
      .filter((f) => f.goal === goal)
      .sort(compareFacts);
    return { goal, repeat: false, facts: mine.map((fact) => {
      const t: TreeFact = { fact, premises: [] };
      if (inUse(fact)) t.premises = fact.premises.map(node);
      else unused.push(t);
      return t;
    }) };
  };
  const tree = (goal: string) => {
    const g = node(goal);
    for (let t; (t = unused.shift()); ) t.premises = t.fact.premises.map(node);
    return g;
  };
  const root = board.root ? tree(board.root) : null;
  const others: TreeGoal[] = [];
  for (const g of [...new Set(facts.map((f) => f.goal))].sort()) if (!seen.has(g)) others.push(tree(g));
  return { root, others };
}
