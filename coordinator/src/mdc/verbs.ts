// The proof workflow verbs of `mdc` (docs/mathdoc.md §2.5, §2.7.2, §2.7.3): `pull` claims a goal
// and writes its goal file, `prove` / `split` submit the file, `done` ends the lease, `note`
// leaves what was learned on the goal. Protocol details (revisions, idempotency keys, lease
// renewal, GoalKey lookup) stay in here.
//
// Leases: a CLI is short-lived, so every verb renews the workspace's leases that are past half
// their time (decision recorded in §2.6). A lease that lapses only stops scheduling; a later
// correct submission is still recorded (§4.8).
import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import type { GoalDraft } from "../goal-draft.js";
import { goalName } from "../goal-view.js";
import { Client, MdcError } from "./client.js";
import { parse, render, sha256 } from "./goal-file.js";
import * as ws from "./workspace.js";
import type {
  AnyRecord,
  FileRecord,
  Lease,
  ModuleRecord,
  Submission,
  Workspace,
} from "./workspace.js";
import type { ModuleDraft } from "../modules.js";
import {
  contribution,
  messages as leanMessages,
  writeGoalDir,
} from "./module-dir.js";

export interface VerbContext {
  client: Client;
  cwd: string;
  /** Where maintainer commands keep their replay records (src/mdc/maintain.ts). */
  stateDir?: string;
  /** DATABASE/BRANCH from -p or MDC_PROJECT. */
  project?: string;
  flag(name: string): string | undefined;
  /** Every value of a flag given more than once, e.g. `--shared A.lean --shared B.lean`. */
  all(name: string): string[];
  has(name: string): boolean;
  now(): number;
  sleep(ms: number): Promise<void>;
}

const iso = (ms: number) => new Date(ms).toISOString();
export const projectPath = (project: string, rest = "") =>
  `/api/coordination/projects/${project}${rest}`;
/**
 * -p DATABASE/BRANCH (or MDC_PROJECT), split: `home` is its branch, `branch` that or --branch.
 * `what` names the command in the usage error.
 */
export function scope(ctx: VerbContext, what: string) {
  if (!ctx.project || !/^[\w-]+\/[\w-]+$/.test(ctx.project))
    throw new MdcError(
      "usage",
      `${what} needs -p DATABASE/BRANCH (or MDC_PROJECT)`,
    );
  const [database, home] = ctx.project.split("/");
  return { database, home, branch: ctx.flag("--branch") ?? home };
}
const number = (ctx: VerbContext, name: string, fallback: number) => {
  const raw = ctx.flag(name);
  if (raw === undefined) return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 0)
    throw new MdcError("invalid_argument", `${name} takes a whole number`);
  return n;
};

let actor: Promise<string> | undefined;
let actorClient: Client | undefined;
/** The caller's actor name, once per run. */
function me(c: Client) {
  if (actorClient !== c) {
    actorClient = c;
    actor = c.request("GET", "/api/me").then((x) => String(x.actor));
  }
  return actor!;
}

/** The workspace at or above `dir`; it must belong to the server in use. */
async function workspace(ctx: VerbContext, dir: string, create = false) {
  let w = await ws.find(dir);
  if (!w && create) w = await ws.create(dir, ctx.client.origin);
  if (!w)
    throw new MdcError(
      "not_pulled",
      "no .mdc/ workspace here or above; start with mdc pull",
    );
  if (w.server !== ctx.client.origin)
    throw new MdcError(
      "workspace_server_mismatch",
      `${w.root} belongs to ${w.server}, not ${ctx.client.origin} (MDC_URL)`,
    );
  return w;
}

/** The database's current project board, as seen from the branch. */
export async function liveBoard(c: Client, database: string, branch: string) {
  const boards: any[] = await c.request(
    "GET",
    `/api/coordination/projects?${new URLSearchParams({ database, branch })}`,
  );
  const b = boards.find((x) => x.project && !x.project.superseded);
  if (!b)
    throw new MdcError(
      "no_project",
      `${database} has no current project board you are a member of`,
    );
  return b;
}

