// Project stage 5 (docs/mathdoc.md §2.7.5): tasks and attempts in their own rows, the lease path,
// the migration of boards written before, and syncs that skip unchanged branches. Real
// PostgreSQL (and TerminusDB for the sync test); LeanGround is the contract double.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { Store } from "../src/store.js";
import { Terminus } from "../src/docs/terminus.js";
import { addGoal, type Board } from "../src/domain.js";
import { board } from "./fixtures.js";
import { client, deploy, env, isolated, removeDatabases } from "./harness.js";
import { leangroundDouble } from "./leanground-double.js";

const dsn = env.dsn;
const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);

/** A ready board with `n` open goals, each with its prove task. */
async function seeded(store: Store, n: number, budget: number) {
  const b = board(`p-${randomUUID().slice(0, 8)}`);
  b.budget = budget;
  b.members = ["alice", ...Array.from({ length: 10 }, (_, i) => `agent${i}`)];
  for (let i = 0; i < n; i++) addGoal(b, `G${i}`, undefined, `goal ${i}`);
  // Every goal is a premise of the root's active decomposition, so every task can be claimed.
  b.facts.d = {
    id: "d",
    goal: b.root,
    premises: Array.from({ length: n }, (_, i) => `G${i}`),
    trust: "trusted",
    state: "active",
    available: true,
  };
  await store.create(b);
  const task = (goal: string) =>
    Object.values(b.tasks).find((t) => t.goal === goal)!.id;
  return { b, task };
}

test(
  "the lease path: concurrent claims, budget, expiry and revisions on separate rows",
  { skip: !dsn },
  async () => {
    const schema = await isolated();
    const store = new Store(schema.url);
    await store.migrate();
    try {
      const { b, task } = await seeded(store, 11, 3);
      const claim = (who: string, goal: string, allocation = 1) =>
        store.apply(b.id, who, null, randomUUID(), {
          type: "claim",
          task: task(goal),
          allocation,
          ttl: 60,
        });
      // Ten agents race for one task: one lease.
      const same = await Promise.allSettled(
        Array.from({ length: 10 }, (_, i) => claim(`agent${i}`, "G0")),
      );
      assert.equal(same.filter((r) => r.status === "fulfilled").length, 1);
      assert.ok(
        same.every(
          (r) =>
            r.status === "fulfilled" ||
            /task_unavailable/.test(String(r.reason)),
        ),
      );
      // Ten agents claim ten tasks at once: the budget of 3 admits exactly two more.
      const many = await Promise.allSettled(
        Array.from({ length: 9 }, (_, i) => claim(`agent${i}`, `G${i + 1}`)),
      );
      assert.equal(many.filter((r) => r.status === "fulfilled").length, 2);
      let p = await store.get(b.id, "alice");
      assert.equal(
        Object.values(p.attempts).filter((a) => !a.outcome).length,
        3,
      );
      assert.equal(p.revision, 0, "leases do not change the board's revision");
      // Board commands still compare revisions; `*` is refused for them.
      await assert.rejects(
        store.apply(b.id, "alice", null, "n1", { type: "notes", notes: "x" }),
        /revision_required/,
      );
      await assert.rejects(
        store.apply(b.id, "alice", 7, "n2", { type: "notes", notes: "x" }),
        /revision_conflict/,
      );
      await store.apply(b.id, "alice", 0, "n3", { type: "notes", notes: "x" });
      // A replay matches on the body, whatever revision it names.
      const first = await store.apply(b.id, "agent9", null, "hb", {
        type: "claim",
        task: task("G10"),
        allocation: 0,
        ttl: 60,
      });
      const again = await store.apply(b.id, "agent9", 5, "hb", {
        type: "claim",
        task: task("G10"),
        allocation: 0,
        ttl: 60,
      });
      assert.deepEqual(
        [again.result, again.revision],
        [first.result, first.revision],
      );
      // A lapsed lease is charged its reservation and frees its task at the next claim.
      p = await store.get(b.id, "alice");
      const held = Object.values(p.attempts).find(
        (a) => !a.outcome && p.tasks[a.task].goal === "G0",
      )!;
      await store.pool.query(
        "UPDATE mdc_attempt SET body = body || jsonb_build_object('expires', 1) WHERE id=$1",
        [held.id],
      );
      await claim("agent5", "G0", 0);
      p = await store.get(b.id, "alice");
      assert.equal(p.attempts[held.id].outcome, "expired");
      assert.equal(p.attempts[held.id].spent, 1);
      assert.equal(p.spent, 1, "spent is the sum of closed attempts");
      // Heartbeat and finish change only their attempt; the history records each.
      const mine = Object.values(p.attempts).find(
        (a) => !a.outcome && a.actor === "agent5",
      )!;
      await store.apply(b.id, "agent5", null, "h", {
        type: "heartbeat",
        attempt: mine.id,
        epoch: mine.epoch,
        ttl: 120,
      });
      await store.apply(b.id, "agent5", null, "f", {
        type: "finish",
        attempt: mine.id,
        epoch: mine.epoch,
        spent: 0,
        outcome: "no_progress",
      });
      p = await store.get(b.id, "alice");
      assert.equal(p.attempts[mine.id].outcome, "no_progress");
      assert.equal(p.tasks[mine.task].state, "ready");
      assert.equal(p.revision, 1, "only the notes command wrote the board");
      const kinds = (await store.history(b.id, "alice")).map(
        (e: { kind: string }) => e.kind,
      );
      for (const k of ["claim", "heartbeat", "finish", "notes"])
        assert.ok(kinds.includes(k), k);
    } finally {
      await store.pool.end();
      await schema.close();
    }
  },
);

