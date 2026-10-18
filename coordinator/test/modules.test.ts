// Module projects (docs/mathdoc.md §4.13, project stage 8): every Lean node is a whole file;
// a goal is pulled as a directory, its files are submitted as one contribution, and a certified
// route is written back as file nodes. The first tests need nothing; the flow needs PostgreSQL
// and TerminusDB, with LeanGround's contract double or, with the live harness variables set and
// a LeanGround that has the lean-modules component, the real service.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { chmod, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.js";
import {
  headerImports,
  projectFiles,
  rootFile,
  statementOf,
} from "../src/modules.js";
import { contribution } from "../src/mdc/module-dir.js";
import type { ModuleRecord } from "../src/mdc/workspace.js";
import type { Node } from "../src/docs/model.js";
import type { Board } from "../src/domain.js";
import {
  actors,
  client,
  deploy,
  env,
  isolated,
  removeDatabases,
} from "./harness.js";
import { leangroundDouble } from "./leanground-double.js";

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);

test("a file's header imports end at its first other command", () => {
  const text =
    "/- a\nimport Fake -/\nmodule\n-- note\npublic import A.B C\nimport all D\n\ntheorem x : True := trivial\nimport E\n";
  assert.deepEqual(headerImports(text), ["A.B", "C", "D"]);
  assert.deepEqual(headerImports("theorem x : True := trivial\n"), []);
});

test("the root's files: dependencies and imports, never written-back contribution files", () => {
  const uuid = (i: number) =>
    `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`;
  const node = (
    i: number,
    content: string,
    depens: number[] = [],
    metadata = {},
  ): Node => ({
    fnode: uuid(i),
    title: `n${i}`,
    module: `Lib.N${i}`,
    depens: depens.map(uuid),
    blocks: [{ srctype: "lean", content, metadata }],
  });
  const nodes = new Map(
    [
      node(1, "import Lib.N2\ntheorem t : True := by\n  sorry\n", [5]),
      node(2, "import Lib.N3\ndef a : Nat := 0\n"),
      node(3, "def b : Nat := 1\n"),
      node(4, "def unused : Nat := 2\n"),
      // A written-back proof file the root depends on is not a project module.
      node(5, "theorem t : True := trivial\n", [], { contribution: "c1" }),
      node(6, "def c : Nat := 3\n", [], { import_module: "Up.Stream" }),
    ].map((n) => [n.fnode, n]),
  );
  const out = projectFiles(nodes, uuid(1));
  assert.equal(out.root?.path, "Lib/N1.lean");
  assert.deepEqual(
    out.shared.map((f) => f.path),
    ["Lib/N3.lean", "Lib/N2.lean"],
  );
  // An imported file keeps its upstream module name; a «quoted» name is no path.
  nodes.get(uuid(1))!.depens.push(uuid(6));
  nodes.set(uuid(7), { ...node(7, "def q : Nat := 4\n"), module: "Lib.«a b»" });
  nodes.get(uuid(1))!.depens.push(uuid(7));
  const again = projectFiles(nodes, uuid(1));
  assert.ok(again.shared.some((f) => f.path === "Up/Stream.lean"));
  assert.equal(again.problems.get(uuid(7))?.reason, "invalid_module_name");
});

test("a statement for display, and the file declaring a root", () => {
  const files = {
    "W/A.lean": "namespace W\ntheorem helper (n : Nat) : n = n := rfl\nend W\n",
    "W/Main.lean":
      "import W.A\n/-- doc -/\ntheorem W.main {α : Type} (x : α) (h : x = x) :\n    (fun y : α => y) x = x := by\n  rfl\n",
  };
  assert.deepEqual(statementOf(files, "W.main"), {
    name: "W.main",
    binders: "{α : Type} (x : α) (h : x = x)",
    type: "(fun y : α => y) x = x",
  });
  assert.equal(statementOf(files, "missing"), undefined);
  assert.equal(rootFile(files, "W.main"), "W/Main.lean");
  // Without a declaration found by name: the file nothing else imports.
  assert.equal(rootFile(files, "W.other"), "W/Main.lean");
});