/**
 * A goal reference: a full GoalKey, a unique prefix of at least 8 hex digits, the title or
 * UUID of a converted node on the branch, or else the goal's name as `mdc goals` shows it.
 */
export async function goalOf(
  c: Client,
  board: any,
  ref: string,
  database: string,
  branch: string,
): Promise<string> {
  if (/^[0-9a-f]{8,64}$/.test(ref)) {
    const hits = Object.keys(board.goals).filter((k) => k.startsWith(ref));
    if (hits.length === 1) return hits[0];
    if (hits.length > 1)
      throw new MdcError(
        "ambiguous_goal",
        `${ref} starts ${hits.length} goals; give more digits`,
      );
  }
  let fnode: string;
  try {
    ({ fnode } = await c.request(
      "GET",
      `/p/${database}/${branch}/api/resolve?ref=${encodeURIComponent(ref)}`,
    ));
  } catch (e) {
    if (!(e instanceof MdcError && e.status === 404)) throw e;
    // A goal without a node on this branch, by the name `mdc goals` shows.
    const named = Object.keys(board.goals).filter(
      (k) =>
        goalName(board, k) === ref || board.goals[k].declaration?.name === ref,
    );
    if (named.length === 1) return named[0];
    if (named.length > 1)
      throw new MdcError(
        "ambiguous_goal",
        `${named.length} goals are named ${ref}; give a GoalKey prefix`,
      );
    throw new MdcError(
      "unknown_goal",
      `${ref} is neither a goal of the project nor a node of ${branch}`,
    );
  }
  const node = board.branches?.[branch]?.nodes?.[fnode];
  if (!node?.goal)
    throw new MdcError(
      "node_has_no_goal",
      `node ${ref} has no resolved goal on ${branch}${node?.reason ? ` (${node.reason})` : ""}`,
    );
  return node.goal;
}

/** Renew the caller's leases in the workspace that are past half their time. */
async function renew(ctx: VerbContext, w: Workspace, skip?: string) {
  const out = { renewed: [] as string[], lost: [] as string[] };
  const who = await me(ctx.client);
  for (const r of await ws.list(w)) {
    const l = r.lease;
    if (!l || r.file === skip || l.actor !== who) continue;
    if (Date.parse(l.expires) - ctx.now() > (l.ttl * 1000) / 2) continue;
    try {
      // The server's clock decides whether the lease has lapsed.
      const res = await ctx.client.current(
        r.project,
        { type: "heartbeat", attempt: l.attempt, epoch: l.epoch, ttl: l.ttl },
        `heartbeat-${l.attempt}-${l.expires}`,
      );
      l.expires = iso(res.result.expires);
      out.renewed.push(r.file);
    } catch (e) {
      if (!(e instanceof MdcError)) throw e;
      if (!["stale_lease", "not_found"].includes(e.reason)) continue;
      r.lease = null;
      out.lost.push(r.file);
    }
    await ws.write(w, r);
  }
  return out;
}

/** Claim the goal's task, or adopt the caller's live lease on it (a rerun after a lost reply). */
async function claim(
  ctx: VerbContext,
  draft: GoalDraft,
  ttl: number,
  allocation: number,
): Promise<Lease> {
  const who = await me(ctx.client);
  const task = draft.task;
  if (!task) throw new MdcError("no_task", `goal ${draft.goal} has no task`);
  const held = task.attempt;
  if (held && held.actor === who && held.expires > ctx.now())
    return {
      task: task.id,
      attempt: held.id,
      epoch: held.epoch,
      actor: who,
      expires: iso(held.expires),
      ttl,
      allocation: held.reserved,
      claimed_at: iso(ctx.now()),
    };
  let res;
  try {
    res = await ctx.client.current(
      draft.project,
      { type: "claim", task: task.id, allocation, ttl },
      `claim-${task.id}-e${task.epoch}`,
    );
  } catch (e) {
    // A lease only avoids duplicate work; the goal can still be worked on without one.
    if (e instanceof MdcError && e.reason === "task_unavailable" && held)
      throw new MdcError(
        e.reason,
        `${held.actor} holds the task until ${iso(held.expires)}; --no-claim writes the file without a lease`,
        e.status,
      );
    throw e;
  }
  return {
    task: task.id,
    attempt: res.result.id,
    epoch: res.result.epoch,
    actor: who,
    expires: iso(res.result.expires),
    ttl,
    allocation,
    claimed_at: iso(ctx.now()),
  };
}

