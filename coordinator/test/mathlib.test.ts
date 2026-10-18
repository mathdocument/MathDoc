// Migration stage 5 precondition (docs/mathdoc.md §3.2): the node conversion on a Mathlib base. Exercises the
// shapes the Init scenario does not: a structure with parameters and a Mathlib class
// argument, `extends`, a noncomputable definition over ℝ, `lemma`, Mathlib instances
// used implicitly, and a context command fixed by the proof environment. Runs when the
// harness variables are set and MDC_TEST_MATHLIB_BASE_KEY names a Mathlib base.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { client, deploy, isolated, ready, removeDatabases } from "./harness.js";
import type { Board } from "../src/domain.js";

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
      const environment = (database: string) =>
        ok("alice", "PUT", `/api/coordination/environments/${database}`, {
          base_key: Number(baseKey),
          context: { opens: ["Finset"] },
        });
      await environment(db);
      const gaussDb = `${db}g`;
      await ok("alice", "POST", "/api/projects", {
        action: "init",
        name: gaussDb,
      });
      const gauss = await client(() => dep, gaussDb).node("gauss_sum", GAUSS);
      await environment(gaussDb);
      const create = (database: string, n: string, key: string) =>
        ok(
          "alice",
          "POST",
          "/api/coordination/projects",
          { document: { database, branch: "main", node: n }, budget: 100 },
          { "idempotency-key": `${database}-${key}` },
        );
      const nodesOf = (b: Board) => b.branches!.main.nodes;

      // Conversion: three definitions registered, the root submitted as a sketch.
      const id = (await create(db, root, "root")).id as string;
      await timed("bind", () => dep.drain());
      let p = await project(id);
      assert.ok(
        p.ready && p.branches!.main.request.bound,
        JSON.stringify({
          error: p.branches!.main.request.error,
          nodes: nodesOf(p),
        }),
      );
      for (const d of [seg, pointed, length])
        assert.equal(
          nodesOf(p)[d].state,
          "registered",
          JSON.stringify(nodesOf(p)[d]),
        );
      assert.equal(nodesOf(p)[nonneg].state, "open");
      assert.equal(
        nodesOf(p)[root].state,
        "submitted",
        JSON.stringify(nodesOf(p)[root]),
      );

      // An agent proves the open lemma from its draft (a leaf with the definition IDs).
      const draft = await ok(
        "alice",
        "GET",
        `/api/coordination/projects/${id}/nodes/${nonneg}/draft?branch=main`,
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
        done(id, "alice", {
          type: "assemble",
          component: "lean-worker",
          branch: "main",
        }),
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

      // `lemma` with a context command equal to the environment's opens: a leaf. A project
      // proves one theorem, so the second root lives in its own database.
      const second = (await create(gaussDb, gauss, "gauss")).id as string;
      await timed("gauss", () => dep.drain());
      const q = await project(second);
      assert.ok(
        q.ready && q.branches!.main.request.bound,
        JSON.stringify({
          error: q.branches!.main.request.error,
          nodes: nodesOf(q),
        }),
      );
      assert.equal(
        nodesOf(q)[gauss].state,
        "submitted",
        JSON.stringify(nodesOf(q)[gauss]),
      );
      assert.equal(q.goals[q.root].status, "certified");
      console.log("mathlib timings (ms)", JSON.stringify(timings));
    } finally {
      await removeDatabases(db);
      await dep.close();
      await schema.close();
    }
  },
);