test("a goal directory's contribution leaves the shared modules out and keeps them unchanged", async () => {
  const dir = await mkdtemp(join(tmpdir(), "mdc-modules-"));
  const shared = "def a : Nat := 0\n";
  const { sha256 } = await import("../src/mdc/goal-file.js");
  await import("node:fs/promises").then(async (fs) => {
    await fs.mkdir(join(dir, "Lib"), { recursive: true });
    await fs.mkdir(join(dir, "Work"), { recursive: true });
    await fs.mkdir(join(dir, ".lake", "build"), { recursive: true });
  });
  await writeFile(join(dir, "Lib", "S.lean"), shared);
  await writeFile(
    join(dir, "Work", "Main.lean"),
    "import Lib.S\ntheorem t : a = 0 := rfl\r\n",
  );
  await writeFile(join(dir, "Work", "Defs.lean"), "def b : Nat := 1\n");
  await writeFile(join(dir, ".lake", "build", "X.lean"), "ignored\n");
  await writeFile(join(dir, "GOAL.md"), "notes\n");
  const record = {
    file: "d",
    shared: { ids: ["s1"], files: { "Lib/S.lean": sha256(shared) } },
  } as unknown as ModuleRecord;
  const out = await contribution(dir, record, [join(dir, "Work", "Defs.lean")]);
  assert.deepEqual(out.files, {
    "Work/Main.lean": "import Lib.S\ntheorem t : a = 0 := rfl\n",
  });
  assert.deepEqual(out.newShared, { "Work/Defs.lean": "def b : Nat := 1\n" });
  await writeFile(join(dir, "Lib", "S.lean"), "def a : Nat := 1\n");
  await assert.rejects(contribution(dir, record), /shared_module_modified/);
  await writeFile(join(dir, "Lib", "S.lean"), shared);
  await writeFile(join(dir, "Work", "bad-name.lean"), "x\n");
  await assert.rejects(contribution(dir, record), /invalid_path/);
});

