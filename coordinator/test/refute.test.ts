// Project stage 5 (docs/mathdoc.md §2.7.5): a wrong subgoal is refuted by proving its negation,
// after which the decomposition using it leaves every route and its tasks are cancelled; `next`
// points agents at what is worth doing. Only mdc commands, plus the owner's review and assembly.
// Real PostgreSQL and TerminusDB; LeanGround is the double, or the real service when the live
// harness variables are set (the Lean is valid in Init and Mathlib).
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.js";
import {
  actors,
  client,
  deploy,
  env,
  isolated,
  ready,
  removeDatabases,
} from "./harness.js";
import { leangroundDouble } from "./leanground-double.js";
import type { Board } from "../src/domain.js";

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);
const D = "def double (n : Nat) : Nat := n + n\n";
const R = "theorem double_add (n : Nat) : double n + 0 = n + n := sorry\n";
// A decomposition through a false premise (its second half says 1 = 2) and a harmless one.
const PREMISES =
  "theorem bad (n : Nat) : double n + 0 = n + n ∧ 1 = 2 := sorry\n" +
  "theorem helper : 1 = 1 := sorry\n";
const PROOF = "by\n  have _ := helper\n  exact (bad n).1\n";
const REFUTATION = "by\n  intro h\n  exact absurd (h 0).2 (by decide)\n";

async function flow(leanground: { url: string; token: string }) {
  const schema = await isolated();
  const db = `refute${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const dep = await deploy(schema.url, leanground);
  const http = client(() => dep, db);
  const dir = await mkdtemp(join(tmpdir(), "mdc-refute-"));
  const mdc = (...argv: string[]) =>
    main(
      argv,
      {
        MDC_URL: dep.base,
        MDC_TOKEN: actors.bob.token,
        MDC_PROJECT: `${db}/main`,
      },
      async () => "",
      { cwd: dir, sleep: () => dep.drain() },
    ) as Promise<any>;
  const edit = async (file: string, fn: (t: string) => string) =>
    writeFile(join(dir, file), fn(await readFile(join(dir, file), "utf8")));
  try {
    await http.ok("alice", "POST", "/api/projects", {
      action: "init",
      name: db,
    });
    await http.ok("alice", "POST", "/api/projects", {
      action: "grant",
      project: `${db}/main`,
      actor: "bob",
      role: "viewer",
    });
    const d = await http.node("double", D);
    const root = await http.node("double_add", R, [d]);
    await http.ok("alice", "PUT", `/api/coordination/environments/${db}`, {
      base_key: 1,
    });
    const id = (
      await http.ok(
        "alice",
        "POST",
        "/api/coordination/projects",
        {
          document: { database: db, branch: "main", node: root },
          members: ["bob"],
          budget: 1000,
        },
        { "idempotency-key": "create" },
      )
    ).id as string;
    await dep.drain();

    // The agent splits the root through the false premise; the owner accepts the split.
    const pulled = await mdc("pull", "double_add");
    await edit(pulled.file, (t) =>
      t
        .replace("\n-- Goal.", `\n${PREMISES}\n-- Goal.`)
        .replace(":= by\n  sorry\n", `:= ${PROOF}`),
    );
    const split = await mdc(
      "split",
      pulled.file,
      "-m",
      "through bad",
      "--wait",
    );
    assert.equal(split.status, "certified", JSON.stringify(split));
    await mdc("done", pulled.file);
    await http.done(id, "alice", {
      type: "decomposition",
      certificate: split.certificate,
      state: "active",
    });
    const goalOf = (name: string) =>
      split.premise_goals.find((x: { name: string }) => x.name === name)
        .goal as string;
    const bad = goalOf("bad");
    const helper = goalOf("helper");

    // next: the split's premises are the frontier; the root has an active decomposition.
    const before = await mdc("next");
    assert.deepEqual(
      before.candidates.map((c: { goal: string }) => c.goal).sort(),
      [bad, helper].sort(),
    );

    // Refuting bad: its negation becomes a goal; proving it marks bad false.
    const negation = await mdc("refute", bad.slice(0, 12));
    assert.equal(negation.refutes, bad);
    assert.equal(negation.root, "not_bad");
    assert.match(
      await readFile(join(dir, negation.file), "utf8"),
      /theorem not_bad : ¬ \(∀ \(n : Nat\), double n \+ 0 = n \+ n ∧ 1 = 2\) := by/,
    );
    await edit(negation.file, (t) =>
      t.replace(":= by\n  sorry\n", `:= ${REFUTATION}`),
    );
    const proved = await mdc("prove", negation.file, "--wait");
    assert.equal(proved.status, "certified", JSON.stringify(proved));
    await mdc("done", negation.file);

    let p: Board = await http.project(id);
    assert.equal(p.goals[bad].refuted?.certificate, proved.certificate);
    assert.equal(p.facts[split.certificate].state, "refuted");
    assert.equal(p.facts[split.certificate].refuted_by, bad);
    const tasks = (goal: string) =>
      Object.values(p.tasks).filter((t) => t.goal === goal);
    for (const g of [bad, helper])
      assert.ok(
        tasks(g).every((t) => t.state === "cancelled"),
        g,
      );
    assert.ok(tasks(p.root).every((t) => t.state !== "cancelled"));
    // The decomposition is in no route: the root cannot be assembled.
    const b = await http.project(id);
    const assemble = await http.call(
      "alice",
      "POST",
      `/api/coordination/projects/${id}/commands`,
      { type: "assemble", component: "lean-worker", branch: "main" },
      { "if-match": `"${b.revision}"`, "idempotency-key": randomUUID() },
    );
    assert.equal(assemble.body.reason, "no_active_route");
    // Its submitter finds why on the root; next now points at the root itself.
    assert.match(
      p.goals[p.root].notes!.at(-1)!.text,
      /is refuted: its premise bad is false/,
    );
    const after = await mdc("next");
    assert.deepEqual(
      after.candidates.map((c: { goal: string }) => c.goal),
      [p.root],
    );
    // A second refute answers with the same negation.
    const again = await mdc("refute", bad, "--no-claim", "-o", "again.lean");
    assert.equal(again.goal, negation.goal);
    p = await http.project(id);
    assert.equal(Object.keys(p.negations ?? {}).length, 1);
  } finally {
    await removeDatabases(db);
    await dep.close();
    await schema.close();
  }
}

test(
  "a wrong subgoal is refuted and its decomposition leaves the routes (LeanGround double)",
  { skip: !documents, timeout: 120000 },
  async () => {
    const lean = await leangroundDouble();
    try {
      await flow({ url: lean.url, token: "double" });
    } finally {
      await lean.close();
    }
  },
);

test(
  "refutation against the real LeanGround",
  { skip: !ready, timeout: 900000 },
  async () => {
    await flow({ url: env.lean!, token: env.leanToken! });
  },
);
