// Reading goals and the maintainer's commands (docs/mathdoc.md §2.7.7): `goals`, `goal`,
// `review`, `pick`, `branch new --pick` and `proof command`.
//
// Replay: a board command is recorded (idempotency key and revision) before it is sent. When its
// reply is lost, running the same command again resends it with the same key and revision; the
// server checks the key first, so it takes effect once. Once a reply has arrived, running the
// command again is a new command. Records live in the state directory, not in a workspace,
// because these commands are not tied to goal files; they never hold a token.
import { randomUUID } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";
import { readFile } from "node:fs/promises";
import { UNCHECKED, type CommandInput } from "../domain.js";
import { MdcError } from "./client.js";
import { sha256 } from "./goal-file.js";
import {
  goalOf,
  liveBoard,
  projectPath,
  scope,
  waitJob,
  type VerbContext,
} from "./verbs.js";
import { writeDurably } from "./workspace.js";

/** `MDC_STATE_DIR`, else `$XDG_STATE_HOME/mdc`, else `~/.local/state/mdc`. */
export function stateDir(env: NodeJS.ProcessEnv) {
  return (
    env.MDC_STATE_DIR ??
    join(env.XDG_STATE_HOME ?? join(homedir(), ".local", "state"), "mdc")
  );
}

export interface CommandRecord {
  format: "mdc-command-record/1";
  server: string;
  project: string;
  type: string;
  key: string;
  /** The revision sent in If-Match; null for commands checked against the current state. */
  revision: number | null;
  /**
   * `sending`: written before sending, outcome unknown; `applied`: the reply arrived;
   * `rejected`: refused, with `reason`; `not_applied`: the revision was outdated, so it never ran.
   */
  status: "sending" | "applied" | "rejected" | "not_applied";
  at: string;
  reason?: string;
}

const recordPath = (ctx: VerbContext, id: string) => {
  if (!ctx.stateDir) throw new Error("no state directory");
  return join(ctx.stateDir, "commands", `${id}.json`);
};
async function readRecord(ctx: VerbContext, id: string) {
  try {
    return JSON.parse(
      await readFile(recordPath(ctx, id), "utf8"),
    ) as CommandRecord;
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw e;
  }
}
const writeRecord = (ctx: VerbContext, id: string, r: CommandRecord) =>
  writeDurably(recordPath(ctx, id), `${JSON.stringify(r, null, 2)}\n`);

/** Send a board command so that a rerun after a lost reply takes effect once. */
export async function sendRecorded(
  ctx: VerbContext,
  project: string,
  body: CommandInput,
): Promise<{ revision: number; result: any; job?: string; resent: boolean }> {
  const c = ctx.client;
  const id = sha256(`${c.origin}\0${project}\0${JSON.stringify(body)}`);
  let r = await readRecord(ctx, id);
  const checked = !UNCHECKED.has(body.type);
  for (let attempt = 0; ; attempt++) {
    const resent = r?.status === "sending";
    if (!r || !resent) {
      r = {
        format: "mdc-command-record/1",
        server: c.origin,
        project,
        type: body.type,
        key: `cmd-${randomUUID()}`,
        revision: checked ? (await c.board(project)).revision : null,
        status: "sending",
        at: new Date(ctx.now()).toISOString(),
      };
      await writeRecord(ctx, id, r);
    }
    try {
      const res = await c.request(
        "POST",
        projectPath(project, "/commands"),
        body,
        {
          "if-match": r.revision === null ? "*" : `"${r.revision}"`,
          "idempotency-key": r.key,
        },
      );
      r.status = "applied";
      await writeRecord(ctx, id, r);
      return { ...res, resent };
    } catch (e) {
      if (!(e instanceof MdcError)) throw e;
      if (e.uncertain)
        throw new MdcError(
          "unreachable",
          `${e.details ?? e.reason}; the command may have arrived: run the same command again to resend it safely`,
          undefined,
          true,
        );
      // An outdated revision means it never ran: read the board again and resend.
      if (e.reason === "revision_conflict" && attempt < 5) {
        r.status = "not_applied";
        await writeRecord(ctx, id, r);
        r = undefined;
        continue;
      }
      r.status = "rejected";
      r.reason = e.reason;
      await writeRecord(ctx, id, r);
      throw e;
    }
  }
}

/** A certificate by its ID or a unique prefix of 8+ hex digits; of `goal` when given. */
export function certificateOf(board: any, ref: string, goal?: string): string {
  if (!/^[0-9a-f]{8,64}$/.test(ref))
    throw new MdcError(
      "unknown_certificate",
      `${ref} is not a certificate ID or a prefix of 8 or more hex digits`,
    );
  const hits = Object.values(board.facts as Record<string, any>).filter(
    (f) => f.id.startsWith(ref) && (!goal || f.goal === goal),
  );
  if (hits.length === 1) return hits[0].id;
  throw new MdcError(
    hits.length ? "ambiguous_certificate" : "unknown_certificate",
    hits.length
      ? `${ref} starts ${hits.length} certificates; give more digits`
      : `no certificate${goal ? ` of goal ${goal.slice(0, 12)}` : ""} starts with ${ref}`,
  );
}

