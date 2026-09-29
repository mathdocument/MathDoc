import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { randomUUID } from "node:crypto";
import { Store } from "../src/store.js";
import { Worker } from "../src/worker.js";
import { LeanGround } from "../src/remote.js";
import { Documents } from "../src/documents.js";
import { server } from "../src/http.js";
import { ingest } from "../src/domain.js";
import { board, fact, identity } from "./fixtures.js";
import type { Config } from "../src/config.js";
const dsn = process.env.MDC_TEST_DATABASE_URL;
async function isolated() {
  const schema = "test_" + randomUUID().replaceAll("-", "");
  const admin = new Store(dsn!);
  await admin.pool.query(`CREATE SCHEMA ${schema}`);
  const u = new URL(dsn!);
  u.searchParams.set("options", `-c search_path=${schema}`);
  const store = new Store(u.toString());
  await store.migrate();
  return {
    store,
    async close() {
      await store.pool.end();
      await admin.pool.query(`DROP SCHEMA ${schema} CASCADE`);
      await admin.pool.end();
    },
  };
}
async function listen(s: ReturnType<typeof createServer>) {
  s.listen(0, "127.0.0.1");
  await once(s, "listening");
  return `http://127.0.0.1:${(s.address() as { port: number }).port}`;
}
async function close(s: ReturnType<typeof createServer>) {
  s.closeAllConnections();
  await new Promise<void>((r, e) => s.close((err) => (err ? e(err) : r())));
}

test(
  "PostgreSQL concurrent claims, idempotency, restart recovery and job fencing",
  { skip: !dsn },
  async () => {
    const env = await isolated();
    try {
      const { store } = env;
      const b = board();
      await store.create(b);
      const task = Object.values(b.tasks)[0];
      const results = await Promise.allSettled(
        ["alice", "bob"].map((a) =>
          store.apply(b.id, a, 0, `claim-${a}`, {
            type: "claim",
            task: task.id,
            allocation: 60,
            ttl: 30,
          }),
        ),
      );
      assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
      const success = results.findIndex((r) => r.status === "fulfilled");
      const actor = ["alice", "bob"][success];
      const replay = await store.apply(b.id, actor, 0, `claim-${actor}`, {
        type: "claim",
        task: task.id,
        allocation: 60,
        ttl: 30,
      });
      assert.equal(replay.revision, 1);
      await assert.rejects(
        () =>
          store.apply(b.id, actor, 0, `claim-${actor}`, {
            type: "claim",
            task: task.id,
            allocation: 10,
            ttl: 30,
          }),
        /idempotency_conflict/,
      );
      const first = await store.claim();
      assert(first);
      await store.pool.query(
        "UPDATE mdc_job SET expires=now()-interval '1 second' WHERE id=$1",
        [first.id],
      );
      const second = await store.claim();
      assert(second);
      assert.equal(second.id, first.id);
      assert.equal(second.epoch, first.epoch + 1);
      await assert.rejects(
        () =>
          store.checkpoint(
            first,
            (b) => {
              b.notes = "stale";
            },
            true,
          ),
        /stale_job/,
      );
      await store.checkpoint(
        second,
        (b) => {
          b.notes = "recovered";
        },
        true,
      );
      assert.equal((await store.get(b.id, "alice")).notes, "recovered");
    } finally {
      await env.close();
    }
  },
);

