import pg from "pg";
import { randomUUID, createHash } from "node:crypto";
import {
  Board,
  Command,
  Attempt,
  Task,
  activeGoals,
  command,
  endLease,
  type Grants,
  UNCHECKED,
  member,
  principalOf,
  readOnly,
  renewLease,
  requireThat,
  takeLease,
} from "./domain.js";
export { UNCHECKED } from "./domain.js";
import { ENVIRONMENT_MIGRATION } from "./proof.js";
import { TOKEN_MIGRATION } from "./identity.js";
import { IMPORT_MIGRATION } from "./imports.js";
export interface Job {
  id: string;
  project: string;
  actor: string;
  kind: string;
  payload: Record<string, unknown>;
  epoch: number;
  tries: number;
}
// docs/mathdoc.md §2.7.5: tasks and attempts, which every lease command changes, live in rows of
// their own; `meta` holds the few board fields the lease path reads. Boards written before keep
// them in the board document until `migrate` moves them.
export const migration = `
CREATE TABLE IF NOT EXISTS mdc_workspace(database text PRIMARY KEY, body jsonb NOT NULL);
CREATE TABLE IF NOT EXISTS mdc_project(id text PRIMARY KEY, body jsonb NOT NULL);
CREATE TABLE IF NOT EXISTS mdc_command(project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, key text NOT NULL, hash text NOT NULL, result jsonb NOT NULL, PRIMARY KEY(project,actor,key));
CREATE TABLE IF NOT EXISTS mdc_event(sequence bigserial PRIMARY KEY, project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, kind text NOT NULL, revision bigint NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS mdc_job(id text PRIMARY KEY, project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, kind text NOT NULL, payload jsonb NOT NULL, status text NOT NULL DEFAULT 'queued', epoch integer NOT NULL DEFAULT 0, tries integer NOT NULL DEFAULT 0, expires timestamptz, available_at timestamptz NOT NULL DEFAULT now(), error text, created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX IF NOT EXISTS mdc_job_queue ON mdc_job(status,available_at);
ALTER TABLE mdc_job ADD COLUMN IF NOT EXISTS result jsonb;
ALTER TABLE mdc_job ADD COLUMN IF NOT EXISTS details text;
CREATE UNIQUE INDEX IF NOT EXISTS mdc_project_scope ON mdc_project((body->'project'->>'database'),(body->'project'->>'environment_id'));
ALTER TABLE mdc_project ADD COLUMN IF NOT EXISTS meta jsonb;
CREATE TABLE IF NOT EXISTS mdc_task(project text NOT NULL REFERENCES mdc_project(id), id text NOT NULL, body jsonb NOT NULL, PRIMARY KEY(project,id));
CREATE TABLE IF NOT EXISTS mdc_attempt(project text NOT NULL REFERENCES mdc_project(id), id text NOT NULL, open boolean NOT NULL, body jsonb NOT NULL, PRIMARY KEY(project,id));
CREATE INDEX IF NOT EXISTS mdc_attempt_open ON mdc_attempt(project) WHERE open;
INSERT INTO mdc_task SELECT p.id, t.key, t.value FROM mdc_project p, jsonb_each(p.body->'tasks') t ON CONFLICT DO NOTHING;
INSERT INTO mdc_attempt SELECT p.id, a.key, NOT (a.value ? 'outcome'), a.value FROM mdc_project p, jsonb_each(p.body->'attempts') a ON CONFLICT DO NOTHING;
UPDATE mdc_project SET body = body - 'tasks' - 'attempts' - 'spent', meta = NULL WHERE body ? 'tasks' OR body ? 'attempts';
${ENVIRONMENT_MIGRATION}${TOKEN_MIGRATION}${IMPORT_MIGRATION}`;
// Boards that take part in scheduling: not a stage 4 proof request, not superseded.
const LIVE = `NOT (p.body ? 'request') AND NOT COALESCE((p.body->'project'->>'superseded')::boolean,false)`;