/** `mdc goals`: one line per goal. */
export async function goals(ctx: VerbContext) {
  const s = scope(ctx, "goals");
  const board = await liveBoard(ctx.client, s.database, s.home);
  const params = new URLSearchParams();
  if (ctx.flag("--status")) params.set("status", ctx.flag("--status")!);
  if (ctx.flag("--branch")) params.set("branch", ctx.flag("--branch")!);
  return ctx.client.request(
    "GET",
    projectPath(board.id, `/goals${params.size ? `?${params}` : ""}`),
  );
}

/** `mdc goal GOAL`: everything about one goal. */
export async function goal(ctx: VerbContext, ref: string | undefined) {
  if (!ref) throw new MdcError("usage", "mdc goal GOAL");
  const s = scope(ctx, "goal");
  const board = await liveBoard(ctx.client, s.database, s.home);
  const key = await goalOf(ctx.client, board, ref, s.database, s.branch);
  return ctx.client.request("GET", projectPath(board.id, `/goals/${key}`));
}

const DECISIONS = {
  "--accept": "active",
  "--pause": "paused",
  "--retire": "retired",
} as const;

/** `mdc review`: list decompositions awaiting review, or decide on one (owners). */
export async function review(ctx: VerbContext, ref: string | undefined) {
  const s = scope(ctx, "review");
  const board = await liveBoard(ctx.client, s.database, s.home);
  const chosen = (Object.keys(DECISIONS) as (keyof typeof DECISIONS)[]).filter(
    (f) => ctx.has(f),
  );
  if (!ref) {
    if (chosen.length)
      throw new MdcError("usage", "mdc review CERT --accept|--pause|--retire");
    return ctx.client.request(
      "GET",
      projectPath(
        board.id,
        `/decompositions?state=${encodeURIComponent(ctx.flag("--state") ?? "proposed")}`,
      ),
    );
  }
  if (chosen.length !== 1)
    throw new MdcError(
      "usage",
      "mdc review CERT takes one of --accept, --pause, --retire",
    );
  const certificate = certificateOf(board, ref);
  const comment = ctx.flag("--message");
  const res = await sendRecorded(ctx, board.id, {
    type: "decomposition",
    certificate,
    state: DECISIONS[chosen[0]],
    ...(comment ? { comment } : {}),
  });
  return {
    certificate,
    goal: res.result.goal,
    state: res.result.state,
    review: res.result.review,
    revision: res.revision,
    resent: res.resent,
  };
}

/** Set (or with `auto`, clear) one goal's pick on a branch. */
async function setPick(
  ctx: VerbContext,
  board: any,
  database: string,
  branch: string,
  goalRef: string,
  certRef: string,
) {
  const key = await goalOf(ctx.client, board, goalRef, database, branch);
  const certificate =
    certRef === "auto" ? null : certificateOf(board, certRef, key);
  const res = await sendRecorded(ctx, board.id, {
    type: "pick",
    branch,
    goal: key,
    certificate,
  });
  return { goal: key, certificate, picks: res.result, resent: res.resent };
}

/** `mdc pick GOAL CERT|auto [--branch B]`: the owner, or an editor of the branch. */
export async function pick(
  ctx: VerbContext,
  goalRef: string | undefined,
  certRef: string | undefined,
) {
  if (!goalRef || !certRef)
    throw new MdcError("usage", "mdc pick GOAL CERT|auto [--branch B]");
  const s = scope(ctx, "pick");
  const board = await liveBoard(ctx.client, s.database, s.home);
  return {
    branch: s.branch,
    ...(await setPick(ctx, board, s.database, s.branch, goalRef, certRef)),
  };
}

/**
 * `mdc branch new NAME --pick GOAL=CERT …`: the document branch (from -p's branch), attached to
 * the project once converted, then its picks. The conversion derives picks from the branch's
 * nodes, so picks are set only after it.
 */
export async function branchNew(
  ctx: VerbContext,
  name: string,
  picks: string[],
  attach: boolean,
) {
  const s = scope(ctx, "branch new");
  const pairs = picks.flatMap((p) => p.split(",")).filter(Boolean);
  for (const p of pairs)
    if (!/^[^=]+=[^=]+$/.test(p))
      throw new MdcError("usage", `--pick takes GOAL=CERT, not ${p}`);
  const created = await ctx.client.request("POST", "/api/projects", {
    action: "new_branch",
    project: `${s.database}/${s.home}`,
    name,
  });
  if (!attach && !pairs.length) return created;
  let board = await liveBoard(ctx.client, s.database, s.home);
  const attached = await sendRecorded(ctx, board.id, {
    type: "attach",
    branch: name,
  });
  if (attached.job) await waitJob(ctx, board.id, attached.job);
  board = await ctx.client.board(board.id);
  const state = board.branches?.[name];
  if (!state?.request?.bound)
    throw new MdcError(
      state?.request?.error ?? "branch_not_bound",
      `${name} was created and attached, but its conversion did not bind`,
    );
  const out = [];
  for (const p of pairs) {
    const [g, c] = p.split("=");
    out.push(await setPick(ctx, board, s.database, name, g, c));
  }
  board = await ctx.client.board(board.id);
  return {
    branch: name,
    created,
    project: board.id,
    picks: board.branches[name].picks,
    set: out,
  };
}