export async function pull(ctx: VerbContext, ref: string | undefined) {
  if (!ref) throw new MdcError("usage", "mdc pull GOAL");
  const { database, branch } = scope(ctx, "pull");
  const ttl = number(ctx, "--ttl", 3600);
  if (ttl < 30 || ttl > 3600)
    throw new MdcError("invalid_argument", "--ttl is 30 to 3600 seconds");
  const allocation = number(ctx, "--allocation", 0);
  const w = await workspace(ctx, ctx.cwd, true);
  const leases = await renew(ctx, w);
  const board = await liveBoard(ctx.client, database, branch);
  const goal = await goalOf(ctx.client, board, ref, database, branch);
  const params = new URLSearchParams({ branch });
  if (ctx.flag("--with")) params.set("with", ctx.flag("--with")!);
  const fetched: GoalDraft | ModuleDraft = await ctx.client.request(
    "GET",
    projectPath(board.id, `/goals/${goal}/draft?${params}`),
  );
  if ("format" in fetched && fetched.format === "mdc-module-draft/1")
    return pullDirectory(
      ctx,
      w,
      leases,
      fetched,
      database,
      branch,
      ttl,
      allocation,
    );
  const draft = fetched as GoalDraft;
  const rendered = render(draft);
  const file = resolve(
    ctx.cwd,
    ctx.flag("--output") ?? `${goal.slice(0, 8)}-${rendered.root}.lean`,
  );
  const rel = ws.relativeFile(w, file);
  try {
    await readFile(file);
    throw new MdcError("file_exists", `${rel} exists; mdc never overwrites it`);
  } catch (e) {
    if (e instanceof MdcError) throw e;
  }
  const lease = ctx.has("--no-claim")
    ? null
    : await claim(ctx, draft, ttl, allocation);
  const record: FileRecord = {
    format: "mdc-goal-file-record/1",
    file: rel,
    project: draft.project,
    database,
    branch,
    goal,
    environment_id: draft.environment_id,
    root: rendered.root,
    root_signature: rendered.root_signature,
    definitions: rendered.definitions,
    premises: rendered.premises,
    prefix_sha256: rendered.prefix_sha256,
    pulled_revision: draft.revision,
    lease,
    submissions: [],
  };
  await ws.write(w, record);
  await writeFile(file, rendered.text, { flag: "wx" });
  return {
    file: rel,
    project: draft.project,
    goal,
    status: draft.status,
    root: rendered.root,
    statement_from: draft.root.from,
    premises: rendered.premises,
    definitions: draft.definitions.map((d) => ({ id: d.id, name: d.name })),
    lease,
    ...leases,
  };
}

/**
 * A module goal (docs/mathdoc.md §4.13): a directory with the goal's statement files
 * as the starting contribution and the shared modules they import.
 */
