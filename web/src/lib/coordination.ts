// Collaboration API (plan §6.3): the same /api/coordination endpoints serve people and
// agents. Commands carry the project revision (If-Match) and an idempotency key.
import { fetchJson } from "./api";
import { projectName } from "./project-path";

export type Trust = "claimed" | "audited" | "trusted";
export interface EnvironmentInput {
  base_key: number;
  context: Record<string, string[]>;
  options: Record<string, string | number | boolean>;
  minimum_trust: Trust;
}
export interface Environment extends EnvironmentInput {
  base: Record<string, unknown>;
  environment_id: string;
  replaces_environment_id?: string;
  updated_by: string;
}
export interface Goal { key: string; statement: string; status: "insufficient" | "derivable" | "certified"; identity?: unknown }
export interface Fact { id: string; goal: string; premises: string[]; trust: Trust; state: "active" | "paused" | "retired"; available: boolean }
export interface Task { id: string; goal: string; kind: "prove" | "decompose" | "formalize"; priority: number; state: "ready" | "leased" | "cancelled"; epoch: number; attempt?: string }
export interface Attempt { id: string; task: string; actor: string; epoch: number; expires: number; reserved: number; spent: number; outcome?: string }
export interface Run {
  id: string; component: string; plan_id?: string; status: "queued" | "running" | "failed" | "certified";
  certification_id?: string; failure?: string; retryable?: boolean;
  route: { root: string; steps: { certificate_id: string; providers: Record<string, string> }[] };
}
export interface NodeBinding {
  node: string; title: string; role: "definition" | "theorem" | "unsupported";
  state: "open" | "submitted" | "registered" | "rejected" | "pending" | "certified";
  goal?: string; conclusion?: string; premises?: string[]; definition_ids?: string[];
  certificate?: string; reason?: string; details?: string;
}
export interface Writeback {
  batch_id: string; run: string; status: "committed" | "aborted"; version?: string;
  reason?: string; details?: string; created: string[]; updated: string[];
  contract?: { operations: { kind: string; node_id: string; certification_id?: string; definition_id?: string }[] };
}
export interface Board {
  id: string; title: string; owner: string; members: string[]; revision: number;
  root: string; minimum_trust: Trust; notes: string; budget: number; spent: number;
  ready: boolean; last_sync?: number; sync_error?: string;
  goals: Record<string, Goal>; facts: Record<string, Fact>; tasks: Record<string, Task>;
  attempts: Record<string, Attempt>; runs: Record<string, Run>;
  request?: {
    database: string; branch: string; node: string; environment_id: string; bound: boolean;
    stale: boolean; identity?: string; error?: string; checked_version?: string;
  };
  nodes?: Record<string, NodeBinding>;
  writebacks?: Record<string, Writeback>;
}
export type Command =
  | { type: "sync" }
  | { type: "claim"; task: string; allocation: number; ttl: number }
  | { type: "heartbeat"; attempt: string; epoch: number; ttl: number }
  | { type: "finish"; attempt: string; epoch: number; spent: number; outcome: "no_progress" | "submitted" | "execution_failed" | "released" }
  | { type: "task"; goal: string; kind: "prove" | "decompose" | "formalize"; priority: number }
  | { type: "decomposition"; certificate: string; state: "active" | "paused" | "retired" }
  | { type: "submit_node"; node: string }
  | { type: "assemble"; component: "lean-worker" }
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
  me: () => fetchJson<{ actor: string; admin: boolean }>("/api/me"),
  environment: (database: string, branch: string) =>
    fetchJson<Environment>(`${base}/environments/${database}/${branch}`),
  putEnvironment: (database: string, branch: string, input: EnvironmentInput) =>
    fetchJson<Environment>(`${base}/environments/${database}/${branch}`, json("PUT", input)),
  projects: (database: string, branch: string, node?: string) =>
    fetchJson<Board[]>(`${base}/projects?${new URLSearchParams({ database, branch, ...(node ? { node } : {}) })}`),
  project: (id: string) => fetchJson<Board>(`${base}/projects/${encodeURIComponent(id)}`),
  create: (input: { document: { database: string; branch: string; node: string }; members: string[]; budget: number; title?: string }) =>
    fetchJson<Board>(`${base}/projects`, json("POST", input, { "idempotency-key": crypto.randomUUID() })),
  command: (board: Pick<Board, "id" | "revision">, command: Command) =>
    fetchJson<{ revision: number; result: unknown; job?: string }>(
      `${base}/projects/${encodeURIComponent(board.id)}/commands`,
      json("POST", command, { "if-match": `"${board.revision}"`, "idempotency-key": crypto.randomUUID() }),
    ),
  jobs: (id: string) =>
    fetchJson<{ id: string; actor: string; kind: string; status: string; tries: number; error: string | null; created_at: string }[]>(
      `${base}/projects/${encodeURIComponent(id)}/jobs`,
    ),
};

/** A readable name for a goal: its document node, else its statement, else a short key. */
export function goalName(board: Board, goal: string): string {
  const node = Object.values(board.nodes ?? {}).find((n) => n.goal === goal);
  if (node) return node.title;
  return board.goals[goal]?.statement || `${goal.slice(0, 10)}...`;
}
