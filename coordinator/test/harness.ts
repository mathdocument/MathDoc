// Shared by the tests that run against a real LeanGround, TerminusDB and PostgreSQL.
// They are skipped unless MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL,
// MDC_TEST_TERMINUS_PASSWORD, MDC_TEST_LEANGROUND_URL and MDC_TEST_LEANGROUND_TOKEN are set.
import assert from "node:assert/strict";
import { once } from "node:events";
import { randomUUID } from "node:crypto";
import type { Server } from "node:http";
import { Store } from "../src/store.js";
import { Worker } from "../src/worker.js";
import { LeanGround } from "../src/remote.js";
import { Documents } from "../src/documents.js";
import { Terminus } from "../src/docs/terminus.js";
import { server } from "../src/http.js";
import type { Board } from "../src/domain.js";
import type { Config } from "../src/config.js";

export const env = {
  dsn: process.env.MDC_TEST_DATABASE_URL,
  terminus: process.env.MDC_TEST_TERMINUS_URL,
  terminusPassword: process.env.MDC_TEST_TERMINUS_PASSWORD,
  lean: process.env.MDC_TEST_LEANGROUND_URL,
  leanToken: process.env.MDC_TEST_LEANGROUND_TOKEN,
};
export const ready = Object.values(env).every(Boolean);

export const actors = {
  alice: { token: "alice-scenario-token", admin: true },
  bob: { token: "bob-scenario-token", admin: false },
  carol: { token: "carol-scenario-token", admin: false },
  eve: { token: "eve-scenario-token", admin: false },
};
export type Actor = keyof typeof actors;

export async function isolated() {
  const schema = "scenario_" + randomUUID().replaceAll("-", "");
  const admin = new Store(env.dsn!);
  await admin.pool.query(`CREATE SCHEMA ${schema}`);
  const u = new URL(env.dsn!);
  u.searchParams.set("options", `-c search_path=${schema}`);
  return {
    url: u.toString(),
    async close() {
      await admin.pool.query(`DROP SCHEMA ${schema} CASCADE`);
      await admin.pool.end();
    },
  };
}

/** One coordinator deployment: API process state and a worker. Recreated to simulate restarts. */
export async function deploy(url: string) {
  const store = new Store(url);
  await store.migrate();
  const lean = new LeanGround(env.lean!, env.leanToken!);
  const cfg: Config = {
    dsn: url,
    host: "127.0.0.1",
    port: 0,
    actors,
    leanUrl: env.lean!,
    leanToken: env.leanToken!,
    leanActor: "coordinator",
    terminusUrl: env.terminus,
    terminusUser: "admin",
    terminusPassword: env.terminusPassword,
    webDir: "/nonexistent",
  };
  const http: Server = server(cfg, store, lean, new Documents());
  http.listen(0, "127.0.0.1");
  await once(http, "listening");
  const base = `http://127.0.0.1:${(http.address() as { port: number }).port}`;
  const terminus = new Terminus({
    url: env.terminus!,
    user: "admin",
    password: env.terminusPassword!,
  });
  const worker = new Worker(
    store,
    lean,
    new Documents(),
    "coordinator",
    terminus,
  );
  return {
    store,
    base,
    /** Run every queued job; periodic syncs are not waited for. */
    async drain() {
      while (await worker.tick());
    },
    async close() {
      http.closeAllConnections();
      await new Promise<void>((r) => http.close(() => r()));
      await store.pool.end();
    },
  };
}

export type Deployment = Awaited<ReturnType<typeof deploy>>;

/** HTTP helpers for one document database; `current` follows redeployments. */
export function client(current: () => Deployment, db: string) {
  const dep = {
    get base() {
      return current().base;
    },
    drain: () => current().drain(),
  };
  const call = async (
    who: Actor,
    method: string,
    path: string,
    body?: unknown,
    headers: Record<string, string> = {},
  ) => {
    const r = await fetch(`${dep.base}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${actors[who].token}`,
        "content-type": "application/json",
        ...headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await r.text();
    return { status: r.status, body: text ? JSON.parse(text) : null };
  };
  const ok = async (...args: Parameters<typeof call>) => {
    const r = await call(...args);
    assert.ok(
      r.status < 300,
      `${args[1]} ${args[2]} → ${r.status} ${JSON.stringify(r.body)}`,
    );
    return r.body;
  };
  const doc = (
    who: Actor,
    method: string,
    route: string,
    body?: unknown,
    revision?: string,
  ) =>
    ok(
      who,
      method,
      `/p/${db}/main/api${route}`,
      body,
      revision ? { "if-match": `"${revision}"` } : {},
    );
  const node = async (title: string, lean: string, deps: string[] = []) => {
    let n = await doc("alice", "POST", "/node/new", { title });
    n = await doc(
      "alice",
      "PUT",
      `/node/${n.fnode}/block/lean`,
      { content: lean },
      n.revision,
    );
    for (const d of deps)
      n = await doc(
        "alice",
        "POST",
        `/node/${n.fnode}/dep/add`,
        { dep_fnode: d },
        n.revision,
      );
    return n.fnode as string;
  };
  const view = async (id: string) =>
    (await doc("alice", "GET", `/node/${id}/view`)).node;
  const setLean = async (id: string, content: string) => {
    const n = await view(id);
    await doc(
      "alice",
      "PUT",
      `/node/${id}/block/lean`,
      { content },
      n.revision,
    );
  };
  let keys = 0;
  const project = async (id: string, who: Actor = "alice"): Promise<Board> =>
    ok(who, "GET", `/api/coordination/projects/${id}`);
  const command = async (
    id: string,
    who: Actor,
    c: unknown,
    key = `k${keys++}`,
  ) => {
    const b = await project(id, who);
    return call(who, "POST", `/api/coordination/projects/${id}/commands`, c, {
      "if-match": `"${b.revision}"`,
      "idempotency-key": key,
    });
  };
  const done = async (id: string, who: Actor, c: unknown) => {
    const r = await command(id, who, c);
    assert.ok(
      r.status < 300,
      `${JSON.stringify(c)} → ${r.status} ${JSON.stringify(r.body)}`,
    );
    await dep.drain();
    if (r.body.job) {
      const jobs = await ok(
        who,
        "GET",
        `/api/coordination/projects/${id}/jobs`,
      );
      const job = jobs.find((x: { id: string }) => x.id === r.body.job);
      assert.equal(
        job.status,
        "done",
        `${JSON.stringify(c).slice(0, 200)} → ${job.error}`,
      );
    }
    return r.body;
  };
  return { call, ok, doc, node, view, setLean, project, command, done };
}