test(
  "HTTP identity and revision guards; real PG + HTTP fact synchronization, assembly, safe document writeback",
  { skip: !dsn },
  async () => {
    const env = await isolated();
    const records: Record<string, any> = {
      t: fact("t", "T", ["A", "B"]),
      a: fact("a", "A"),
      b: fact("b", "B"),
    };
    let docVersion = "branch:v1";
    let docWrites = 0;
    const nodeId = randomUUID();
    let node = {
      fnode: nodeId,
      title: "T",
      module: "Lib.Target",
      depens: [],
      blocks: [
        { srctype: "text", content: "Original statement", metadata: {} },
      ] as any[],
    };
    let failAssembly = true;
    let assemblyCalls = 0;
    const upstream = createServer(async (req, res) => {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const body = JSON.parse(Buffer.concat(chunks).toString() || "{}");
      const reply = (data: unknown, status = 200) => {
        res.writeHead(status, {
          "content-type": "application/json",
          "TerminusDB-Data-Version": docVersion,
        });
        res.end(JSON.stringify(data));
      };
      const path = req.url!.split("?")[0];
      if (path.startsWith("/api/document/")) {
        if (req.method === "GET")
          return reply([
            { "@type": "Node", ...node, blocks: JSON.stringify(node.blocks) },
          ]);
        if (req.headers["terminusdb-data-version"] !== docVersion)
          return reply({ error: "DataVersion" }, 409);
        node = { ...body[0], blocks: JSON.parse(body[0].blocks) };
        docVersion = "branch:v2";
        docWrites++;
        return reply([]);
      }
      if (path.startsWith("/v1/bases/"))
        return reply({ base: { base_key: 1, fingerprint: "base" } });
      if (path === "/v1/goals/resolve")
        return reply({ goal_key: identity("T") });
      if (path === "/v1/facts/project") return reply({ id: body.id });
      if (path === "/v1/facts/events")
        return reply({
          cursor: 10,
          has_more: false,
          events:
            body.after < 10
              ? Object.keys(records).map((id, i) => ({
                  sequence: i + 1,
                  kind: "certificate",
                  fact_id: id,
                }))
              : [],
        });
      if (path === "/v1/facts/read")
        return records[body.id]
          ? reply(records[body.id])
          : reply({ reason: "not_found" }, 404);
      if (path === "/v1/facts/plan") return reply({ id: "fixed-plan" });
      if (path === "/v1/facts/widen") return reply({ id: body.id });
      if (path === "/v1/facts/assemble") {
        assemblyCalls++;
        if (failAssembly) {
          failAssembly = false;
          return reply({
            plan_id: "fixed-plan",
            goal: "T",
            status: "failed",
            failure: { reason: "component_unavailable", retryable: true },
          });
        }
        records.final = fact("final", "T");
        return reply({
          plan_id: "fixed-plan",
          goal: "T",
          status: "certified",
          certification_id: "final",
        });
      }
      reply({ reason: "unknown" }, 404);
    });
    const baseUrl = await listen(upstream);
    const lean = new LeanGround(baseUrl, "remote-secret");
    const docs = new Documents(baseUrl, "admin", "password");
    const cfg: Config = {
      dsn: dsn!,
      host: "127.0.0.1",
      port: 17843,
      actors: {
        alice: { token: "alice-secret-token", admin: true },
        bob: { token: "bob-secret-token", admin: false },
        eve: { token: "eve-secret-token", admin: false },
      },
      leanUrl: baseUrl,
      leanToken: "remote-secret",
      leanActor: "coordinator",
      terminusUser: "admin",
      appDir: "app/dist",
    };
    const api = server(cfg, env.store, lean, docs);
    const apiUrl = await listen(api);
    try {
      const b = board();
      b.node_ref = {
        database: "docs",
        branch: "main",
        node: nodeId,
        revision: docVersion,
      };
      await env.store.create(b);
      const request = (
        path: string,
        token: string,
        method = "GET",
        body?: unknown,
        headers = {},
      ) =>
        fetch(`${apiUrl}/api${path === "/me" ? "" : "/coordination"}${path}`, {
          method,
          headers: {
            authorization: `Bearer ${token}`,
            "content-type": "application/json",
            ...headers,
          },
          body: body ? JSON.stringify(body) : undefined,
        });
      assert.equal(
        (await request("/projects/project", "eve-secret-token")).status,
        404,
      );
      assert.equal((await request("/projects/project", "bad")).status, 401);
      const spoof = await request(
        "/projects/project/commands",
        "bob-secret-token",
        "POST",
        { type: "notes", notes: "fake", actor: "alice" },
        { "if-match": '"0"', "idempotency-key": "spoof" },
      );
      assert.equal(spoof.status, 400);
      const worker = new Worker(env.store, lean, docs, "coordinator");
      await worker.tick();
      await worker.tick();
      let state = await env.store.get("project", "alice");
      assert.equal(state.goals.T.status, "derivable");
      assert.equal(Object.keys(state.goals).length, 3);
      assert.equal(state.cursor, 10);
      const assemble = await env.store.apply(
        "project",
        "alice",
        state.revision,
        "assemble",
        { type: "assemble", component: "lean-worker" },
      );
      await worker.tick();
      state = await env.store.get("project", "alice");
      const failed = state.runs[assemble.result.id];
      assert.equal(failed.status, "failed");
      assert(failed.retryable);
      const retried = await env.store.apply(
        "project",
        "alice",
        state.revision,
        "retry",
        { type: "retry", run: failed.id },
      );
      await worker.tick();
      state = await env.store.get("project", "alice");
      assert.equal(state.goals.T.status, "certified");
      assert.equal(state.runs[retried.result.id].plan_id, "fixed-plan");
      assert.equal(assemblyCalls, 2);
      await env.store.apply("project", "alice", state.revision, "accept", {
        type: "accept",
        run: retried.result.id,
      });
      await worker.tick();
      state = await env.store.get("project", "alice");
      assert.equal(state.accepted?.certification_id, "final");
      assert.equal(docWrites, 1);
      assert.equal(
        node.blocks.find((b) => b.srctype === "lean").metadata.certification_id,
        "final",
      );
      // Exact-operation retry recovers a write with a lost response; another accept
      // cannot overwrite a modified document version.
      const operation = state.accepted!.operation;
      await docs.accept(
        b.node_ref!,
        records.final.source,
        "final",
        operation,
        "alice",
      );
      assert.equal(docWrites, 1);
      await assert.rejects(
        () =>
          docs.accept(
            b.node_ref!,
            records.final.source,
            "final",
            "different-operation",
            "alice",
          ),
        /document_revision_conflict/,
      );
      delete records.a;
      await env.store.apply("project", "alice", state.revision, "sync-revoke", {
        type: "sync",
      });
      await worker.tick();
      state = await env.store.get("project", "alice");
      assert.equal(state.facts.a.available, false);
    } finally {
      await close(api);
      await close(upstream);
      await env.close();
    }
  },
);

