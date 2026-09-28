import pg from "pg";
import { randomUUID, createHash } from "node:crypto";
import { Board, Command, command, member, requireThat } from "./domain.js";
export interface Job {
  id: string;
  project: string;
  actor: string;
  kind: string;
  payload: Record<string, unknown>;
  epoch: number;
  tries: number;
}
export const migration = `
CREATE TABLE IF NOT EXISTS mdc_project(id text PRIMARY KEY, body jsonb NOT NULL);
CREATE TABLE IF NOT EXISTS mdc_command(project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, key text NOT NULL, hash text NOT NULL, result jsonb NOT NULL, PRIMARY KEY(project,actor,key));
CREATE TABLE IF NOT EXISTS mdc_event(sequence bigserial PRIMARY KEY, project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, kind text NOT NULL, revision bigint NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS mdc_job(id text PRIMARY KEY, project text NOT NULL REFERENCES mdc_project(id), actor text NOT NULL, kind text NOT NULL, payload jsonb NOT NULL, status text NOT NULL DEFAULT 'queued', epoch integer NOT NULL DEFAULT 0, tries integer NOT NULL DEFAULT 0, expires timestamptz, available_at timestamptz NOT NULL DEFAULT now(), error text, created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX IF NOT EXISTS mdc_job_queue ON mdc_job(status,available_at);
`;
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
  async create(b: Board) {
    await this.transaction(async (c) => {
      await c.query("INSERT INTO mdc_project VALUES($1,$2)", [b.id, b]);
      await this.enqueue(c, b.id, b.owner, "provision", {});
    });
  }
  async list(actor: string): Promise<Board[]> {
    return (
      await this.pool.query(
        "SELECT body FROM mdc_project WHERE body->'members' ? $1 ORDER BY id",
        [actor],
      )
    ).rows.map((r) => r.body);
  }
  async get(id: string, actor: string): Promise<Board> {
    const row = (
      await this.pool.query("SELECT body FROM mdc_project WHERE id=$1", [id])
    ).rows[0];
    requireThat(row, "not_found", 404);
    member(row.body, actor);
    return row.body;
  }
  private async enqueue(
    c: pg.PoolClient,
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
  async apply(
    id: string,
    actor: string,
    revision: number,
    key: string,
    input: Command,
  ) {
    const hash = createHash("sha256")
      .update(JSON.stringify({ revision, input }))
      .digest("hex");
    return this.transaction(async (c) => {
      const row = (
        await c.query("SELECT body FROM mdc_project WHERE id=$1 FOR UPDATE", [
          id,
        ])
      ).rows[0];
      requireThat(row, "not_found", 404);
      const b: Board = row.body;
      member(b, actor);
      const old = (
        await c.query(
          "SELECT hash,result FROM mdc_command WHERE project=$1 AND actor=$2 AND key=$3",
          [id, actor, key],
        )
      ).rows[0];
      if (old) {
        requireThat(old.hash === hash, "idempotency_conflict");
        return old.result;
      }
      requireThat(b.revision === revision, "revision_conflict");
      const now = Number(
        (
          await c.query(
            "SELECT extract(epoch FROM clock_timestamp())*1000 AS ms",
          )
        ).rows[0].ms,
      );
      const out = command(b, actor, input, now);
      const job = out.job
        ? await this.enqueue(c, id, actor, out.job.kind, out.job.payload)
        : undefined;
      await this.save(c, b, actor, input.type);
      const response = { revision: b.revision, result: out.result, job };
      await c.query("INSERT INTO mdc_command VALUES($1,$2,$3,$4,$5)", [
        id,
        actor,
        key,
        hash,
        response,
      ]);
      return response;
    });
  }
  private async save(c: pg.PoolClient, b: Board, actor: string, kind: string) {
    b.revision++;
    await c.query("UPDATE mdc_project SET body=$2 WHERE id=$1", [b.id, b]);
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
  async checkpoint(j: Job, fn: (b: Board) => void, done = false) {
    await this.transaction(async (c) => {
      const lock = (
        await c.query(
          "SELECT id FROM mdc_job WHERE id=$1 AND epoch=$2 AND status='running' AND expires>now() FOR UPDATE",
          [j.id, j.epoch],
        )
      ).rows[0];
      requireThat(lock, "stale_job");
      const b: Board = (
        await c.query("SELECT body FROM mdc_project WHERE id=$1 FOR UPDATE", [
          j.project,
        ])
      ).rows[0].body;
      member(b, j.actor);
      fn(b);
      await this.save(c, b, j.actor, `job:${j.kind}`);
      if (done)
        await c.query(
          "UPDATE mdc_job SET status='done',expires=NULL,error=NULL WHERE id=$1",
          [j.id],
        );
    });
  }
  async fail(j: Job, error: string, retry: boolean) {
    await this.transaction(async (c) => {
      const valid = (
        await c.query(
          "SELECT id FROM mdc_job WHERE id=$1 AND epoch=$2 AND status='running' AND expires>now() FOR UPDATE",
          [j.id, j.epoch],
        )
      ).rows[0];
      if (!valid) return;
      const willRetry = retry && j.tries < 5;
      await c.query(
        "UPDATE mdc_job SET status=$3,error=$4,expires=NULL,available_at=now()+interval '5 seconds' WHERE id=$1 AND epoch=$2",
        [j.id, j.epoch, willRetry ? "queued" : "failed", error],
      );
      const b: Board = (
        await c.query("SELECT body FROM mdc_project WHERE id=$1 FOR UPDATE", [
          j.project,
        ])
      ).rows[0].body;
      if (j.kind === "sync" || j.kind === "provision") b.sync_error = error;
      if (j.kind === "assemble" && !willRetry) {
        const r = b.runs[String(j.payload.run)];
        if (r) {
          r.status = "failed";
          r.failure = error;
          r.retryable = retry;
        }
      }
      await this.save(c, b, j.actor, "job:failed");
    });
  }
  async scheduleSync() {
    await this.transaction(async (c) => {
      await c.query("SELECT pg_advisory_xact_lock(682193119)");
      await c.query(
        `INSERT INTO mdc_job(id,project,actor,kind,payload) SELECT 'sync-'||p.id||'-'||gen_random_uuid(),p.id,p.body->>'owner','sync','{}'::jsonb FROM mdc_project p WHERE p.body->>'ready'='true' AND COALESCE((p.body->>'last_sync')::bigint,0)<extract(epoch FROM now())*1000-15000 AND NOT EXISTS(SELECT 1 FROM mdc_job j WHERE j.project=p.id AND j.kind='sync' AND (j.status IN ('queued','running') OR j.created_at>now()-interval '15 seconds'))`,
      );
    });
  }
  async jobs(id: string, actor: string) {
    await this.get(id, actor);
    return (
      await this.pool.query(
        "SELECT id,actor,kind,status,tries,error,created_at FROM mdc_job WHERE project=$1 ORDER BY created_at DESC LIMIT 100",
        [id],
      )
    ).rows;
  }
  async history(id: string, actor: string) {
    await this.get(id, actor);
    return (
      await this.pool.query(
        "SELECT sequence,actor,kind,revision,created_at FROM mdc_event WHERE project=$1 ORDER BY sequence DESC LIMIT 100",
        [id],
      )
    ).rows;
  }
}