/** What the lease path and permission checks need, without reading the board document. */
export interface Meta {
  revision: number;
  owner: string;
  members: string[];
  ready: boolean;
  budget: number;
  read_only: string | null;
  /** The branch whose read access guards the board (§4.3). */
  home: { database: string; branch: string } | null;
  database: string | null;
  /** Goals whose tasks can be claimed, and certified goals. */
  active: string[];
  certified: string[];
}
export function metaOf(b: Board): Meta {
  return {
    revision: b.revision,
    owner: b.owner,
    members: b.members,
    ready: b.ready,
    budget: b.budget,
    read_only: readOnly(b) ?? null,
    home: b.project
      ? { database: b.project.database, branch: "main" }
      : b.request
        ? { database: b.request.database, branch: b.request.branch }
        : null,
    database: b.project?.database ?? null,
    active: [...activeGoals(b)],
    certified: Object.values(b.goals)
      .filter((g) => g.status === "certified")
      .map((g) => g.key),
  };
}
/** Commands that never write the board document: they take the lease path. */
const LIGHT = new Set<Command["type"]>([
  "claim",
  "heartbeat",
  "finish",
  "submit",
  "resolve",
  "import",
  "sync",
  "submit_node",
  "negate",
]);
type Db = Pick<pg.PoolClient, "query">;
/** The board document as stored: tasks and attempts have rows, spent is derived. */
function documentOf(b: Board) {
  const { tasks: _t, attempts: _a, spent: _s, ...body } = b;
  return body;
}
/**
 * JSON with object keys sorted. PostgreSQL's jsonb reorders keys, so a value read back and the
 * same value received again (a certificate's goal identity, every sync) differ in key order only.
 */
function canonical(value: unknown) {
  return JSON.stringify(value, (_k, v) =>
    v && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(
          Object.keys(v)
            .sort()
            .map((k) => [k, v[k]]),
        )
      : v,
  );
}
/** Everything a client can act on: the board without sync bookkeeping. */
function observable(b: Board) {
  const { last_sync: _, ...rest } = b;
  return canonical(rest);
}
interface Loaded {
  board: Board;
  /** Tasks and attempts as read, to write back only what changed. */
  seen: Map<string, string>;
}
/**
 * Retryable job failures (LeanGround or TerminusDB unreachable, timeouts) back off: 5 s, 15 s,
 * 45 s, … at most 15 minutes apart, for 8 tries in all (about 40 minutes; §2.7.7).
 */
export const JOB_TRIES = 8;
export const retryDelay = (tries: number) =>
  Math.min(5 * 3 ** Math.max(tries - 1, 0), 900);
const leaseLock = (c: Db, project: string) =>
  c.query("SELECT pg_advisory_xact_lock(hashtext($1))", [
    `mdc-lease:${project}`,
  ]);