test(
  "worker applies events and cursor atomically; rejected upstream batches cannot lose events",
  { skip: !dsn },
  async () => {
    const env = await isolated();
    const b = board();
    await env.store.create(b);
    const init = await env.store.claim();
    assert(init);
    await env.store.checkpoint(init, () => {}, true);
    const upstream = createServer(async (req, res) => {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const input = JSON.parse(Buffer.concat(chunks).toString() || "{}");
      res.setHeader("content-type", "application/json");
      if (req.url === "/v1/facts/events")
        res.end(
          JSON.stringify({
            cursor: 20,
            has_more: false,
            events: [
              { sequence: 1, kind: "certificate", fact_id: "first" },
              { sequence: 2, kind: "certificate", fact_id: "broken" },
            ],
          }),
        );
      else if (input.id === "first")
        res.end(JSON.stringify(fact("first", "T", ["A"])));
      else res.end(JSON.stringify({ id: "broken", bad: "contract" }));
    });
    const url = await listen(upstream);
    try {
      await env.store.apply("project", "alice", 1, "sync", { type: "sync" });
      const worker = new Worker(
        env.store,
        new LeanGround(url, "token"),
        new Documents(),
        "coordinator",
      );
      await worker.tick();
      const state = await env.store.get("project", "alice");
      assert.equal(state.cursor, 0);
      assert.equal(Object.keys(state.facts).length, 0);
      assert.equal(state.sync_error, "invalid_upstream_contract");
    } finally {
      await close(upstream);
      await env.close();
    }
  },
);