async function pullDirectory(
  ctx: VerbContext,
  w: Workspace,
  leases: Awaited<ReturnType<typeof renew>>,
  draft: ModuleDraft,
  database: string,
  branch: string,
  ttl: number,
  allocation: number,
) {
  const name = draft.statement.const.split(".").at(-1)!.replace(/[^\w]/g, "_");
  const dir = resolve(
    ctx.cwd,
    ctx.flag("--output") ?? `${draft.goal.slice(0, 8)}-${name}`,
  );
  const rel = ws.relativeFile(w, dir);
  if (await ws.read(w, rel))
    throw new MdcError("file_exists", `${rel} was pulled already`);
  const lease = ctx.has("--no-claim")
    ? null
    : await claim(ctx, draft as unknown as GoalDraft, ttl, allocation);
  const start = await writeGoalDir(dir, draft);
  const record: ModuleRecord = {
    format: "mdc-module-dir-record/1",
    file: rel,
    project: draft.project,
    database,
    branch,
    goal: draft.goal,
    environment_id: draft.environment_id,
    root: draft.statement.const,
    statement: {
      from: draft.statement.from,
      ...(draft.statement.certificate
        ? { certificate: draft.statement.certificate }
        : {}),
      ...(draft.statement.negates
        ? { negates: draft.statement.negates.goal }
        : {}),
      paths: start.statement,
    },
    shared: { ids: draft.shared.map((s) => s.id), files: start.shared },
    pulled_revision: draft.revision,
    lease,
    submissions: [],
  };
  await ws.write(w, record);
  return {
    directory: rel,
    project: draft.project,
    goal: draft.goal,
    status: draft.status,
    root: draft.statement.const,
    statement_from: draft.statement.from,
    files: start.statement,
    shared: Object.keys(start.shared),
    lease,
    ...leases,
  };
}

/** The record of a pulled file or directory. */
export async function pulled(ctx: VerbContext, file: string) {
  const abs = resolve(ctx.cwd, file);
  const w = await workspace(ctx, dirname(abs));
  const rel = ws.relativeFile(w, abs);
  const record = await ws.read(w, rel);
  if (!record)
    throw new MdcError("not_pulled", `${rel} was not written by mdc pull`);
  return { w, record, abs };
}

/** Refresh a queued submission from its job; with `wait`, until the job ends. */
async function follow(
  ctx: VerbContext,
  w: Workspace,
  record: AnyRecord,
  s: Submission,
  wait: boolean,
) {
  const timeout = number(ctx, "--timeout", 600) * 1000;
  const until = ctx.now() + timeout;
  while (s.status === "queued" && s.job) {
    const job = await ctx.client.request(
      "GET",
      projectPath(record.project, `/jobs/${s.job}`),
    );
    if (job.status === "done") {
      s.status = "certified";
      s.certificate = job.result?.certificate;
      s.state = job.result?.state;
      s.premises = job.result?.premises;
    } else if (job.status === "failed") {
      s.status = "rejected";
      s.reason = job.error ?? "job_failed";
      if (job.details) s.messages = job.details;
    } else if (!wait) break;
    else if (ctx.now() >= until)
      throw new MdcError(
        "wait_timeout",
        `job ${s.job} is still ${job.status}; run the same command again to keep waiting`,
      );
    else await ctx.sleep(1000);
  }
  await ws.write(w, record);
}

/**
 * §4.11 replay: the key and revision are written before sending; an uncertain send is resent
 * unchanged (the server checks the key before the revision, so a send that took effect only
 * returns its first result); an outdated revision means it never ran, so a new one is made.
 */
