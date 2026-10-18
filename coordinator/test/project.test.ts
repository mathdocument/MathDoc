// Project stage 1 (docs/mathdoc.md §2.7, §3.3): one project board per database, branches
// as routes over the shared proof graph. Real PostgreSQL and TerminusDB; LeanGround is the
// contract double of leanground-double.ts, so nothing here checks a Lean proof. Skipped
// unless MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL and MDC_TEST_TERMINUS_PASSWORD are set.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import {
  client,
  deploy,
  env,
  isolated,
  removeDatabases,
  type Actor,
} from "./harness.js";
import { leangroundDouble } from "./leanground-double.js";
import { newProjectBoard, type Board } from "../src/domain.js";

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);

const D = "def double (n : Nat) : Nat := n + n\n";
const B = "theorem double_eq (n : Nat) : double n = n + n := sorry\n";
const A1 = "theorem double_two : double 2 = 4 := sorry\n";
const A2 = "theorem double_three : double 3 = 6 := sorry\n";
// Decomposition 1 (branch main): the root from A1 and A2.
const R1 =
  "theorem sum_doubles : double 2 + double 3 = 10 := by\n  rw [double_two, double_three]\n";
// Decomposition 2 (branch alt): the root directly from the shared lemma B.
const R2 =
  "theorem sum_doubles : double 2 + double 3 = 10 := by\n  rw [double_eq, double_eq]\n";

