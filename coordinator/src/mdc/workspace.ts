// The `.mdc/` working directory (docs/mathdoc.md §4.11): which goal each file belongs to, its
// lease and its submissions. It never holds a token, so it can be shared without leaking one.
// Records are written (and synced) before a request is sent, so a rerun can replay it.
import { mkdir, open, readdir, readFile, rename, stat } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { MdcError } from "./client.js";

export const DIRECTORY = ".mdc";

export interface Workspace {
  root: string;
  server: string;
}
export interface Lease {
  task: string;
  attempt: string;
  epoch: number;
  actor: string;
  /** ISO time the lease ends unless renewed. */
  expires: string;
  ttl: number;
  allocation: number;
  claimed_at: string;
}
export interface Submission {
  key: string;
  revision: number;
  request_sha256: string;
  intent: "prove" | "split";
  /**
   * `sending`: written before sending, outcome unknown; `queued`: the board took it; `certified`:
   * its job recorded the certificate; `rejected`: refused, with `reason`; `not_applied`: the
   * revision was outdated, so it never ran.
   */
  status: "sending" | "queued" | "certified" | "rejected" | "not_applied";
  at: string;
  job?: string;
  certificate?: string;
  /** The certificate's state on the board: `proposed` awaits the owner's review. */
  state?: string;
  reason?: string;
  /** For `rejected`: LeanGround's explanation, e.g. Lean's errors. */
  messages?: string;
  premises?: unknown[];
}
export interface FileRecord {
  format: "mdc-goal-file-record/1";
  /** Path of the goal file relative to the workspace root. */
  file: string;
  project: string;
  database: string;
  branch: string;
  goal: string;
  environment_id: string;
  root: string;
  root_signature: string;
  definitions: string[];
  premises: { name: string; goal: string }[];
  prefix_sha256: string;
  pulled_revision: number;
  lease: Lease | null;
  submissions: Submission[];
}

/**
 * A module project's goal directory (docs/mathdoc.md §4.13): every `.lean` file in it
 * is the contribution, except the shared modules it was pulled with, which must stay unchanged.
 */
export interface ModuleRecord {
  format: "mdc-module-dir-record/1";
  /** Path of the directory relative to the workspace root. */
  file: string;
  project: string;
  database: string;
  branch: string;
  goal: string;
  environment_id: string;
  /** The statement constant: the root theorem a contribution proves, unless `--root` names another. */
  root: string;
  /**
   * Where the starting files came from: the root node, the decomposition stating the goal, or
   * (a negation) the refuted goal's files with `¬` at the end of one.
   */
  statement: {
    from: "node" | "decomposition" | "negation";
    certificate?: string;
    /** A negation: the goal it refutes. */
    negates?: string;
    paths: string[];
  };
  /** Shared modules: their IDs (sent with every submission) and their files' SHA-256 by path. */
  shared: { ids: string[]; files: Record<string, string> };
  pulled_revision: number;
  lease: Lease | null;
  submissions: Submission[];
}
/** Either kind of record: what leases, replays and `done` need of both. */
export type AnyRecord = FileRecord | ModuleRecord;

async function exists(path: string) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** The nearest `.mdc/` at or above `dir`, like git looks for `.git/`. */
export async function find(dir: string): Promise<Workspace | undefined> {
  for (let d = resolve(dir); ; d = dirname(d)) {
    const file = join(d, DIRECTORY, "workspace.json");
    if (await exists(file)) {
      const w = JSON.parse(await readFile(file, "utf8"));
      return { root: d, server: String(w.server) };
    }
    if (dirname(d) === d) return undefined;
  }
}

/** Write a file durably: a synced temporary file renamed over the target. */
export async function writeDurably(path: string, text: string) {
  await mkdir(dirname(path), { recursive: true });
  const tmp = `${path}.${process.pid}.tmp`;
  const fh = await open(tmp, "w");
  try {
    await fh.writeFile(text, "utf8");
    await fh.sync();
  } finally {
    await fh.close();
  }
  await rename(tmp, path);
}

export async function create(dir: string, server: string): Promise<Workspace> {
  const root = resolve(dir);
  await writeDurably(
    join(root, DIRECTORY, "workspace.json"),
    `${JSON.stringify({ format: "mdc-workspace/1", server }, null, 2)}\n`,
  );
  return { root, server };
}

/** The record path of a file: `.mdc/files/<path relative to the root, encoded>.json`. */
export function relativeFile(w: Workspace, file: string) {
  const rel = relative(w.root, resolve(file));
  if (rel.startsWith("..") || rel.startsWith(sep) || rel === "")
    throw new MdcError(
      "outside_workspace",
      `${file} is not inside the workspace ${w.root}`,
    );
  return rel.split(sep).join("/");
}
const recordPath = (w: Workspace, rel: string) =>
  join(w.root, DIRECTORY, "files", `${encodeURIComponent(rel)}.json`);

export async function read(
  w: Workspace,
  rel: string,
): Promise<AnyRecord | undefined> {
  try {
    return JSON.parse(await readFile(recordPath(w, rel), "utf8"));
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw e;
  }
}

export async function write(w: Workspace, r: AnyRecord) {
  await writeDurably(recordPath(w, r.file), `${JSON.stringify(r, null, 2)}\n`);
}

export async function list(w: Workspace): Promise<AnyRecord[]> {
  let names: string[];
  try {
    names = await readdir(join(w.root, DIRECTORY, "files"));
  } catch {
    return [];
  }
  const out: AnyRecord[] = [];
  for (const n of names.filter((x) => x.endsWith(".json")).sort()) {
    const r = await read(w, decodeURIComponent(n.slice(0, -".json".length)));
    if (r) out.push(r);
  }
  return out;
}