async function send(
  ctx: VerbContext,
  w: Workspace,
  record: AnyRecord,
  request: Record<string, unknown>,
  hash: string,
  intent: Submission["intent"],
  pending?: Submission,
): Promise<Submission> {
  let s = pending;
  for (let attempt = 0; ; attempt++) {
    if (!s) {
      const board = await ctx.client.board(record.project);
      if (board.project?.environment_id !== record.environment_id)
        throw new MdcError(
          "environment_changed",
          "the project environment changed since mdc pull; pull the goal again",
        );
      s = {
        key: `submit-${hash.slice(0, 32)}-r${board.revision}`,
        revision: board.revision,
        request_sha256: hash,
        intent,
        status: "sending",
        at: iso(ctx.now()),
      };
      record.submissions.push(s);
      await ws.write(w, record);
    }
    try {
      const res = await ctx.client.request(
        "POST",
        projectPath(record.project, "/commands"),
        request,
        { "if-match": `"${s.revision}"`, "idempotency-key": s.key },
      );
      s.status = "queued";
      s.job = res.job;
      await ws.write(w, record);
      return s;
    } catch (e) {
      if (!(e instanceof MdcError)) throw e;
      if (e.uncertain) {
        await ws.write(w, record);
        throw new MdcError(
          "unreachable",
          `${e.details ?? e.reason}; the submission may have arrived: run the same command again to resend it safely`,
          undefined,
          true,
        );
      }
      if (e.reason === "revision_conflict" && attempt < 5) {
        s.status = "not_applied";
        await ws.write(w, record);
        s = undefined;
        continue;
      }
      s.status = "rejected";
      s.reason = e.reason;
      await ws.write(w, record);
      throw e;
    }
  }
}

export async function submit(
  ctx: VerbContext,
  file: string | undefined,
  intent: "prove" | "split",
) {
  if (!file) throw new MdcError("usage", `mdc ${intent} FILE`);
  const description = ctx.flag("--message");
  if (intent === "split" && !description)
    throw new MdcError("usage", "mdc split FILE -m DESCRIPTION");
  const { w, record: pulledRecord } = await pulled(ctx, file);
  // Working on a file, even one still refused locally, keeps its lease; --dry-run stays offline.
  const leases = ctx.has("--dry-run") ? undefined : await renew(ctx, w);
  if (pulledRecord.format === "mdc-module-dir-record/1")
    return submitDirectory(
      ctx,
      w,
      pulledRecord,
      file,
      intent,
      description,
      leases,
    );
  const record = pulledRecord;
  const parsed = parse(await readFile(resolve(ctx.cwd, file), "utf8"), record);
  if (parsed.root_is_sorry)
    throw new MdcError("root_is_sorry", `${record.root} is still sorry`);
  if (intent === "prove" && parsed.mode === "sketch")
    throw new MdcError(
      "has_premises",
      `sorry premises remain (${parsed.placeholders.join(", ")}); use mdc split for a decomposition`,
    );
  if (intent === "split" && parsed.mode === "leaf")
    throw new MdcError(
      "no_premises",
      "no sorry premise remains; use mdc prove for a complete proof",
    );
  const request = {
    type: "submit",
    goal: record.goal,
    source: parsed.source,
    expected_root: record.root,
    component: parsed.component,
    mode: parsed.mode,
    definitions: record.definitions,
    new_definitions: parsed.new_definitions,
    ...(description ? { description } : {}),
  };
  const summary = {
    file: record.file,
    goal: record.goal,
    mode: parsed.mode,
    premises: parsed.placeholders,
    dropped: parsed.dropped,
    new_definitions: parsed.new_definitions,
  };
  if (ctx.has("--dry-run")) return { ...summary, source: parsed.source };
  return {
    ...summary,
    ...(await deliver(ctx, w, record, request, intent)),
    ...leases,
  };
}

/** Send a submission once (§4.11 replay) and follow its job; what the verb reports about it. */
async function deliver(
  ctx: VerbContext,
  w: Workspace,
  record: AnyRecord,
  request: Record<string, unknown>,
  intent: "prove" | "split",
) {
  const fresh = (await ws.read(w, record.file))!;
  const hash = sha256(JSON.stringify(request));
  const last = fresh.submissions
    .filter((x) => x.request_sha256 === hash)
    .at(-1);
  let s: Submission;
  let replayed = false;
  if (
    last &&
    !ctx.has("--again") &&
    ["queued", "certified"].includes(last.status)
  ) {
    s = last;
    replayed = true;
  } else
    s = await send(
      ctx,
      w,
      fresh,
      request,
      hash,
      intent,
      last?.status === "sending" && !ctx.has("--again") ? last : undefined,
    );
  await follow(ctx, w, fresh, s, ctx.has("--wait"));
  return {
    status: s.status,
    replayed,
    key: s.key,
    job: s.job,
    ...(s.certificate ? { certificate: s.certificate } : {}),
    ...(s.state ? { certificate_state: s.state } : {}),
    ...(s.premises ? { premise_goals: s.premises } : {}),
    ...(s.reason ? { reason: s.reason } : {}),
    ...(s.messages ? { messages: s.messages } : {}),
  };
}

