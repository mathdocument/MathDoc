// Plan §10 stage 4: the complete scenario on a real LeanGround (lean-worker and
// lean-worker-sketch), a real TerminusDB and PostgreSQL. Skipped unless all of
// MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL, MDC_TEST_TERMINUS_PASSWORD,
// MDC_TEST_LEANGROUND_URL and MDC_TEST_LEANGROUND_TOKEN are set.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { client, deploy, isolated, ready, type Actor } from "./harness.js";

const D = "def double (n : Nat) : Nat := n + n\n";
const B = "theorem double_eq (n : Nat) : double n = n + n := sorry\n";
const A1 = "theorem double_two : double 2 = 4 := sorry\n";
const A2 = "theorem double_three : double 3 = 6 := sorry\n";
const R =
  "theorem sum_doubles : double 2 + double 3 = 10 := by\n  rw [double_two, double_three]\n";

test(
  "stage 4 scenario: shared lemma, dropout, paused decomposition, restart, assembly and writeback",
  { skip: !ready, timeout: 900000 },
  async () => {
    const schema = await isolated();
    const db = `scen${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    let dep = await deploy(schema.url);
    const { call, ok, doc, node, view, setLean, project, command, done } =
      client(() => dep, db);
    try {
      // Workspace: alice owns it, bob edits, carol only reads, eve has no role.
      await ok("alice", "POST", "/api/projects", { action: "init", name: db });
      for (const [who, role] of [
        ["bob", "editor"],
        ["carol", "viewer"],
      ])
        await ok("alice", "POST", "/api/projects", {
          action: "grant",
          project: `${db}/main`,
          actor: who,
          role,
        });

      const d = await node("double", D);
      const b = await node("double_eq", B, [d]);
      const a1 = await node("double_two", A1, [b, d]);
      const a2 = await node("double_three", A2, [b, d]);
      const r = await node("sum_doubles", R, [a1, a2, d]);

      // Proof environment: owners only; Project/lean is not touched.
      const envPath = `/api/coordination/environments/${db}/main`;
      assert.equal(
        (await call("bob", "PUT", envPath, { base_key: 1 })).status,
        403,
      );
      assert.equal((await call("eve", "GET", envPath)).status, 404);
      const environment = await ok("alice", "PUT", envPath, {
        base_key: 1,
        minimum_trust: "audited",
      });
      assert.ok(
        environment.base.root_module,
        "the base resolves to a complete BaseRef",
      );

      // Creating a ProofRequest needs editor rights; members must be able to read the branch.
      const create = (
        who: Actor,
        key: string,
        extra: Record<string, unknown> = {},
      ) =>
        call(
          who,
          "POST",
          "/api/coordination/projects",
          {
            document: { database: db, branch: "main", node: r },
            members: ["bob"],
            budget: 1000,
            ...extra,
          },
          { "idempotency-key": `${db}-${key}` },
        );
      assert.equal((await create("carol", "c1")).status, 403);
      assert.equal((await create("eve", "e1")).status, 404);
      assert.equal(
        (await create("alice", "a0", { members: ["eve"] })).body.reason,
        "member_lacks_document_access",
      );
      const created = await create("alice", "a1");
      assert.equal(created.status, 201);
      const id = created.body.id as string;
      assert.equal(
        (await create("alice", "a1")).body.id,
        id,
        "creation is idempotent",
      );
      await dep.drain();

      // Conversion: definition registered, theorem nodes resolved, the root submitted as a sketch.
      let p = await project(id);
      assert.ok(p.ready && p.request?.bound, JSON.stringify(p.sync_error));
      assert.equal(p.nodes![d].state, "registered");
      assert.equal(p.nodes![r].state, "submitted");
      assert.deepEqual(
        [p.nodes![b].state, p.nodes![a1].state, p.nodes![a2].state],
        ["open", "open", "open"],
      );
      const binding = p.nodes![r].binding as {
        component: string;
        mode: string;
        premise_placeholders: unknown[];
      };
      assert.equal(binding.component, "lean-worker-sketch");
      assert.equal(binding.premise_placeholders.length, 2);
      assert.equal(p.goals[p.root].status, "insufficient");
      const goal = (n: string) => p.nodes![n].goal!;
      assert.equal(
        p.request!.contract!.proof_request.dependencies.length,
        3,
        "A1, A2 and the definition",
      );
      // A1 and A2 share the premise B: one goal, one task.
      const tasks = (g: string) =>
        Object.values(p.tasks).filter((t) => t.goal === g);
      assert.equal(tasks(goal(b)).length, 1);

      // Unauthorised readers see neither the project, its facts, drafts nor the document.
      assert.equal(
        (await call("eve", "GET", `/api/coordination/projects/${id}`)).status,
        404,
      );
      assert.equal(
        (await call("carol", "GET", `/api/coordination/projects/${id}`)).status,
        404,
      );
      const certId = p.nodes![r].certificate!;
      assert.equal(
        (
          await call(
            "eve",
            "GET",
            `/api/coordination/projects/${id}/facts/${certId}`,
          )
        ).status,
        404,
      );
      assert.equal(
        (
          await call(
            "eve",
            "GET",
            `/api/coordination/projects/${id}/nodes/${a1}/draft`,
          )
        ).status,
        404,
      );
      assert.equal(
        (await call("eve", "GET", `/p/${db}/main/api/graph/full`)).status,
        404,
      );
      assert.equal(
        (
          await command(id, "alice", {
            type: "members",
            members: ["bob", "eve"],
          })
        ).body.reason,
        "member_lacks_document_access",
      );

      // B is not on any route yet (its parents are open), so it cannot be scheduled.
      const inactive = await command(id, "bob", {
        type: "claim",
        task: tasks(goal(b))[0].id,
        allocation: 1,
        ttl: 300,
      });
      assert.equal(
        inactive.body.reason,
        "inactive_goal",
        JSON.stringify(inactive),
      );
      // Two participants race for one exclusive lease: exactly one wins.
      const taskB = tasks(goal(a1))[0].id;
      const rev = (await project(id)).revision;
      const race = await Promise.all(
        (["alice", "bob"] as const).map((who) =>
          call(
            who,
            "POST",
            `/api/coordination/projects/${id}/commands`,
            { type: "claim", task: taskB, allocation: 100, ttl: 300 },
            { "if-match": `"${rev}"`, "idempotency-key": `race-${who}` },
          ),
        ),
      );
      assert.equal(
        race.filter((x) => x.status === 200).length,
        1,
        JSON.stringify(race),
      );
      const winner =
        race.findIndex((x) => x.status === 200) === 0 ? "alice" : "bob";
      // Replaying the same request neither claims twice nor reserves budget twice.
      const replay = await call(
        winner,
        "POST",
        `/api/coordination/projects/${id}/commands`,
        { type: "claim", task: taskB, allocation: 100, ttl: 300 },
        { "if-match": `"${rev}"`, "idempotency-key": `race-${winner}` },
      );
      assert.deepEqual(replay.body, race.find((x) => x.status === 200)!.body);
      p = await project(id);
      assert.equal(
        Object.values(p.attempts).filter((a) => !a.outcome).length,
        1,
      );

      // Participants prove the two sub-lemmas from the agent draft (sketches with premise B).
      const prove = async (who: Actor, n: string, proof: string) => {
        const draft = await ok(
          who,
          "GET",
          `/api/coordination/projects/${id}/nodes/${n}/draft`,
        );
        assert.equal(draft.component, "lean-worker-sketch");
        const source = (draft.source as string).replace(
          /:= sorry\n$/,
          `:= ${proof}\n`,
        );
        return done(id, who, {
          type: "submit",
          goal: draft.goal,
          source,
          expected_root: draft.expected_root,
          component: "lean-worker-sketch",
          mode: "sketch",
          definitions: draft.definitions,
        });
      };
      await prove("alice", a1, "by rw [double_eq]");

      // bob claims A2 with a short lease and drops out.
      const taskA2 = tasks(goal(a2))[0].id;
      const lease = (
        await command(id, "bob", {
          type: "claim",
          task: taskA2,
          allocation: 50,
          ttl: 30,
        })
      ).body.result;
      await delay(31000);
      assert.equal(
        (
          await command(id, "bob", {
            type: "heartbeat",
            attempt: lease.id,
            epoch: lease.epoch,
            ttl: 30,
          })
        ).body.reason,
        "stale_lease",
      );
      // Expiry is recorded by the next state change (here the periodic sync).
      await done(id, "alice", { type: "sync" });
      p = await project(id);
      assert.equal(p.attempts[lease.id].outcome, "expired");
      assert.equal(
        p.attempts[lease.id].spent,
        50,
        "a dropout is charged its reservation",
      );
      assert.equal(p.tasks[taskA2].state, "ready");
      // Expiry revokes scheduling, not results: bob's late proof is still absorbed.
      await prove("bob", a2, "by rw [double_eq]");

      // Decomposition 1 of B is a cycle (premise = B itself): stored, never proves B.
      const cycle =
        "theorem double_eq_again (n : Nat) : double n = n + n := sorry\ntheorem double_eq (n : Nat) : double n = n + n := double_eq_again n\n";
      const defs = [...p.nodes![d].definition_ids!];
      await done(id, "bob", {
        type: "submit",
        goal: goal(b),
        source: cycle,
        expected_root: "double_eq",
        component: "lean-worker-sketch",
        mode: "sketch",
        definitions: defs,
      });
      p = await project(id);
      const cycleFact = Object.values(p.facts).find(
        (f) => f.goal === goal(b) && f.premises.includes(goal(b)),
      )!;
      assert.ok(cycleFact, "the cyclic decomposition is recorded");
      assert.equal(
        p.goals[goal(b)].status,
        "insufficient",
        "a cycle without leaf evidence is not proved",
      );
      await done(id, "alice", {
        type: "decomposition",
        certificate: cycleFact.id,
        state: "paused",
      });

      // Decomposition 2 of B introduces a new definition `twice`, registered with the sketch.
      const split =
        "def twice (n : Nat) : Nat := n + n\n" +
        "theorem double_twice (n : Nat) : double n = twice n := sorry\n" +
        "theorem twice_eq (n : Nat) : twice n = n + n := sorry\n" +
        "theorem double_eq (n : Nat) : double n = n + n := by\n  rw [double_twice, twice_eq]\n";
      await done(id, "alice", {
        type: "submit",
        goal: goal(b),
        source: split,
        expected_root: "double_eq",
        component: "lean-worker-sketch",
        mode: "sketch",
        definitions: defs,
        new_definitions: ["twice"],
      });
      p = await project(id);
      const splitFact = Object.values(p.facts).find(
        (f) => f.goal === goal(b) && f.premises.length === 2,
      )!;
      const cert = await ok(
        "alice",
        "GET",
        `/api/coordination/projects/${id}/facts/${splitFact.id}`,
      );
      const twiceId = (cert.definitions as string[]).find(
        (x) => !defs.includes(x),
      )!;
      assert.ok(twiceId, "the new definition is used by the certificate");

      // --- Service restart: a new API and worker on the same databases resume the work.
      const cursor = p.cursor;
      await dep.close();
      dep = await deploy(schema.url);
      await done(id, "alice", { type: "sync" });
      p = await project(id);
      assert.ok(p.cursor >= cursor);

      // The new subgoals need statements (with both definitions) before leaf proofs; the
      // certificate's premise bindings name them.
      const premiseGoal = (name: string) =>
        (cert.bindings as { name: string; role: string; goal: string }[]).find(
          (x) => x.role === "premise" && x.name === name,
        )!.goal;
      for (const [name, prop, source] of [
        [
          "double_twice",
          "∀ (n : Nat), double n = twice n",
          "theorem double_twice (n : Nat) : double n = twice n := rfl\n",
        ],
        [
          "twice_eq",
          "∀ (n : Nat), twice n = n + n",
          "theorem twice_eq (n : Nat) : twice n = n + n := rfl\n",
        ],
      ]) {
        const g = premiseGoal(name);
        await done(id, "bob", {
          type: "resolve",
          goal: g,
          proposition: prop,
          definitions: [...defs, twiceId],
        });
        assert.ok((await project(id)).goals[g].identity, `resolved ${name}`);
        await done(id, "bob", {
          type: "submit",
          goal: g,
          source,
          expected_root: name,
          component: "lean-worker",
          mode: "leaf",
          definitions: [...defs, twiceId],
        });
      }
      p = await project(id);
      assert.equal(p.goals[goal(b)].status, "derivable");
      assert.equal(p.goals[p.root].status, "derivable");

      // Assembly selects a route through active decompositions only.
      await done(id, "alice", { type: "assemble", component: "lean-worker" });
      p = await project(id);
      const run = Object.values(p.runs).find((x) => x.status === "certified");
      assert.ok(run, JSON.stringify(Object.values(p.runs)));
      assert.ok(
        !run.route.steps.some((s) => s.certificate_id === cycleFact.id),
        "paused decomposition not used",
      );
      assert.equal(p.goals[p.root].status, "certified");
      let rootView = await view(r);
      assert.equal(
        rootView.formalization.lean_certification.status,
        "certified",
      );

      // A conflicting node aborts the whole batch; nothing is written.
      const count = async () =>
        (await doc("alice", "GET", "/graph/full")).nodes.length;
      const nodesBefore = await count();
      await setLean(b, B.replace("sorry", "sorry -- edited"));
      await done(id, "alice", { type: "accept", run: run.id });
      p = await project(id);
      const [batch] = Object.values(p.writebacks!);
      assert.equal(batch.status, "aborted");
      assert.equal(batch.reason, "document_conflict");
      assert.equal(await count(), nodesBefore);
      assert.equal(
        (await view(a1)).blocks[0].content,
        A1,
        "no partial writeback",
      );

      // Restore the text: the same batch commits atomically, and replays create nothing new.
      await setLean(b, B);
      await done(id, "alice", { type: "accept", run: run.id });
      p = await project(id);
      assert.equal(
        p.writebacks![batch.batch_id].status,
        "committed",
        JSON.stringify(p.writebacks),
      );
      const after = await count();
      assert.equal(
        after,
        nodesBefore + 3,
        "double_twice, twice_eq and the definition twice",
      );
      await done(id, "alice", { type: "accept", run: run.id });
      assert.equal(await count(), after, "replay creates no nodes");
      const lean = async (n: string) =>
        (await view(n)).blocks.find(
          (x: { srctype: string }) => x.srctype === "lean",
        );
      assert.equal(
        (await lean(a1)).content,
        "theorem double_two : double 2 = 4 := by rw [double_eq]\n",
      );
      assert.match((await lean(b)).content, /rw \[double_twice, twice_eq\]/);
      assert.doesNotMatch((await lean(b)).content, /sorry/);
      assert.equal(
        (await lean(r)).metadata.certification_id,
        run.certification_id,
      );
      const full = await doc("alice", "GET", "/graph/full");
      const titles = full.nodes.map((n: { title: string }) => n.title);
      for (const t of ["double_twice", "twice_eq", "twice"])
        assert.ok(titles.includes(t), t);
      rootView = await view(r);
      assert.equal(
        rootView.formalization.lean_certification.status,
        "certified",
      );

      // Editing a definition node: dependants no longer show as certified; accept is refused.
      await setLean(d, "def double (n : Nat) : Nat := 2 * n\n");
      rootView = await view(r);
      assert.equal(rootView.formalization.lean_certification.status, "stale");
      await done(id, "alice", { type: "sync" });
      p = await project(id);
      assert.equal(p.request!.stale, true);
      assert.equal(
        (await command(id, "alice", { type: "accept", run: run.id })).body
          .reason,
        "proof_request_stale",
      );
      await setLean(d, D);

      // Unsupported shapes and same-name definitions are rejected with reasons, per node.
      const d2 = await node(
        "double (other)",
        "def double (n : Nat) : Nat := 2 * n\n",
      );
      const inst = await node("instance", "instance : Inhabited Nat := ⟨0⟩\n");
      const r3 = await node("uses both", "theorem r3 : double 1 = 2 := rfl\n", [
        d,
        d2,
        inst,
      ]);
      const second = await call(
        "alice",
        "POST",
        "/api/coordination/projects",
        { document: { database: db, branch: "main", node: r3 }, budget: 10 },
        { "idempotency-key": `${db}-second` },
      );
      await dep.drain();
      const q = await project(second.body.id);
      assert.equal(q.ready, false);
      // The root is blocked by the first rejected dependency it has, named in the details.
      assert.equal(q.sync_error, "root_ambiguous_binding");
      assert.match(q.nodes![r3].details!, /instance/);
      assert.equal(q.nodes![d].state, "registered");
      assert.equal(q.nodes![d2].reason, "definition_name_taken");
      assert.equal(q.nodes![inst].reason, "unsupported_declaration");
    } finally {
      await dep.close();
      await schema.close();
    }
  },
);