export class Store {
  pool: pg.Pool;
  constructor(dsn: string) {
    this.pool = new pg.Pool({ connectionString: dsn, max: 10 });
  }
  async migrate() {
    const c = await this.pool.connect();
    try {
      await c.query("BEGIN");
      await c.query("SELECT pg_advisory_xact_lock(682193117)");
      await c.query(migration);
      // Boards moved above (or created before `meta`) get it now; their revision is unchanged.
      for (const { id } of (
        await c.query("SELECT id FROM mdc_project WHERE meta IS NULL")
      ).rows) {
        const { board } = (await this.load(c, id, false))!;
        await c.query("UPDATE mdc_project SET meta=$2 WHERE id=$1", [
          id,
          metaOf(board),
        ]);
      }
      await c.query("COMMIT");
    } catch (e) {
      await c.query("ROLLBACK");
      throw e;
    } finally {
      c.release();
    }
  }
  async transaction<T>(fn: (c: pg.PoolClient) => Promise<T>): Promise<T> {
    const c = await this.pool.connect();
    try {
      await c.query("BEGIN");
      const out = await fn(c);
      await c.query("COMMIT");
      return out;
    } catch (e) {
      await c.query("ROLLBACK");
      throw e;
    } finally {
      c.release();
    }
  }
  /** The board document with its tasks and attempts, as one Board. */
  private async assemble(db: Db, body: Board): Promise<Loaded> {
    const seen = new Map<string, string>();
    const b = { ...body, tasks: {}, attempts: {}, spent: 0 } as Board;
    for (const r of (
      await db.query("SELECT id, body FROM mdc_task WHERE project=$1", [b.id])
    ).rows) {
      b.tasks[r.id] = r.body;
      seen.set(`t:${r.id}`, canonical(r.body));
    }
    for (const r of (
      await db.query("SELECT id, body FROM mdc_attempt WHERE project=$1", [
        b.id,
      ])
    ).rows) {
      const a: Attempt = r.body;
      b.attempts[r.id] = a;
      seen.set(`a:${r.id}`, canonical(a));
      // Spent is the sum of closed attempts (finished, expired or revoked).
      if (a.outcome) b.spent += a.spent;
    }
    return { board: b, seen };
  }
  /**
   * Load a board. Locked, it serializes with every other writer: the lease lock (claims), the
   * board row and the open attempts (heartbeats, finishes).
   */
  private async load(c: Db, id: string, lock: boolean) {
    if (lock) await leaseLock(c, id);
    const row = (
      await c.query(
        `SELECT body FROM mdc_project WHERE id=$1${lock ? " FOR UPDATE" : ""}`,
        [id],
      )
    ).rows[0];
    if (!row) return undefined;
    if (lock)
      await c.query(
        "SELECT id FROM mdc_attempt WHERE project=$1 AND open FOR UPDATE",
        [id],
      );
    return this.assemble(c, row.body);
  }
  private async insertBoard(c: Db, b: Board) {
    const { tasks, attempts, spent: _, ...body } = b;
    await c.query("INSERT INTO mdc_project(id, body, meta) VALUES($1,$2,$3)", [
      b.id,
      body,
      metaOf(b),
    ]);
    await this.putRows(c, b.id, Object.values(tasks), Object.values(attempts));
  }
  private async putRows(
    c: Db,
    project: string,
    tasks: Task[],
    attempts: Attempt[],
  ) {
    if (tasks.length)
      await c.query(
        `INSERT INTO mdc_task(project,id,body) SELECT $1, x->>'id', x FROM jsonb_array_elements($2::jsonb) x
           ON CONFLICT (project,id) DO UPDATE SET body=EXCLUDED.body`,
        [project, JSON.stringify(tasks)],
      );
    if (attempts.length)
      await c.query(
        `INSERT INTO mdc_attempt(project,id,open,body) SELECT $1, x->>'id', NOT (x ? 'outcome'), x FROM jsonb_array_elements($2::jsonb) x
           ON CONFLICT (project,id) DO UPDATE SET body=EXCLUDED.body, open=EXCLUDED.open`,
        [project, JSON.stringify(attempts)],
      );
  }
  async create(b: Board) {
    await this.transaction(async (c) => {
      await this.insertBoard(c, b);
      await this.enqueue(c, b.id, b.owner, "provision", {});
    });
  }
  /**
   * The actor's boards; with a branch, the database's project boards (current first, then
   * superseded) followed by the branch's stage 4 proof requests.
   */
  async list(
    actor: string,
    document?: { database: string; branch: string; node?: string },
  ): Promise<Board[]> {
    const rows = document
      ? (
          await this.pool.query(
            `SELECT p.body FROM mdc_project p WHERE p.body->'members' ? $1 AND (
             (p.body->'project'->>'database'=$2 AND ($4::text IS NULL OR p.body->'branches'->$3->'nodes' ? $4))
             OR (p.body->'request'->>'database'=$2 AND p.body->'request'->>'branch'=$3 AND ($4::text IS NULL OR p.body->'nodes' ? $4)))
           ORDER BY CASE WHEN ${LIVE} THEN 0 WHEN p.body ? 'project' THEN 1 ELSE 2 END, p.id`,
            [
              principalOf(actor),
              document.database,
              document.branch,
              document.node ?? null,
            ],
          )
        ).rows
      : (
          await this.pool.query(
            "SELECT body FROM mdc_project WHERE body->'members' ? $1 ORDER BY id",
            [principalOf(actor)],
          )
        ).rows;
    const out: Board[] = [];
    for (const r of rows)
      out.push((await this.assemble(this.pool, r.body)).board);
    return out;
  }
  /**
   * Boards that can certify a branch's nodes: the database's project boards and the branch's
   * bound proof requests. Goals, facts and branches only: no tasks or attempts.
   */
  async bound(database: string, branch: string): Promise<Board[]> {
    return (
      await this.pool.query(
        `SELECT body FROM mdc_project WHERE body->'project'->>'database'=$1
           OR (body->'request'->>'database'=$1 AND body->'request'->>'branch'=$2 AND (body->'request'->>'bound')::boolean)
         ORDER BY id`,
        [database, branch],
      )
    ).rows.map((r) => r.body);
  }
  /** The database's project board for an environment, if it exists (without tasks or attempts). */
  async project(database: string, environmentId: string) {
    return (
      await this.pool.query(
        "SELECT body FROM mdc_project WHERE body->'project'->>'database'=$1 AND body->'project'->>'environment_id'=$2",
        [database, environmentId],
      )
    ).rows[0]?.body as Board | undefined;
  }
  /**
   * Submit a branch: create the database's project board (its provision job attaches the
   * branch), or attach the branch to the existing board. Replaying (actor, key) with the
   * same request returns the first outcome without queuing another attach.
   */
  async submitBranch(input: {
    actor: string;
    key: string;
    hash: string;
    database: string;
    environmentId: string;
    branch: string;
    create: () => Board;
  }): Promise<{ board: Board; created: boolean }> {
    const { actor, key, hash } = input;
    for (let attempt = 0; ; attempt++)
      try {
        return await this.transaction(async (c) => {
          const found = (
            await c.query(
              "SELECT id FROM mdc_project WHERE body->'project'->>'database'=$1 AND body->'project'->>'environment_id'=$2",
              [input.database, input.environmentId],
            )
          ).rows[0];
          if (!found) {
            const b = input.create();
            await this.insertBoard(c, b);
            await this.enqueue(c, b.id, b.owner, "provision", {});
            await c.query("INSERT INTO mdc_command VALUES($1,$2,$3,$4,$5)", [
              b.id,
              actor,
              key,
              hash,
              { created: true },
            ]);
            return { board: b, created: true };
          }
          const loaded = (await this.load(c, found.id, true))!;
          const b = loaded.board;
          const old = (
            await c.query(
              "SELECT hash,result FROM mdc_command WHERE project=$1 AND actor=$2 AND key=$3",
              [b.id, actor, key],
            )
          ).rows[0];
          if (old) {
            requireThat(old.hash === hash, "idempotency_conflict");
            return { board: b, created: old.result.created === true };
          }
          // Reading main already shows the board exists; joining it is the owner's decision.
          requireThat(
            b.members.includes(principalOf(actor)),
            "not_a_project_member",
            403,
          );
          const out = command(
            b,
            actor,
            { type: "attach", branch: input.branch },
            await this.now(c),
          );
          const job = await this.enqueue(
            c,
            b.id,
            actor,
            out.job!.kind,
            out.job!.payload,
          );
          await this.save(c, loaded, actor, "attach");
          await c.query("INSERT INTO mdc_command VALUES($1,$2,$3,$4,$5)", [
            b.id,
            actor,
            key,
            hash,
            { created: false, job },
          ]);
          return { board: b, created: false };
        });
      } catch (e) {
        // Two first submissions raced; the scope index admitted one board. Attach to it.
        if ((e as { code?: string }).code === "23505" && attempt === 0)
          continue;
        throw e;
      }
  }
  /**
   * After an environment change, the database's boards for other environments stop
   * accepting commands; returning to an earlier environment revives its board, whose
   * goals mean the same again.
   */
  async supersede(
    c: pg.PoolClient,
    database: string,
    environmentId: string,
    actor: string,
  ) {
    const ids = (
      await c.query(
        "SELECT id FROM mdc_project WHERE body->'project'->>'database'=$1 ORDER BY id",
        [database],
      )
    ).rows.map((r) => r.id as string);
    for (const id of ids) {
      const loaded = (await this.load(c, id, true))!;
      const b = loaded.board;
      if (b.project!.removed) continue;
      const superseded = b.project!.environment_id !== environmentId;
      if (Boolean(b.project!.superseded) === superseded) continue;
      if (superseded) b.project!.superseded = true;
      else delete b.project!.superseded;
      await this.save(c, loaded, actor, superseded ? "superseded" : "revived");
    }
  }
  /**
   * The database was removed: its boards stop syncing and can never be revived. They keep their
   * history under a tombstone name (`~` cannot occur in a database name), which also frees the
   * (database, environment) scope for a database later created under the same name.
   */
  async retireDatabase(c: pg.PoolClient, database: string, actor: string) {
    const ids = (
      await c.query(
        "SELECT id FROM mdc_project WHERE body->'project'->>'database'=$1 ORDER BY id",
        [database],
      )
    ).rows.map((r) => r.id as string);
    const at = Math.floor(await this.now(c));
    for (const id of ids) {
      const loaded = (await this.load(c, id, true))!;
      const p = loaded.board.project!;
      p.superseded = true;
      p.removed = { database, at, by: actor };
      p.database = `${database}~removed~${at}`;
      await this.save(c, loaded, actor, "removed");
    }
    return ids;
  }
  private async now(c: Db) {
    return Number(
      (await c.query("SELECT extract(epoch FROM clock_timestamp())*1000 AS ms"))
        .rows[0].ms,
    );
  }
  async get(id: string, actor: string): Promise<Board> {
    const loaded = await this.load(this.pool, id, false);
    requireThat(loaded, "not_found", 404);
    member(loaded.board, actor);
    return loaded.board;
  }
  /** The board's meta for a member: permission checks without reading the board. */
  async meta(id: string, actor: string, db: Db = this.pool): Promise<Meta> {
    const row = (
      await db.query("SELECT meta FROM mdc_project WHERE id=$1", [id])
    ).rows[0];
    requireThat(row?.meta, "not_found", 404);
    const m: Meta = row.meta;
    requireThat(m.members.includes(principalOf(actor)), "not_found", 404);
    return m;
  }
  private async enqueue(
    c: Db,
    project: string,
    actor: string,
    kind: string,
    payload: Record<string, unknown>,
  ) {
    const id = randomUUID();
    await c.query(
      "INSERT INTO mdc_job(id,project,actor,kind,payload) VALUES($1,$2,$3,$4,$5)",
      [id, project, actor, kind, payload],
    );
    return id;
  }
  /**
   * Apply a command. `revision` is what the client read (null for `If-Match: *`); commands in
   * UNCHECKED are checked against the current state instead, and their replays match on the
   * body alone. Commands in LIGHT never write the board document.
   */
  async apply(
    id: string,
    actor: string,
    revision: number | null,
    key: string,
    input: Command,
    grants: Grants = {},
  ) {
    const checked = !UNCHECKED.has(input.type);
    requireThat(!checked || revision !== null, "revision_required", 428);
    const hash = createHash("sha256")
      .update(JSON.stringify(checked ? { revision, input } : { input }))
      .digest("hex");
    const light =
      LIGHT.has(input.type) && !(input.type === "finish" && input.note);
    return this.transaction(async (c) => {
      const replay = async () => {
        const old = (
          await c.query(
            "SELECT hash,result FROM mdc_command WHERE project=$1 AND actor=$2 AND key=$3",
            [id, actor, key],
          )
        ).rows[0];
        if (old) requireThat(old.hash === hash, "idempotency_conflict");
        return old?.result;
      };
      const record = async (response: unknown) =>
        c.query("INSERT INTO mdc_command VALUES($1,$2,$3,$4,$5)", [
          id,
          actor,
          key,
          hash,
          response,
        ]);
      if (light) {
        const m = await this.meta(id, actor, c);
        const old = await replay();
        if (old) return old;
        const out = await this.lease(c, id, m, actor, input);
        const job = out.job
          ? await this.enqueue(c, id, actor, out.job.kind, out.job.payload)
          : undefined;
        await c.query(
          "INSERT INTO mdc_event(project,actor,kind,revision) VALUES($1,$2,$3,$4)",
          [id, actor, input.type, m.revision],
        );
        const response = { revision: m.revision, result: out.result, job };
        await record(response);
        return response;
      }
      const loaded = await this.load(c, id, true);
      requireThat(loaded, "not_found", 404);
      const b = loaded.board;
      member(b, actor);
      const old = await replay();
      if (old) return old;
      if (checked) requireThat(b.revision === revision, "revision_conflict");
      const out = command(b, actor, input, await this.now(c), grants);
      const job = out.job
        ? await this.enqueue(c, id, actor, out.job.kind, out.job.payload)
        : undefined;
      await this.save(c, loaded, actor, input.type);
      const response = { revision: b.revision, result: out.result, job };
      await record(response);
      return response;
    });
  }
  /**
   * The lease path: claim, heartbeat and finish change only their task and attempt rows; the
   * commands that queue a job read the board document without locking or writing it.
   */
  private async lease(
    c: Db,
    id: string,
    m: Meta,
    actor: string,
    input: Command,
  ): Promise<{
    result: unknown;
    job?: { kind: string; payload: Record<string, unknown> };
  }> {
    requireThat(!m.read_only, m.read_only!);
    requireThat(m.ready || input.type === "sync", "project_initializing");
    const now = await this.now(c);
    const row = async <T>(
      table: "mdc_task" | "mdc_attempt",
      key: string,
      lock: boolean,
    ) =>
      (
        await c.query(
          `SELECT body FROM ${table} WHERE project=$1 AND id=$2${lock ? " FOR UPDATE" : ""}`,
          [id, key],
        )
      ).rows[0]?.body as T | undefined;
    const putTask = (t: Task) =>
      c.query("UPDATE mdc_task SET body=$3 WHERE project=$1 AND id=$2", [
        id,
        t.id,
        t,
      ]);
    const putAttempt = (a: Attempt) =>
      c.query(
        `INSERT INTO mdc_attempt(project,id,open,body) VALUES($1,$2,$3,$4)
           ON CONFLICT (project,id) DO UPDATE SET body=EXCLUDED.body, open=EXCLUDED.open`,
        [id, a.id, !a.outcome, a],
      );
    switch (input.type) {
      case "claim": {
        // Claims serialize per project, so that reservations never exceed the budget.
        await leaseLock(c, id);
        await this.expireLeases(c, id, now);
        const t = await row<Task>("mdc_task", input.task, true);
        requireThat(t, "not_found", 404);
        const sums = (
          await c.query(
            `SELECT COALESCE(SUM((body->>'reserved')::bigint) FILTER (WHERE open),0) AS reserved,
                    COALESCE(SUM((body->>'spent')::bigint) FILTER (WHERE NOT open),0) AS spent
               FROM mdc_attempt WHERE project=$1`,
            [id],
          )
        ).rows[0];
        const a = takeLease(t, actor, input, now, {
          active: m.active.includes(t.goal),
          certified: m.certified.includes(t.goal),
          committed: Number(sums.reserved) + Number(sums.spent),
          budget: m.budget,
        });
        await putAttempt(a);
        await putTask(t);
        return { result: a };
      }
      case "heartbeat":
      case "finish": {
        const a = await row<Attempt>("mdc_attempt", input.attempt, true);
        requireThat(a, "not_found", 404);
        const t = (await row<Task>(
          "mdc_task",
          a.task,
          input.type === "finish",
        ))!;
        if (input.type === "heartbeat") renewLease(a, t, actor, input, now);
        else {
          endLease(a, t, actor, input, now);
          await putTask(t);
        }
        await putAttempt(a);
        return { result: a };
      }
      default: {
        // Jobs are validated against the board as it is; nothing in it changes.
        const body = (
          await c.query("SELECT body FROM mdc_project WHERE id=$1", [id])
        ).rows[0].body;
        const b = { ...body, tasks: {}, attempts: {}, spent: 0 } as Board;
        return command(b, actor, input, now);
      }
    }
  }
  /** Record lapsed leases: charged their reservation, their tasks ready again (§4.8). */
  private async expireLeases(c: Db, id: string, now: number) {
    await c.query(
      `WITH e AS (
         UPDATE mdc_attempt SET open=false,
           body = body || jsonb_build_object('outcome','expired','spent', body->'reserved')
         WHERE project=$1 AND open AND (body->>'expires')::numeric <= $2::numeric
         RETURNING id, body->>'task' AS task)
       UPDATE mdc_task t SET body = (t.body - 'attempt') || '{"state":"ready"}'::jsonb
         FROM e WHERE t.project=$1 AND t.id=e.task AND t.body->>'attempt' = e.id`,
      [id, now],
    );
  }
  /** Write the board document and meta, and the tasks and attempts that changed. */
  private async save(c: Db, loaded: Loaded, actor: string, kind: string) {
    const b = loaded.board;
    b.revision++;
    await c.query("UPDATE mdc_project SET body=$2, meta=$3 WHERE id=$1", [
      b.id,
      documentOf(b),
      metaOf(b),
    ]);
    const changed = <T extends { id: string }>(prefix: string, xs: T[]) =>
      xs.filter((x) => loaded.seen.get(`${prefix}${x.id}`) !== canonical(x));
    await this.putRows(
      c,
      b.id,
      changed("t:", Object.values(b.tasks)),
      changed("a:", Object.values(b.attempts)),
    );
    await c.query(
      "INSERT INTO mdc_event(project,actor,kind,revision) VALUES($1,$2,$3,$4)",
      [b.id, actor, kind, b.revision],
    );
  }
  async claim(): Promise<Job | undefined> {
    return this.transaction(async (c) => {
      // A short global claim lock closes the NOT EXISTS race between workers.
      await c.query("SELECT pg_advisory_xact_lock(682193118)");
      const row = (
        await c.query(
          `SELECT j.* FROM mdc_job j WHERE (j.status='queued' AND j.available_at<=now() OR j.status='running' AND j.expires<=now()) AND NOT EXISTS(SELECT 1 FROM mdc_job x WHERE x.project=j.project AND x.id<>j.id AND x.status='running' AND x.expires>now()) ORDER BY j.created_at,j.id FOR UPDATE OF j SKIP LOCKED LIMIT 1`,
        )
      ).rows[0];
      if (!row) return;
      const r = (
        await c.query(
          "UPDATE mdc_job SET status='running',epoch=epoch+1,tries=tries+1,expires=now()+interval '90 seconds' WHERE id=$1 RETURNING *",
          [row.id],
        )
      ).rows[0];
      return r;
    });
  }
  async renew(j: Job) {
    return (
      (
        await this.pool.query(
          "UPDATE mdc_job SET expires=now()+interval '90 seconds' WHERE id=$1 AND epoch=$2 AND status='running' AND expires>now()",
          [j.id, j.epoch],
        )
      ).rowCount === 1
    );
  }
  /** Apply `fn` to the board under the job's claim; `done` ends the job with `result`. */
  async checkpoint(
    j: Job,
    fn: (b: Board) => void,
    done = false,
    result?: unknown,
  ) {
    await this.transaction(async (c) => {
      const lock = (
        await c.query(
          "SELECT id FROM mdc_job WHERE id=$1 AND epoch=$2 AND status='running' AND expires>now() FOR UPDATE",
          [j.id, j.epoch],
        )
      ).rows[0];
      requireThat(lock, "stale_job");
      const loaded = (await this.load(c, j.project, true))!;
      member(loaded.board, j.actor);
      const before = observable(loaded.board);
      fn(loaded.board);
      // A job that changed nothing but its bookkeeping (an idle sync) keeps the revision, so
      // that it does not make every client's next checked command conflict (§2.7.5).
      if (observable(loaded.board) === before)
        await c.query("UPDATE mdc_project SET body=$2 WHERE id=$1", [
          loaded.board.id,
          documentOf(loaded.board),
        ]);
      else await this.save(c, loaded, j.actor, `job:${j.kind}`);
      if (done)
        await c.query(
          "UPDATE mdc_job SET status='done',expires=NULL,error=NULL,details=NULL,result=$2 WHERE id=$1",
          [j.id, result ?? null],
        );
    });
  }
  /** `details` explains `error`, e.g. the Lean messages of a proof LeanGround rejected. */
  async fail(j: Job, error: string, retry: boolean, details?: string) {
    await this.transaction(async (c) => {
      const valid = (
        await c.query(
          "SELECT id FROM mdc_job WHERE id=$1 AND epoch=$2 AND status='running' AND expires>now() FOR UPDATE",
          [j.id, j.epoch],
        )
      ).rows[0];
      if (!valid) return;
      const willRetry = retry && j.tries < JOB_TRIES;
      await c.query(
        "UPDATE mdc_job SET status=$3,error=$4,details=$6,expires=NULL,available_at=now()+make_interval(secs=>$5) WHERE id=$1 AND epoch=$2",
        [
          j.id,
          j.epoch,
          willRetry ? "queued" : "failed",
          error,
          retryDelay(j.tries),
          details ?? null,
        ],
      );
      const loaded = (await this.load(c, j.project, true))!;
      const b = loaded.board;
      if (j.kind === "sync" || j.kind === "provision") b.sync_error = error;
      const name = String(j.payload.branch);
      if (j.kind === "attach" && b.branches && Object.hasOwn(b.branches, name))
        if (!willRetry) b.branches[name].request.error = error;
      if (j.kind === "assemble" && !willRetry) {
        const r = b.runs[String(j.payload.run)];
        if (r) {
          r.status = "failed";
          r.failure = error;
          if (details) r.details = details;
          else delete r.details;
          r.retryable = retry;
        }
      }
      await this.save(c, loaded, j.actor, "job:failed");
    });
  }
  async scheduleSync() {
    await this.transaction(async (c) => {
      await c.query("SELECT pg_advisory_xact_lock(682193119)");
      await c.query(
        `INSERT INTO mdc_job(id,project,actor,kind,payload) SELECT 'sync-'||p.id||'-'||gen_random_uuid(),p.id,p.body->>'owner','sync','{}'::jsonb FROM mdc_project p WHERE p.body->>'ready'='true' AND ${LIVE} AND COALESCE((p.body->>'last_sync')::bigint,0)<extract(epoch FROM now())*1000-15000 AND NOT EXISTS(SELECT 1 FROM mdc_job j WHERE j.project=p.id AND j.kind='sync' AND (j.status IN ('queued','running') OR j.created_at>now()-interval '15 seconds'))`,
      );
    });
  }
  async jobs(id: string, actor: string) {
    await this.meta(id, actor);
    return (
      await this.pool.query(
        "SELECT id,actor,kind,status,tries,error,details,result,created_at FROM mdc_job WHERE project=$1 ORDER BY created_at DESC LIMIT 100",
        [id],
      )
    ).rows;
  }
  async job(id: string, actor: string, job: string) {
    await this.meta(id, actor);
    const row = (
      await this.pool.query(
        "SELECT id,actor,kind,status,tries,error,details,result,created_at FROM mdc_job WHERE project=$1 AND id=$2",
        [id, job],
      )
    ).rows[0];
    requireThat(row, "not_found", 404);
    return row;
  }
  async history(id: string, actor: string) {
    await this.meta(id, actor);
    return (
      await this.pool.query(
        "SELECT sequence,actor,kind,revision,created_at FROM mdc_event WHERE project=$1 ORDER BY sequence DESC LIMIT 100",
        [id],
      )
    ).rows;
  }
}