/**
 * `mdc prove DIR` / `mdc split DIR`: the directory's files as one contribution. Whether it is a
 * complete proof or a decomposition is LeanGround's finding (its placeholders), checked before
 * anything is recorded: `has_premises` / `no_premises` as with goal files.
 */
async function submitDirectory(
  ctx: VerbContext,
  w: Workspace,
  record: ModuleRecord,
  dir: string,
  intent: "prove" | "split",
  description: string | undefined,
  leases: Awaited<ReturnType<typeof renew>> | undefined,
) {
  const newShared = ctx.all("--shared");
  if (newShared.length && intent !== "split")
    throw new MdcError(
      "usage",
      "--shared names files of a decomposition's new shared modules: mdc split",
    );
  const { files, newShared: shared } = await contribution(
    resolve(ctx.cwd, dir),
    record,
    newShared,
  );
  const root = ctx.flag("--root") ?? record.root;
  const request = {
    type: "submit_modules",
    goal: record.goal,
    files,
    root,
    shared: record.shared.ids,
    ...(shared ? { new_shared: shared } : {}),
    mode: intent === "prove" ? "leaf" : "sketch",
    ...(description ? { description } : {}),
  };
  const summary = {
    directory: record.file,
    goal: record.goal,
    root,
    files: Object.keys(files),
    ...(shared ? { new_shared: Object.keys(shared) } : {}),
  };
  if (ctx.has("--dry-run")) return { ...summary, request };
  return {
    ...summary,
    ...(await deliver(ctx, w, record, request, intent)),
    ...leases,
  };
}

export async function done(ctx: VerbContext, file: string | undefined) {
  let w: Workspace;
  let record: AnyRecord;
  if (file) ({ w, record } = await pulled(ctx, file));
  else {
    w = await workspace(ctx, ctx.cwd);
    const who = await me(ctx.client);
    const held = (await ws.list(w)).filter((r) => r.lease?.actor === who);
    if (held.length !== 1)
      throw new MdcError(
        held.length ? "lease_ambiguous" : "no_lease",
        held.length
          ? `leases on ${held.map((r) => r.file).join(", ")}: name the file`
          : "no file in this workspace holds a lease of yours",
      );
    record = held[0];
  }
  const lease = record.lease;
  if (!lease) throw new MdcError("no_lease", `${record.file} holds no lease`);
  const spent = number(ctx, "--spent", 0);
  const note = ctx.flag("--note");
  const leases = await renew(ctx, w, record.file);
  const board = await ctx.client.board(record.project);
  const a = board.attempts?.[lease.attempt];
  const outcome =
    ctx.flag("--outcome") ??
    (record.submissions.some(
      (s) =>
        ["queued", "certified"].includes(s.status) && s.at >= lease.claimed_at,
    )
      ? "submitted"
      : "no_progress");
  let result: Record<string, unknown>;
  if (a?.outcome)
    result = { lease: "already_finished", outcome: a.outcome, spent: a.spent };
  else
    try {
      await ctx.client.current(
        record.project,
        {
          type: "finish",
          attempt: lease.attempt,
          epoch: lease.epoch,
          spent,
          outcome,
          ...(note ? { note } : {}),
        },
        `finish-${lease.attempt}`,
      );
      result = { lease: "finished", outcome, spent };
    } catch (e) {
      if (!(e instanceof MdcError) || e.reason !== "stale_lease") throw e;
      result = { lease: "lost", reason: "stale_lease" };
    }
  record.lease = null;
  await ws.write(w, record);
  return { file: record.file, goal: record.goal, ...result, ...leases };
}