test(
  "a job that writes back equal values in another key order keeps the revision",
  { skip: !dsn },
  async () => {
    // jsonb stores object keys in its own order; a sync receives the same goal identity from
    // LeanGround in another one. Neither is a change a client could act on (§2.7.7).
    const schema = await isolated();
    const store = new Store(schema.url);
    await store.migrate();
    try {
      const b = board(`p-${randomUUID().slice(0, 8)}`);
      await store.create(b);
      await store.pool.query("DELETE FROM mdc_job WHERE project=$1", [b.id]);
      const job = async () => {
        await store.apply(b.id, "alice", null, randomUUID(), { type: "sync" });
        return (await store.claim())!;
      };
      const revision = async () => (await store.get(b.id, "alice")).revision;
      const before = await revision();
      await store.checkpoint(
        await job(),
        (x) => {
          const g = x.goals[x.root];
          g.identity = Object.fromEntries(
            Object.entries(g.identity!).reverse(),
          ) as typeof g.identity;
          x.last_sync = Date.now();
        },
        true,
      );
      assert.equal(await revision(), before, "same values, other key order");
      await store.checkpoint(
        await job(),
        (x) => {
          x.goals[x.root].statement = "changed";
        },
        true,
      );
      assert.equal(await revision(), before + 1, "a real change still counts");
    } finally {
      await store.pool.end();
      await schema.close();
    }
  },
);

test(
  "migrate moves tasks and attempts of boards written before into their rows",
  { skip: !dsn },
  async () => {
    const schema = await isolated();
    const store = new Store(schema.url);
    await store.migrate();
    try {
      const old: Board = board("legacy-board");
      old.attempts.a1 = {
        id: "a1",
        task: Object.keys(old.tasks)[0],
        actor: "bob",
        epoch: 1,
        expires: 5,
        reserved: 4,
        spent: 3,
        outcome: "submitted",
      };
      old.spent = 3;
      // As stored before stage 5: one document, no meta.
      await store.pool.query(
        "INSERT INTO mdc_project(id, body) VALUES($1,$2)",
        [old.id, old],
      );
      await store.migrate();
      await store.migrate(); // idempotent
      const row = (
        await store.pool.query(
          "SELECT body, meta FROM mdc_project WHERE id=$1",
          [old.id],
        )
      ).rows[0];
      assert.ok(!("tasks" in row.body) && !("attempts" in row.body));
      assert.equal(row.meta.revision, old.revision);
      const p = await store.get(old.id, "alice");
      assert.deepEqual(p.tasks, old.tasks);
      assert.deepEqual(p.attempts, old.attempts);
      assert.equal(p.spent, 3);
    } finally {
      await store.pool.end();
      await schema.close();
    }
  },
);

test(
  "a sync skips branches whose data version did not change",
  { skip: !documents, timeout: 120000 },
  async () => {
    const schema = await isolated();
    const lean = await leangroundDouble();
    const dep = await deploy(schema.url, { url: lean.url, token: "double" });
    const db = `sync${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    const http = client(() => dep, db);
    const load = Terminus.prototype.database;
    let loads = 0;
    // Count full document loads made through any database handle.
    Terminus.prototype.database = function (...a: Parameters<typeof load>) {
      const d = load.apply(this, a);
      const original = d.load.bind(d);
      d.load = () => {
        loads++;
        return original();
      };
      return d;
    };
    try {
      await http.ok("alice", "POST", "/api/projects", {
        action: "init",
        name: db,
      });
      const root = await http.node(
        "root",
        "theorem root_thm : 1 = 1 := sorry\n",
      );
      await http.ok("alice", "PUT", `/api/coordination/environments/${db}`, {
        base_key: 1,
      });
      const id = (
        await http.ok(
          "alice",
          "POST",
          "/api/coordination/projects",
          { document: { database: db, branch: "main", node: root }, budget: 1 },
          { "idempotency-key": "c" },
        )
      ).id as string;
      await dep.drain();
      loads = 0;
      const idle = (await http.project(id)).revision;
      await http.done(id, "alice", { type: "sync" });
      assert.equal(loads, 0, "unchanged branch: no snapshot load");
      assert.equal(
        (await http.project(id)).revision,
        idle,
        "an idle sync keeps the revision, so checked commands do not conflict",
      );
      await http.setLean(root, "theorem root_thm : 1 = 1 := by\n  rfl\n");
      loads = 0; // the edit itself goes through the document API
      await http.done(id, "alice", { type: "sync" });
      assert.equal(loads, 1, "changed branch: loaded once");
      const p = await http.project(id);
      assert.equal(p.branches!.main.request.stale, true);
      loads = 0;
      await http.done(id, "alice", { type: "sync" });
      assert.equal(loads, 0);
      assert.equal((await http.project(id)).branches!.main.request.stale, true);
    } finally {
      Terminus.prototype.database = load;
      await removeDatabases(db);
      await dep.close();
      await lean.close();
      await schema.close();
    }
  },
);