test(
  "project board: branches share goals, choose their own routes, and stay readable when retired",
  { skip: !documents, timeout: 120000 },
  async () => {
    const schema = await isolated();
    const lean = await leangroundDouble();
    const db = `proj${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    const dep = await deploy(schema.url, { url: lean.url, token: "double" });
    const main = client(() => dep, db);
    const alt = client(() => dep, db, "alt");
    const { call, ok, done, project, command } = main;
    const directory = (who: Actor, body: Record<string, unknown>) =>
      ok(who, "POST", "/api/projects", body);
    const submit = (who: Actor, branch: string, node: string, key: string) =>
      call(
        who,
        "POST",
        "/api/coordination/projects",
        {
          document: { database: db, branch, node },
          members: ["bob"],
          budget: 1000,
        },
        { "idempotency-key": key },
      );
    const certification = async (c: typeof main, id: string) =>
      (await c.view(id)).formalization.lean_certification;
    try {
      await directory("alice", { action: "init", name: db });
      await directory("alice", {
        action: "grant",
        project: `${db}/main`,
        actor: "bob",
        role: "editor",
      });
      const d = await main.node("double", D);
      const b = await main.node("double_eq", B, [d]);
      const a1 = await main.node("double_two", A1, [b, d]);
      const a2 = await main.node("double_three", A2, [b, d]);
      const r = await main.node("sum_doubles", R1, [a1, a2, d]);

      // The environment belongs to the database: owners set it, members read it.
      const envPath = `/api/coordination/environments/${db}`;
      assert.equal(
        (await call("bob", "PUT", envPath, { base_key: 1 })).status,
        403,
      );
      await ok("alice", "PUT", envPath, { base_key: 1 });
      assert.equal((await ok("bob", "GET", envPath)).base_key, 1);

      // Creating the project fixes the database's root theorem: owners only.
      assert.equal((await submit("bob", "main", r, "bob-1")).status, 403);
      const created = await submit("alice", "main", r, "alice-1");
      assert.equal(created.status, 201, JSON.stringify(created.body));
      const id = created.body.id as string;
      const replay = await submit("alice", "main", r, "alice-1");
      assert.equal(
        replay.status,
        201,
        "a replayed creation returns the first outcome",
      );
      assert.equal(replay.body.id, id);
      await dep.drain();
      let p = await project(id);
      assert.ok(
        p.ready && p.branches!.main.request.bound,
        JSON.stringify(p.branches),
      );
      const goal = (branch: string, n: string) =>
        p.branches![branch].nodes[n].goal!;
      const d1 = p.branches!.main.nodes[r].certificate!;
      assert.equal(
        p.facts[d1].premises.length,
        2,
        "main proves the root from A1 and A2",
      );
      assert.deepEqual(p.branches!.main.picks, { [p.root]: d1 });

      // A second branch with another decomposition of the same root.
      await directory("bob", {
        action: "new_branch",
        project: `${db}/main`,
        name: "alt",
      });
      // Its root uses B directly; A1 and A2 are no longer premises there.
      await alt.setLean(r, R2);
      const added = await alt.doc(
        "alice",
        "POST",
        `/node/${r}/dep/add`,
        { dep_fnode: b },
        (await alt.view(r)).revision,
      );
      await alt.doc(
        "alice",
        "POST",
        `/node/${r}/dep/rm`,
        { dep_fnodes: [a1, a2] },
        added.revision,
      );
      assert.equal(
        (await submit("bob", "alt", r, "bob-alt")).status,
        200,
        "an editor attaches a branch",
      );
      await dep.drain();
      p = await project(id);
      assert.ok(
        p.branches!.alt.request.bound,
        JSON.stringify(p.branches!.alt.request),
      );
      const d2 = p.branches!.alt.nodes[r].certificate!;
      assert.deepEqual(
        p.facts[d2].premises,
        [goal("alt", b)],
        "alt proves the root from B",
      );
      assert.equal(goal("alt", r), p.root, "both branches prove one root goal");
      assert.equal(
        goal("alt", b),
        goal("main", b),
        "the lemma B is one goal on both branches",
      );
      assert.equal(
        Object.values(p.tasks).filter((t) => t.goal === goal("main", b)).length,
        1,
      );
      assert.equal(p.facts[d1].state, "active");

      // Acceptance: B proved on main shows as certified on alt, without any import.
      assert.equal((await certification(alt, b)).status, "insufficient");
      await main.setLean(b, B.replace("sorry", "rfl"));
      await done(id, "bob", { type: "submit_node", branch: "main", node: b });
      p = await project(id);
      assert.equal(p.goals[goal("main", b)].status, "certified");
      assert.equal((await certification(main, b)).status, "certified");
      const onAlt = await certification(alt, b);
      assert.equal(onAlt.status, "certified", JSON.stringify(onAlt));
      assert.equal(onAlt.branch, "alt", "alt shows its own conversion of B");
      assert.ok(!lean.calls.includes("/v1/facts/import"));

      // Routes follow each branch's picks: alt closes, main still needs A1 and A2.
      const assembleMain = await command(id, "alice", {
        type: "assemble",
        component: "lean-worker",
        branch: "main",
      });
      assert.equal(assembleMain.body.reason, "no_active_route");
      await done(id, "alice", {
        type: "assemble",
        component: "lean-worker",
        branch: "alt",
      });
      p = await project(id);
      const run = Object.values(p.runs).find((x) => x.status === "certified")!;
      assert.equal(run.branch, "alt");
      assert.equal(
        run.route.steps.find((s) => p.facts[s.certificate_id].goal === p.root)!
          .certificate_id,
        d2,
      );
      // Accepting writes into alt only; main keeps its own decomposition.
      await done(id, "alice", { type: "accept", run: run.id });
      p = await project(id);
      assert.equal(
        Object.values(p.branches!.alt.writebacks)[0].status,
        "committed",
      );
      assert.equal(
        (await alt.view(r)).blocks.find(
          (x: { srctype: string }) => x.srctype === "lean",
        ).metadata.certification_id,
        run.certification_id,
      );
      assert.equal(
        (await main.view(r)).blocks.find(
          (x: { srctype: string }) => x.srctype === "lean",
        ).content,
        R1,
      );
      // An owner can point main's route at alt's decomposition.
      await done(id, "alice", {
        type: "pick",
        branch: "main",
        goal: p.root,
        certificate: d2,
      });
      await done(id, "alice", {
        type: "assemble",
        component: "lean-worker",
        branch: "main",
      });
      p = await project(id);
      assert.ok(
        Object.values(p.runs).some(
          (x) => x.branch === "main" && x.status === "certified",
        ),
      );

      // A branch whose root states something else submits nothing.
      await directory("alice", {
        action: "new_branch",
        project: `${db}/main`,
        name: "other",
      });
      const other = client(() => dep, db, "other");
      await other.setLean(
        r,
        "theorem sum_doubles : double 2 + double 3 = 11 := sorry\n",
      );
      const facts = Object.keys(p.facts).length;
      await submit("alice", "other", r, "alice-other");
      await dep.drain();
      p = await project(id);
      assert.equal(p.branches!.other.request.error, "root_statement_differs");
      assert.equal(p.branches!.other.request.bound, false);
      assert.equal(Object.keys(p.facts).length, facts);

      // Branch visibility: carol reads main only; alt and its runs are left out for her.
      await directory("alice", {
        action: "grant",
        project: `${db}/main`,
        actor: "carol",
        role: "viewer",
      });
      await done(id, "alice", { type: "members", members: ["bob", "carol"] });
      await dep.drain();
      const seen = await project(id, "carol");
      assert.deepEqual(Object.keys(seen.branches!).sort(), ["main"]);
      assert.ok(Object.values(seen.runs).every((x) => x.branch === "main"));
      assert.equal(
        (
          await call(
            "carol",
            "GET",
            `/api/coordination/projects/${id}/nodes/${a1}/draft?branch=alt`,
          )
        ).status,
        404,
      );

      // A stage 4 proof request is readable but takes no commands.
      const legacy: Board = {
        ...newProjectBoard(
          "legacy-1",
          "alice",
          {
            document: { database: db, branch: "main", node: r },
            members: [],
            budget: 1,
          },
          "old",
          {
            base: {},
            context: {},
            options: {},
            minimum_trust: "audited",
            environment_id: "old",
          },
        ),
        ready: true,
      };
      delete legacy.project;
      delete legacy.branches;
      legacy.request = {
        database: db,
        branch: "main",
        node: r,
        environment_id: "old",
        bound: true,
        stale: false,
      };
      legacy.nodes = {};
      await dep.store.create(legacy);
      assert.equal(
        (await main.command("legacy-1", "alice", { type: "sync" })).body.reason,
        "proof_request_read_only",
      );
      assert.equal((await project("legacy-1")).id, "legacy-1");

      // Changing the environment retires the board: readable, no commands; a new one starts.
      await ok("alice", "PUT", envPath, {
        base_key: 1,
        options: { maxHeartbeats: 1000 },
      });
      p = await project(id);
      assert.equal(p.project!.superseded, true);
      assert.equal(
        (await command(id, "alice", { type: "sync" })).body.reason,
        "project_superseded",
      );
      const fresh = await submit("alice", "main", r, "alice-2");
      assert.equal(fresh.status, 201);
      assert.notEqual(fresh.body.id, id);
      await dep.drain();
      const q = await project(fresh.body.id);
      assert.notEqual(
        q.root,
        p.root,
        "the new environment gives new goal identities",
      );
      assert.equal((await certification(main, b)).project, fresh.body.id);
      // Returning to the first environment revives its board and retires the second.
      await ok("alice", "PUT", envPath, { base_key: 1 });
      assert.equal((await project(id)).project!.superseded, undefined);
      assert.equal((await project(fresh.body.id)).project!.superseded, true);
      assert.equal((await command(id, "alice", { type: "sync" })).status, 200);
      assert.equal((await submit("alice", "main", r, "alice-3")).body.id, id);
    } finally {
      await removeDatabases(db);
      await dep.close();
      await lean.close();
      await schema.close();
    }
  },
);