/** Leave a note on a goal: a pulled file's goal, or a goal reference with -p. */
export async function note(ctx: VerbContext, ref: string | undefined) {
  const text = ctx.flag("--message");
  if (!ref || !text) throw new MdcError("usage", "mdc note FILE|GOAL -m TEXT");
  let target: { project: string; goal: string } | undefined;
  let w: Workspace | undefined;
  try {
    const p = await pulled(ctx, ref);
    w = p.w;
    target = { project: p.record.project, goal: p.record.goal };
  } catch (e) {
    if (!(e instanceof MdcError) || e.reason !== "not_pulled") throw e;
  }
  if (!target) {
    const { database, home: branch } = scope(ctx, "a goal reference");
    const board = await liveBoard(ctx.client, database, branch);
    target = {
      project: board.id,
      goal: await goalOf(ctx.client, board, ref, database, branch),
    };
    w = await ws.find(ctx.cwd);
  }
  const leases =
    w && w.server === ctx.client.origin ? await renew(ctx, w) : undefined;
  // The same note twice by the same agent is kept once: a rerun after a lost reply replays.
  const key = sha256(`${target.goal}\0${text}`).slice(0, 32);
  const res = await ctx.client.current(
    target.project,
    { type: "note", goal: target.goal, text },
    `note-${key}`,
  );
  return { goal: target.goal, note: res.result, ...leases };
}

/** Wait for a queued job (the worker may run elsewhere); its result, or its error as a failure. */
export async function waitJob(ctx: VerbContext, project: string, job: string) {
  const until = ctx.now() + number(ctx, "--timeout", 600) * 1000;
  for (;;) {
    const j = await ctx.client.request(
      "GET",
      projectPath(project, `/jobs/${job}`),
    );
    if (j.status === "done") return j.result;
    if (j.status === "failed")
      throw new MdcError(
        j.error ?? "job_failed",
        `job ${job} failed`,
        undefined,
        false,
        j.details ?? undefined,
      );
    if (ctx.now() >= until)
      throw new MdcError("wait_timeout", `job ${job} is still ${j.status}`);
    await ctx.sleep(1000);
  }
}

/**
 * Work on refuting a goal (docs/mathdoc.md §2.7.5): register its negation ¬GOAL as a goal if
 * needed, then pull that goal's file. Proving it with `mdc prove` marks GOAL false.
 */
export async function refute(ctx: VerbContext, ref: string | undefined) {
  if (!ref) throw new MdcError("usage", "mdc refute GOAL");
  const { database, branch } = scope(ctx, "refute");
  const board = await liveBoard(ctx.client, database, branch);
  const goal = await goalOf(ctx.client, board, ref, database, branch);
  let negation = Object.entries(
    (board.negations ?? {}) as Record<string, string>,
  ).find(([, t]) => t === goal)?.[0];
  if (!negation) {
    const res = await ctx.client.current(
      board.id,
      { type: "negate", goal },
      `negate-${goal}`,
    );
    negation =
      res.result?.negation ??
      (await waitJob(ctx, board.id, String(res.job))).negation;
  }
  return { refutes: goal, ...(await pull(ctx, negation)) };
}

/**
 * A draft check (§2.7.8): LeanGround checks the file on the project's base and answers with
 * Lean's diagnostics. Nothing is recorded and no certificate is made, so it is no submission.
 * A pulled file is checked in its own project and renews its workspace's leases; any other
 * file in the project of MDC_PROJECT (or -p).
 */
