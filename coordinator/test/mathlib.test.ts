// Plan §11 / stage 5 precondition: the node conversion on a Mathlib base. Exercises the
// shapes the Init scenario does not: a structure with parameters and a Mathlib class
// argument, `extends`, a noncomputable definition over ℝ, `lemma`, Mathlib instances
// used implicitly, and a context command fixed by the proof environment. Runs when the
// harness variables are set and MDC_TEST_MATHLIB_BASE_KEY names a Mathlib base.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { client, deploy, isolated, ready } from "./harness.js";

const baseKey = process.env.MDC_TEST_MATHLIB_BASE_KEY;

const SEG =
  "structure ClosedSeg (α : Type) [LinearOrder α] where\n  lo : α\n  hi : α\n  le : lo ≤ hi\n";
const POINTED =
  "structure PointedSeg (α : Type) [LinearOrder α] extends ClosedSeg α where\n  p : α\n  mem : lo ≤ p ∧ p ≤ hi\n";
const LENGTH =
  "noncomputable def segLength (I : ClosedSeg ℝ) : ℝ := I.hi - I.lo\n";
const NONNEG =
  "theorem segLength_nonneg (I : ClosedSeg ℝ) : 0 ≤ segLength I := sorry\n";
const ROOT =
  "theorem pointed_length_nonneg (P : PointedSeg ℝ) : 0 ≤ segLength P.toClosedSeg := by\n  exact segLength_nonneg P.toClosedSeg\n";
const GAUSS =
  "open Finset\n\nlemma gauss_sum (n : ℕ) : (∑ i ∈ range n, i) * 2 = n * (n - 1) := by\n  rw [Finset.sum_range_id_mul_two]\n";

test(
  "Mathlib base: parameterised structures, extends, ℝ, lemma and a fixed open",
  { skip: !(ready && baseKey), timeout: 1800000 },
  async () => {
    const schema = await isolated();
    const db = `mlib${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    const dep = await deploy(schema.url);
    const { ok, node, view, project, command, done } = client(() => dep, db);
    const timings: Record<string, number> = {};
    const timed = async <T>(label: string, fn: () => Promise<T>) => {
      const t = Date.now();
      try {
        return await fn();
      } finally {
        timings[label] = Date.now() - t;
      }
    };
    try {
      await ok("alice", "POST", "/api/projects", { action: "init", name: db });
      const seg = await node("ClosedSeg", SEG);
      const pointed = await node("PointedSeg", POINTED, [seg]);
      const length = await node("segLength", LENGTH, [seg]);
      const nonneg = await node("segLength_nonneg", NONNEG, [seg, length]);
      const root = await node("pointed_length_nonneg", ROOT, [
        nonneg,
        pointed,
        length,
      ]);
      const gauss = await node("gauss_sum", GAUSS);
      await ok("alice", "PUT", `/api/coordination/environments/${db}/main`, {
        base_key: Number(baseKey),
        context: { opens: ["Finset"] },
      });
      const create = (n: string, key: string) =>
        ok(
          "alice",
          "POST",
          "/api/coordination/projects",
          { document: { database: db, branch: "main", node: n }, budget: 100 },
          { "idempotency-key": `${db}-${key}` },
        );

      // Conversion: three definitions registered, the root submitted as a sketch.
      const id = (await create(root, "root")).id as string;
      await timed("bind", () => dep.drain());
      let p = await project(id);
      assert.ok(
        p.ready,
        JSON.stringify({ error: p.sync_error, nodes: p.nodes }),
      );
      for (const d of [seg, pointed, length])
        assert.equal(
          p.nodes![d].state,
          "registered",
          JSON.stringify(p.nodes![d]),
        );
      assert.equal(p.nodes![nonneg].state, "open");
      assert.equal(
        p.nodes![root].state,
        "submitted",
        JSON.stringify(p.nodes![root]),
      );

      // An agent proves the open lemma from its draft (a leaf with the definition IDs).
      const draft = await ok(
        "alice",
        "GET",
        `/api/coordination/projects/${id}/nodes/${nonneg}/draft`,
      );
      assert.equal(draft.component, "lean-worker");
      // Not vacuous: a wrong proof is rejected by the Mathlib worker.
      const wrong = await command(id, "alice", {
        type: "submit",
        goal: draft.goal,
        source: draft.source.replace(/:= sorry\n$/, ":= le_refl 0\n"),
        expected_root: draft.expected_root,
        component: "lean-worker",
        mode: "leaf",
        definitions: draft.definitions,
      });
      await dep.drain();
      const jobs = await ok(
        "alice",
        "GET",
        `/api/coordination/projects/${id}/jobs`,
      );
      assert.equal(
        jobs.find((j: { id: string }) => j.id === wrong.body.job).status,
        "failed",
      );
      assert.equal(
        (await project(id)).goals[draft.goal].status,
        "insufficient",
      );
      await timed("leaf", () =>
        done(id, "alice", {
          type: "submit",
          goal: draft.goal,
          source: draft.source.replace(
            /:= sorry\n$/,
            ":= sub_nonneg.mpr I.le\n",
          ),
          expected_root: draft.expected_root,
          component: "lean-worker",
          mode: "leaf",
          definitions: draft.definitions,
        }),
      );
      p = await project(id);
      assert.equal(p.goals[p.root].status, "derivable");
      await timed("assemble", () =>
        done(id, "alice", { type: "assemble", component: "lean-worker" }),
      );
      p = await project(id);
      const run = Object.values(p.runs).find((r) => r.status === "certified");
      assert.ok(run, JSON.stringify(p.runs));
      await timed("writeback", () =>
        done(id, "alice", { type: "accept", run: run.id }),
      );
      const lemma = (await view(nonneg)).blocks.find(
        (b: { srctype: string }) => b.srctype === "lean",
      );
      assert.match(lemma.content, /sub_nonneg\.mpr I\.le/);
      assert.equal(
        (await view(root)).formalization.lean_certification.status,
        "certified",
      );

      // `lemma` with a context command equal to the environment's opens: a leaf.
      const second = (await create(gauss, "gauss")).id as string;
      await timed("gauss", () => dep.drain());
      const q = await project(second);
      assert.ok(
        q.ready,
        JSON.stringify({ error: q.sync_error, nodes: q.nodes }),
      );
      assert.equal(
        q.nodes![gauss].state,
        "submitted",
        JSON.stringify(q.nodes![gauss]),
      );
      assert.equal(q.goals[q.root].status, "certified");
      console.log("mathlib timings (ms)", JSON.stringify(timings));
    } finally {
      await dep.close();
      await schema.close();
    }
  },
);