async function flow(
  leanground: { url: string; token: string },
  options: { real: boolean },
) {
  const schema = await isolated();
  const db = `modules${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const dep = await deploy(schema.url, leanground);
  const http = client(() => dep, db);
  const dir = await mkdtemp(join(tmpdir(), "mdc-modules-"));
  const io = { cwd: dir, now: () => Date.now(), sleep: () => dep.drain() };
  const as =
    (who: "alice" | "bob") =>
    (...argv: string[]) =>
      main(
        argv,
        {
          MDC_URL: dep.base,
          MDC_TOKEN: actors[who].token,
          MDC_PROJECT: `${db}/main`,
        },
        async () => "",
        io,
      ) as Promise<any>;
  const mdc = as("bob");
  const read = (path: string) => readFile(join(dir, path), "utf8");
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
    // A shared module (a definition) and the root statement, a whole file importing it.
    const defs = await http.node(
      "Defs",
      "def double (n : Nat) : Nat := n + n\n",
    );
    const defsModule = (await http.view(defs)).module as string;
    const statement = `import ${defsModule}\n\ntheorem double_add (n : Nat) : double n + 0 = n + n := by\n  sorry\n`;
    const root = await http.node("double_add", statement, [defs]);
    // A modules environment takes no context: every file states its own.
    const refused = await http.call(
      "alice",
      "PUT",
      `/api/coordination/environments/${db}`,
      {
        base_key: 1,
        mode: "modules",
        context: { opens: ["Nat"] },
      },
    );
    assert.equal(refused.status, 400);
    const environment = await http.ok(
      "alice",
      "PUT",
      `/api/coordination/environments/${db}`,
      {
        base_key: 1,
        mode: "modules",
      },
    );
    assert.equal(environment.mode, "modules");
    const created = await http.ok(
      "alice",
      "POST",
      "/api/coordination/projects",
      {
        document: { database: db, branch: "main", node: root },
        members: ["bob"],
        budget: 1000,
      },
      { "idempotency-key": "create" },
    );
    const id = created.id as string;
    await dep.drain();
    let p: Board = await http.project(id);
    assert.ok(
      p.ready && p.branches!.main.request.bound,
      JSON.stringify(p.branches),
    );
    assert.deepEqual([p.mode, p.review], ["modules", "none"]);
    const nodes = p.branches!.main.nodes;
    assert.deepEqual(
      [nodes[root].role, nodes[root].state, nodes[root].conclusion],
      ["theorem", "open", "double_add"],
    );
    assert.deepEqual(
      [nodes[defs].role, nodes[defs].state],
      ["definition", "registered"],
    );
    const sharedId = nodes[defs].definition_ids![0];
    assert.deepEqual(p.goals[p.root].module, {
      const: "double_add",
      shared: [sharedId],
      node: root,
      path: `${(await http.view(root)).module.split(".").join("/")}.lean`,
    });
    assert.deepEqual(p.shared_modules![sharedId].nodes, [defs]);
    assert.equal(p.goals[p.root].declaration?.type, "double n + 0 = n + n");
    // Declaration commands do not apply to a module project.
    const declaration = await http.command(id, "bob", {
      type: "submit",
      goal: p.root,
      source: "theorem double_add (n : Nat) : double n + 0 = n + n := rfl\n",
      expected_root: "double_add",
      component: "lean-worker",
      mode: "leaf",
    });
    assert.equal(declaration.body.reason, "not_in_module_mode");

    // pull: a directory with the statement as Work/Main.lean and the shared module, read only.
    const pulled = await mdc("pull", "double_add");
    const goalDir = pulled.directory as string;
    assert.equal(goalDir, `${p.root.slice(0, 8)}-double_add`);
    assert.deepEqual(pulled.files, ["Work/Main.lean"]);
    assert.equal(await read(`${goalDir}/Work/Main.lean`), statement);
    const defsPath = `${defsModule.split(".").join("/")}.lean`;
    const sharedPath = `${goalDir}/${defsPath}`;
    assert.equal(
      await read(sharedPath),
      "def double (n : Nat) : Nat := n + n\n",
    );
    assert.equal((await stat(join(dir, sharedPath))).mode & 0o222, 0);
    assert.match(
      await read(`${goalDir}/GOAL.md`),
      /Prove the theorem `double_add`/,
    );
    await assert.rejects(
      mdc("pull", "double_add", "--no-claim"),
      /file_exists/,
    );

    // A decomposition: one placeholder, and the statement proved from it.
    const split = statement.replace(
      "theorem double_add (n : Nat) : double n + 0 = n + n := by\n  sorry\n",
      "theorem double_eq (n : Nat) : double n = n + n := by\n  sorry\n\n" +
        "theorem double_add (n : Nat) : double n + 0 = n + n := by\n  rw [Nat.add_zero, double_eq]\n",
    );
    await writeFile(join(dir, goalDir, "Work", "Main.lean"), split);
    // A scratch file outside any goal directory goes to the resident checker on the base.
    await writeFile(join(dir, "scratch-gf.lean"), "#check Nat.add_zero\n");
    const scratch = await mdc("check", "scratch-gf.lean");
    assert.equal(scratch.ok, true, JSON.stringify(scratch));
    assert.ok(Array.isArray(scratch.diagnostics));
    const checked = await mdc("check", goalDir);
    assert.equal(checked.ok, true, JSON.stringify(checked));
    assert.deepEqual(checked.premises, ["double_eq"]);
    // A shared module stays as pulled.
    await chmod(join(dir, sharedPath), 0o644);
    await writeFile(
      join(dir, sharedPath),
      "def double (n : Nat) : Nat := 2 * n\n",
    );
    await assert.rejects(
      mdc("split", goalDir, "-m", "x"),
      /shared_module_modified/,
    );
    await writeFile(
      join(dir, sharedPath),
      "def double (n : Nat) : Nat := n + n\n",
    );
    // prove refuses a decomposition before anything is recorded.
    const wrongKind = await mdc("prove", goalDir, "--wait");
    assert.deepEqual(
      [wrongKind.status, wrongKind.reason],
      ["rejected", "has_premises"],
    );
    const sketched = await mdc(
      "split",
      goalDir,
      "-m",
      "unfold `double`",
      "--wait",
    );
    assert.equal(sketched.status, "certified", JSON.stringify(sketched));
    // Decompositions take effect at once (§6 decision 4).
    assert.equal(sketched.certificate_state, "active");
    const [premise] = sketched.premise_goals;
    assert.deepEqual(
      [premise.name, premise.new, premise.statement],
      ["double_eq", true, "resolved"],
    );
    p = await http.project(id);
    assert.equal(p.facts[sketched.certificate].description, "unfold `double`");
    const g = p.goals[premise.goal];
    assert.ok(g.identity, "the premise can take a proof");
    assert.deepEqual(g.module, {
      const: "double_eq",
      shared: [sharedId],
      certificate: sketched.certificate,
    });
    assert.deepEqual(g.declaration, {
      name: "double_eq",
      binders: "(n : Nat)",
      type: "double n = n + n",
    });

    // The premise's directory starts from the decomposition's files.
    const second = await mdc("pull", premise.goal.slice(0, 10));
    const premiseDir = second.directory as string;
    assert.equal(second.statement_from, "decomposition");
    assert.equal(await read(`${premiseDir}/Work/Main.lean`), split);
    await writeFile(
      join(dir, premiseDir, "Work", "Main.lean"),
      split.replace(
        "theorem double_eq (n : Nat) : double n = n + n := by\n  sorry\n",
        "theorem double_eq (n : Nat) : double n = n + n := by\n  rfl\n",
      ),
    );
    // A root of another statement does not connect. (`double_add` would: its type reduces to
    // the premise's, since `n + 0` is `n` by definition, and the kernel compares up to that.)
    await writeFile(
      join(dir, premiseDir, "Work", "Other.lean"),
      "theorem other (n : Nat) : n = n := rfl\n",
    );
    const wrong = await mdc("prove", premiseDir, "--root", "other", "--wait");
    await import("node:fs/promises").then((fs) =>
      fs.rm(join(dir, premiseDir, "Work", "Other.lean")),
    );
    assert.equal(wrong.status, "rejected");
    assert.equal(wrong.reason, "verification_failed");
    const proved = await mdc("prove", premiseDir, "--wait");
    assert.equal(proved.status, "certified", JSON.stringify(proved));
    for (const d of [premiseDir, goalDir]) await mdc("done", d);
    p = await http.project(id);
    assert.equal(p.goals[premise.goal].status, "certified");
    assert.equal(p.goals[p.root].status, "derivable");

    // Assembly with the module component, whatever the client named.
    await http.done(id, "alice", {
      type: "assemble",
      component: "lean-worker",
      branch: "main",
    });
    p = await http.project(id);
    const run = Object.values(p.runs).at(-1)!;
    assert.equal(run.component, "lean-modules");
    assert.equal(run.status, "certified", `${run.failure}: ${run.details}`);

    // Writeback: every file of the route as a node; the root keeps its text.
    await http.done(id, "alice", { type: "accept", run: run.id });
    p = await http.project(id);
    const written = Object.values(p.branches!.main.writebacks)[0];
    assert.equal(
      written.status,
      "committed",
      `${written.reason}: ${written.details}`,
    );
    const rootNode = await http.view(root);
    const rootLean = rootNode.blocks.find(
      (x: { srctype: string }) => x.srctype === "lean",
    );
    assert.equal(rootLean.content, statement);
    assert.equal(rootLean.metadata.certification_id, run.certification_id);
    const files = Object.values(p.branches!.main.nodes).filter((n) => n.file);
    assert.deepEqual(
      files
        .map((n) => [n.certificate, n.file!.path, n.file!.root, n.state])
        .sort(),
      [
        [proved.certificate, "Work/Main.lean", true, "certified"],
        [sketched.certificate, "Work/Main.lean", true, "certified"],
      ].sort(),
    );
    const splitNode = files.find(
      (n) => n.certificate === sketched.certificate,
    )!;
    const proofNode = files.find((n) => n.certificate === proved.certificate)!;
    assert.ok(
      rootNode.depens.includes(splitNode.node),
      "the root depends on its proof",
    );
    const splitView = await http.view(splitNode.node);
    assert.deepEqual(
      new Set(splitView.depens),
      new Set([defs, proofNode.node]),
    );
    assert.equal(
      splitView.blocks[0].metadata.contribution,
      sketched.certificate,
    );
    assert.equal(p.branches!.main.request.stale, false);
    // The goal keeps its name; the file nodes are named by their paths.
    assert.match(splitNode.title, / — Work\/Main\.lean$/);

    // A decomposition with a new shared module: its definition states the new goal.
    const third = await as("alice")(
      "pull",
      "double_add",
      "--no-claim",
      "-o",
      "triple",
    );
    assert.equal(third.directory, "triple");
    await import("node:fs/promises").then((fs) =>
      fs.mkdir(join(dir, "triple", "Extra"), { recursive: true }),
    );
    await writeFile(
      join(dir, "triple", "Extra", "Triple.lean"),
      `import ${defsModule}\n\ndef triple (n : Nat) : Nat := double n + n\n`,
    );
    await writeFile(
      join(dir, "triple", "Work", "Main.lean"),
      `import ${defsModule}\nimport Extra.Triple\n\n` +
        "theorem triple_eq (n : Nat) : triple n = n + n + n := by\n  sorry\n\n" +
        "theorem double_add (n : Nat) : double n + 0 = n + n := by\n" +
        "  have h := triple_eq n\n  unfold triple at h\n  omega\n",
    );
    const viaTriple = await as("alice")(
      "split",
      "triple",
      "--shared",
      join(dir, "triple", "Extra", "Triple.lean"),
      "-m",
      "through triple",
      "--wait",
    );
    assert.equal(viaTriple.status, "certified", JSON.stringify(viaTriple));
    assert.deepEqual(viaTriple.new_shared, ["Extra/Triple.lean"]);
    p = await http.project(id);
    const tripleGoal = viaTriple.premise_goals[0].goal as string;
    const registered = Object.values(p.shared_modules!).find(
      (m) => m.certificate === viaTriple.certificate,
    )!;
    assert.deepEqual(registered.modules, ["Extra.Triple"]);
    assert.deepEqual(
      new Set(p.goals[tripleGoal].module!.shared),
      new Set([sharedId, registered.id]),
    );
    const fourth = await mdc("pull", tripleGoal.slice(0, 10), "--no-claim");
    assert.deepEqual(
      new Set(fourth.shared),
      new Set([defsPath, "Extra/Triple.lean"]),
    );
    // Refutation (§4.13): a decomposition with a false premise. `mdc refute` states the
    // premise's negation at the end of the decomposition's file; its complete proof marks the
    // premise false and refutes the decomposition.
    await as("alice")("pull", "double_add", "--no-claim", "-o", "wrong");
    const wrongSplit =
      `import ${defsModule}\n\n` +
      "theorem double_succ (n : Nat) : double n = n + n + 1 := by\n  sorry\n\n" +
      "theorem double_add (n : Nat) : double n + 0 = n + n := by\n" +
      "  have h := double_succ n\n  unfold double at h\n  omega\n";
    await writeFile(join(dir, "wrong", "Work", "Main.lean"), wrongSplit);
    const viaWrong = await as("alice")(
      "split",
      "wrong",
      "-m",
      "a false premise",
      "--wait",
    );
    assert.equal(viaWrong.status, "certified", JSON.stringify(viaWrong));
    const falseGoal = viaWrong.premise_goals[0].goal as string;
    const refuting = await mdc("refute", falseGoal.slice(0, 10));
    assert.deepEqual(
      [refuting.refutes, refuting.statement_from, refuting.root],
      [falseGoal, "negation", "MDCRefute.not_double_succ"],
    );
    assert.deepEqual(refuting.files, ["Work/Main.lean"]);
    const negated = await read(`${refuting.directory}/Work/Main.lean`);
    assert.equal(
      negated,
      `${wrongSplit}\ntheorem _root_.MDCRefute.not_double_succ : ¬ type_of% @_root_.double_succ := sorry\n`,
    );
    assert.match(
      await read(`${refuting.directory}/GOAL.md`),
      /stated at the end of `Work\/Main\.lean`: the negation of goal/,
    );
    p = await http.project(id);
    assert.equal(p.negations![refuting.goal], falseGoal);
    assert.deepEqual(p.goals[refuting.goal].module, {
      const: "MDCRefute.not_double_succ",
      shared: p.goals[falseGoal].module!.shared,
      negates: { goal: falseGoal, path: "Work/Main.lean" },
    });
    assert.deepEqual(p.goals[refuting.goal].declaration, {
      name: "not_double_succ",
      binders: "",
      type: "¬ (∀ (n : Nat), double n = n + n + 1)",
    });
    // Asking again gives the same negation.
    const again = await http.command(id, "bob", {
      type: "negate",
      goal: falseGoal,
    });
    assert.equal(again.body.result.negation, refuting.goal);
    await writeFile(
      join(dir, refuting.directory, "Work", "Main.lean"),
      negated.replace(
        / := sorry\n$/,
        " := fun h => by\n  have h0 := h 0\n  unfold double at h0\n  omega\n",
      ),
    );
    const disproved = await mdc("prove", refuting.directory, "--wait");
    assert.equal(disproved.status, "certified", JSON.stringify(disproved));
    await mdc("done", refuting.directory);
    p = await http.project(id);
    assert.equal(
      p.goals[falseGoal].refuted?.certificate,
      disproved.certificate,
    );
    assert.deepEqual(
      [
        p.facts[viaWrong.certificate].state,
        p.facts[viaWrong.certificate].refuted_by,
      ],
      ["refuted", falseGoal],
    );

    if (options.real) {
      // The real kernel decides: the premise's own proof connects.
      await writeFile(
        join(dir, fourth.directory, "Work", "Main.lean"),
        `import ${defsModule}\nimport Extra.Triple\n\ntheorem triple_eq (n : Nat) : triple n = n + n + n := by\n  unfold triple double\n  omega\n`,
      );
      const tripled = await mdc("prove", fourth.directory, "--wait");
      assert.equal(tripled.status, "certified", JSON.stringify(tripled));
    }
  } finally {
    await removeDatabases(db);
    await dep.close();
    await schema.close();
  }
}

test(
  "a module project from its statement to writeback (LeanGround double)",
  { skip: !documents, timeout: 120000 },
  async () => {
    const lean = await leangroundDouble();
    try {
      await flow({ url: lean.url, token: "double" }, { real: false });
    } finally {
      await lean.close();
    }
  },
);

test(
  "the same module project against the real LeanGround",
  {
    skip: !(documents && process.env.MDC_TEST_MODULES_LEANGROUND_URL),
    timeout: 1800000,
  },
  async () => {
    await flow(
      {
        url: process.env.MDC_TEST_MODULES_LEANGROUND_URL!,
        token: process.env.MDC_TEST_MODULES_LEANGROUND_TOKEN!,
      },
      { real: true },
    );
  },
);