export async function check(ctx: VerbContext, file: string | undefined) {
  if (!file) throw new MdcError("usage", "mdc check FILE|DIR");
  let project: string;
  let leases: { renewed: string[]; lost: string[] } | undefined;
  try {
    const { w, record } = await pulled(ctx, file);
    project = record.project;
    leases = await renew(ctx, w);
    if (record.format === "mdc-module-dir-record/1") {
      // docs/mathdoc.md §4.13: compiled in LeanGround's sandbox and checked as a
      // contribution to the directory's goal; new shared files are compiled with the rest.
      const { files, newShared } = await contribution(
        resolve(ctx.cwd, file),
        record,
      );
      const root = ctx.flag("--root") ?? record.root;
      const out = await ctx.client.request(
        "POST",
        projectPath(project, "/check"),
        {
          files: { ...files, ...newShared },
          shared: record.shared.ids,
          goal: record.goal,
          root,
        },
      );
      const lines = [
        leanMessages(out.diagnostics),
        ...(Array.isArray(out.violations)
          ? out.violations.map((v: string) => `${root}: ${v}`)
          : []),
      ].filter(Boolean);
      return {
        directory: record.file,
        root,
        ok: out.ok === true,
        compiled: out.compiled === true,
        ...(out.reason ? { reason: out.reason } : {}),
        premises: Array.isArray(out.premises)
          ? out.premises.map((p: { name?: string }) => p.name)
          : [],
        axioms: out.axioms ?? [],
        ...(lines.length ? { messages: lines.join("\n") } : {}),
        ...leases,
      };
    }
  } catch (e) {
    if (!(e instanceof MdcError && e.reason === "not_pulled")) throw e;
    const { database, home } = scope(ctx, "check");
    // A scratch file, e.g. `#check` lines: LeanGround's resident checker on the base, in a
    // module project too (a sandboxed build would import Mathlib afresh every time).
    project = (await liveBoard(ctx.client, database, home)).id;
  }
  const source = await readFile(resolve(ctx.cwd, file), "utf8");
  const out = await ctx.client.request("POST", projectPath(project, "/check"), {
    source,
  });
  // Lean's messages as plain lines, which the command line also prints as they are.
  const messages = (
    out.diagnostics as {
      severity: string;
      line?: number | null;
      col?: number | null;
      text: string;
    }[]
  )
    .map(
      (d) =>
        `${file}:${d.line ?? "?"}:${d.col ?? "?"}: ${d.severity}: ${d.text}`,
    )
    .join("\n");
  return { file, ...out, ...(messages ? { messages } : {}), ...leases };
}

/** Declarations of the project's base whose names contain one of the queries (§2.7.8). */
export async function lemma(ctx: VerbContext, queries: string[]) {
  if (!queries.length)
    throw new MdcError(
      "usage",
      "mdc lemma QUERY... [--kind theorem|definition|all] [-n N]",
    );
  const { database, home } = scope(ctx, "lemma");
  const board = await liveBoard(ctx.client, database, home);
  const params = new URLSearchParams();
  for (const q of queries) params.append("q", q);
  if (ctx.flag("--kind")) params.set("kind", ctx.flag("--kind")!);
  params.set("limit", String(number(ctx, "--max", 20)));
  return ctx.client.request("GET", projectPath(board.id, `/search?${params}`));
}

/** The goals most worth working on (§2.7.5); with --claim, pull the first one. */
export async function next(ctx: VerbContext) {
  const { database, home } = scope(ctx, "next");
  const board = await liveBoard(ctx.client, database, home);
  const params = new URLSearchParams({
    limit: String(number(ctx, "--max", 10)),
  });
  if (ctx.flag("--branch")) params.set("branch", ctx.flag("--branch")!);
  const out = await ctx.client.request(
    "GET",
    projectPath(board.id, `/next?${params}`),
  );
  if (!ctx.has("--claim")) return out;
  const first = out.candidates[0];
  if (!first) throw new MdcError("nothing_to_do", "no free goal needs work");
  return { recommended: first, ...(await pull(ctx, first.goal)) };
}
