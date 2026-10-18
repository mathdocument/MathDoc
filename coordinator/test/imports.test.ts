// External proof imports (docs/external-proofs.md §3.3). The upstream is a small git repository
// made here; LeanGround is a double of POST/GET /v1/project-checks. The second part needs real
// PostgreSQL and TerminusDB (MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL/PASSWORD).
import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { certificationLookup, importedSource } from "../src/certification.js";
import type { Node } from "../src/docs/model.js";
import { buildImport, closure, leanImports, Upstream } from "../src/imports.js";
import {
  actors,
  client,
  deploy,
  env,
  isolated,
  removeDatabases,
} from "./harness.js";
import { projectBoard } from "./fixtures.js";

const CONFIG = {
  challenge_module: "ComparatorChallenges.Toy",
  solution_module: "OAI.Toy.Main",
  theorem_names: ["OAI.Toy.main"],
  definition_names: ["OAI.Toy.Good"],
  permitted_axioms: ["propext", "Quot.sound", "Classical.choice"],
  enable_nanoda: false,
};
const FILES: Record<string, string> = {
  "lean/lean-toolchain": "leanprover/lean4:v4.34.1\n",
  "lean/ComparatorChallenges/Toy.json": JSON.stringify(CONFIG),
  "lean/ComparatorChallenges/Toy.lean":
    "import Mathlib\n\nnamespace OAI.Toy\ndef Good (n : Nat) : Prop := n = n\ntheorem main : Good 1 := by\n  sorry\nend OAI.Toy\n",
  "lean/OAI/Toy/Main.lean":
    "/- A toy proof.\n   import Fake.Module -/\nmodule\n\npublic import OAI.Toy.Basic\nimport Mathlib.Data.Nat.Basic\n\nnamespace OAI.Toy\ntheorem main : Good 1 := rfl\nimport Not.AnImport\n",
  "lean/OAI/Toy/Basic.lean":
    "-- basics\nimport Mathlib.Logic.Basic Batteries.Data\n\nnamespace OAI.Toy\ndef Good (n : Nat) : Prop := n = n\n",
  "lean/OAI/Toy/Unrelated.lean": "def unrelated := 1\n",
  "lean/docs/042.md":
    "# Toy results\n\nThe following describes the scope.\n\n- [A toy paper](../../preprints/Toy/paper.pdf)\n- [A missing paper](../../preprints/Gone/paper.pdf)\n\n## Scope\n\nWe prove $1 = 1$; see [the statement](../ComparatorChallenges/Toy.lean#L5).\n\n| Result | Comparator statement |\n| --- | --- |\n| Toy theorem | [Toy.lean](../ComparatorChallenges/Toy.lean) |\n",
  "preprints/Toy/paper.pdf": "%PDF-1.4 toy\n",
};

async function upstream() {
  const repo = await mkdtemp(join(tmpdir(), "mdc-upstream-"));
  for (const [path, text] of Object.entries(FILES)) {
    await mkdir(dirname(join(repo, path)), { recursive: true });
    await writeFile(join(repo, path), text);
  }
  const git = (...a: string[]) => execFileSync("git", ["-C", repo, ...a], { encoding: "utf8" });
  git("init", "-q");
  git("-c", "user.name=t", "-c", "user.email=t@t", "add", ".");
  git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "toy");
  return { repo, commit: git("rev-parse", "HEAD").trim() };
}

test("import lines are the header's, in any of Lean's spellings", () => {
  assert.deepEqual(
    leanImports(FILES["lean/OAI/Toy/Main.lean"]),
    ["OAI.Toy.Basic", "Mathlib.Data.Nat.Basic"],
  );
  assert.deepEqual(leanImports(FILES["lean/OAI/Toy/Basic.lean"]), [
    "Mathlib.Logic.Basic",
    "Batteries.Data",
  ]);
  assert.deepEqual(leanImports("theorem x : True := trivial\nimport A\n"), []);
});

test("an import is read from git at its commit, whatever the working tree holds", async () => {
  const { repo, commit } = await upstream();
  await writeFile(join(repo, "lean/OAI/Toy/Basic.lean"), "import Changed\n");
  const up = new Upstream(repo, commit);
  const c = await closure(up, "lean", "OAI.Toy.Main");
  assert.deepEqual(
    c.modules.map((m) => [m.module, m.imports]),
    [["OAI.Toy.Basic", []], ["OAI.Toy.Main", ["OAI.Toy.Basic"]]],
  );
  assert.deepEqual(c.external, ["Batteries", "Mathlib"]);
  const missing = await up.read(["lean/nope.lean", "lean/OAI", "../x"]);
  assert.deepEqual([...missing.values()], [null, null, null]);

  const source = { repo, lean: "lean", workspace: "toy", url: "https://example.org/toy" };
  const { record, graph } = await buildImport("oaic-toy", "toy", source, "Toy", "alice");
  assert.equal(record.commit, commit);
  assert.deepEqual(record.scope, {
    path: "lean/docs/042.md",
    family: "042",
    title: "Toy results",
    result: "Toy theorem",
  });
  assert.deepEqual(record.papers, [
    { title: "A toy paper", path: "preprints/Toy/paper.pdf", present: true },
    { title: "A missing paper", path: "preprints/Gone/paper.pdf", present: false },
  ]);
  assert.deepEqual(record.proof, {
    module: "OAI.Toy.Main",
    path: "lean/OAI/Toy/Main.lean",
    files: 2,
    lines: 15,
    external: ["Batteries", "Mathlib"],
  });
  // Five kinds of summary node plus one per file of the proof's closure.
  assert.equal(graph.nodes.length, 8);
  const byId = Object.fromEntries(graph.nodes.map((n) => [n.fnode, n])) as Record<string, Node>;
  const scope = byId[record.nodes.scope].blocks[0].content;
  assert.match(scope, new RegExp(`\\(https://example.org/toy/blob/${commit}/lean/ComparatorChallenges/Toy.lean#L5\\)`));
  assert.match(scope, /\$1 = 1\$/);
  assert.deepEqual(byId[record.nodes.overview].depens, [record.nodes.statement, record.nodes.proof]);

  // The statement and each file hold the upstream text in a Lean block, marked with its origin;
  // a file depends on the files it imports, and the proof's summary on the main file.
  const modules = record.nodes.modules!;
  assert.deepEqual(Object.keys(modules).sort(), ["OAI.Toy.Basic", "OAI.Toy.Main"]);
  const lean = (id: string) => byId[id].blocks.find((b) => b.srctype === "lean")!;
  assert.equal(lean(record.nodes.statement).content, FILES["lean/ComparatorChallenges/Toy.lean"]);
  assert.equal(lean(modules["OAI.Toy.Main"]).content, FILES["lean/OAI/Toy/Main.lean"]);
  assert.equal(lean(modules["OAI.Toy.Main"]).metadata.import_path, "lean/OAI/Toy/Main.lean");
  assert.equal(lean(modules["OAI.Toy.Main"]).metadata.import_commit, commit);
  assert.deepEqual(byId[modules["OAI.Toy.Main"]].depens, [modules["OAI.Toy.Basic"]]);
  assert.deepEqual(byId[modules["OAI.Toy.Basic"]].depens, []);
  assert.deepEqual(byId[record.nodes.proof].depens, [record.nodes.statement, modules["OAI.Toy.Main"]]);
  assert.match(byId[modules["OAI.Toy.Basic"]].blocks[0].content, /outside the repository: `Mathlib.Logic.Basic`, `Batteries.Data`/);
  for (const id of [record.nodes.overview, record.nodes.scope, record.nodes.proof])
    assert.deepEqual(byId[id].blocks.map((b) => b.srctype), ["text"]);

  // While its text is the upstream one, a file shows as imported; edited, it is ordinary Lean.
  const main = byId[modules["OAI.Toy.Main"]];
  const certification = certificationLookup([], new Map(), "main");
  assert.deepEqual(certification(main), {
    status: "imported",
    module: "OAI.Toy.Main",
    path: "lean/OAI/Toy/Main.lean",
    commit,
  });
  const edited = {
    ...main,
    blocks: main.blocks.map((b) => (b.srctype === "lean" ? { ...b, content: `${b.content}-- edited\n` } : b)),
  };
  assert.equal(importedSource(edited), null);
  assert.deepEqual(certification(edited), { status: "not_submitted" });
  // Node ids depend only on the database, so a rebuilt import keeps them.
  const again = await buildImport("oaic-toy", "toy", source, "Toy", "bob");
  assert.deepEqual(again.record.nodes, record.nodes);
  await assert.rejects(buildImport("oaic-toy", "toy", source, "../Toy", "alice"), /invalid_import_config/);
  await assert.rejects(buildImport("oaic-toy", "toy", source, "Nope", "alice"), /import_config_not_found/);
});

/** LeanGround's project-check endpoints: queued on submit, passed when read; `lose` fails one reply. */
async function leangroundDouble() {
  const checks = new Map<string, any>();
  const requests: any[] = [];
  let lose = false;
  const srv = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
    const reply = (status: number, data: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(data));
    };
    if (req.headers.authorization !== "Bearer lean-token") return reply(401, { kind: "fact_error", reason: "unauthenticated" });
    if (req.method === "POST" && req.url === "/v1/project-checks") {
      requests.push(body);
      let c = [...checks.values()].find((x) => x.key === body.idempotency_key);
      if (!c) {
        c = { check_id: `pc-${checks.size}`, key: body.idempotency_key, status: "queued", result: null };
        checks.set(c.check_id, c);
      }
      if (lose) {
        lose = false;
        return reply(503, { kind: "unavailable" });
      }
      return reply(200, { check_id: c.check_id, status: c.status, result: c.result });
    }
    const m = /^\/v1\/project-checks\/(.+)$/.exec(req.url ?? "");
    if (req.method === "GET" && m && checks.has(m[1])) {
      const c = checks.get(m[1]);
      c.status = "passed";
      c.result = { targets: [{ name: "OAI.Toy.main", kind: "theorem", status: "passed" }], isolated: false };
      return reply(200, { check_id: c.check_id, status: c.status, completed_at: "2026-10-14T00:00:00Z", result: c.result });
    }
    reply(404, { kind: "not_found" });
  });
  srv.listen(0, "127.0.0.1");
  await once(srv, "listening");
  return {
    url: `http://127.0.0.1:${(srv.address() as { port: number }).port}`,
    requests,
    checks,
    loseNext: () => (lose = true),
    close: () => new Promise<void>((r) => srv.close(() => r())),
  };
}

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);

test(
  "an imported proof: created by an admin, read like its database, checked by LeanGround, removed with it",
  { skip: !documents },
  async () => {
    const { repo, commit } = await upstream();
    const lean = await leangroundDouble();
    const schema = await isolated();
    const dep = await deploy(schema.url, { url: lean.url, token: "lean-token" }, {
      importSources: { toy: { repo, lean: "lean", workspace: "toy-ws", url: "https://example.org/toy" } },
    });
    const db = `oaic${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    const http = client(() => dep, db);
    try {
      const create = { database: db, source: "toy", config: "Toy" };
      const denied = await http.call("bob", "POST", "/api/imports", create);
      assert.equal(denied.status, 403, JSON.stringify(denied.body));
      const made = await http.ok("alice", "POST", "/api/imports", create);
      assert.equal(made.import.commit, commit);
      assert.equal((await http.call("alice", "POST", "/api/imports", create)).status, 409);
      const graph = await http.doc("alice", "GET", "/graph/full");
      assert.equal(graph.nodes.length, 8);
      // The statement and the files are imported Lean, neither theorems nor definitions to submit.
      assert.deepEqual(
        graph.nodes.map((n: { certification: string | null }) => n.certification ?? "none").sort(),
        ["imported", "imported", "imported", "none", "none", "none", "none", "none"],
      );
      const main = made.import.nodes.modules["OAI.Toy.Main"];
      const shown = async () => (await http.doc("alice", "GET", `/node/${main}/view`)).node;
      assert.equal((await shown()).formalization.lean_certification.status, "imported");
      assert.equal((await shown()).formalization.lean_role, null);
      assert.deepEqual(
        (await http.doc("alice", "GET", "/graph/roots")).map((r: { title: string }) => r.title),
        ["Toy theorem"],
      );

      // Read access follows the database's main branch.
      assert.equal((await http.call("bob", "GET", `/api/imports/${db}`)).status, 404);
      assert.deepEqual(await http.ok("bob", "GET", "/api/imports"), []);
      await http.ok("alice", "POST", "/api/projects", { action: "grant", project: `${db}/main`, actor: "bob", role: "viewer" });
      assert.equal((await http.ok("bob", "GET", "/api/imports")).length, 1);

      // Source browsing: the statement and the proof's closure, nothing else.
      const tree = await http.ok("bob", "GET", `/api/imports/${db}/source`);
      assert.deepEqual(tree.modules.map((m: { module: string }) => m.module), ["OAI.Toy.Basic", "OAI.Toy.Main"]);
      const file = await http.ok("bob", "GET", `/api/imports/${db}/source/file?module=OAI.Toy.Basic`);
      assert.equal(file.text, FILES["lean/OAI/Toy/Basic.lean"]);
      assert.equal(file.url, `https://example.org/toy/blob/${commit}/lean/OAI/Toy/Basic.lean`);
      assert.equal((await http.ok("bob", "GET", `/api/imports/${db}/source/file?module=ComparatorChallenges.Toy`)).path,
        "lean/ComparatorChallenges/Toy.lean");
      for (const module of ["OAI.Toy.Unrelated", "..%2F..%2Fetc%2Fpasswd", "OAI.Toy..Main"])
        assert.equal((await http.call("bob", "GET", `/api/imports/${db}/source/file?module=${module}`)).status, 404);

      // Checks: owners submit; a reply lost after LeanGround queued the check is resent with
      // the same key and runs once.
      assert.equal((await http.call("bob", "POST", `/api/imports/${db}/checks`)).status, 403);
      lean.loseNext();
      const failed = await http.ok("alice", "POST", `/api/imports/${db}/checks`);
      assert.equal(failed.status, "submit_failed");
      const queued = await http.ok("alice", "POST", `/api/imports/${db}/checks`);
      assert.equal(queued.id, failed.id);
      assert.equal(queued.status, "queued");
      assert.equal(lean.checks.size, 1);
      assert.deepEqual(lean.requests.map((r) => r.idempotency_key), [failed.idempotency_key, failed.idempotency_key]);
      assert.deepEqual(lean.requests[0].workspace, "toy-ws");
      assert.deepEqual(lean.requests[0].config.definition_names, ["OAI.Toy.Good"]);
      const read = await http.ok("bob", "GET", `/api/imports/${db}`);
      assert.equal(read.checks.length, 1);
      assert.equal(read.checks[0].status, "passed");
      assert.equal(read.checks[0].result.targets[0].status, "passed");

      // An edited file is ordinary Lean; rebuilding at the import's commit restores the upstream
      // text under the same ids, and the checks stay.
      const before = await shown();
      await http.doc("alice", "PUT", `/node/${main}/block/lean`, { content: "theorem edited : True := trivial\n" }, before.revision);
      assert.equal((await shown()).formalization.lean_certification.status, "not_submitted");
      assert.equal((await http.call("bob", "POST", `/api/imports/${db}/rebuild`)).status, 403);
      const rebuilt = await http.ok("alice", "POST", `/api/imports/${db}/rebuild`);
      assert.deepEqual(rebuilt.import.nodes, made.import.nodes);
      assert.equal(rebuilt.import.created_by, "alice");
      assert.equal(rebuilt.import.rebuilt_by, "alice");
      assert.equal((await shown()).formalization.lean_certification.status, "imported");
      assert.equal((await http.doc("alice", "GET", "/graph/full")).nodes.length, 8);
      assert.equal((await http.ok("bob", "GET", `/api/imports/${db}`)).checks.length, 1);

      // Removing the database removes its import and retires its boards.
      const b = projectBoard(`board-${db}`);
      b.project!.database = db;
      await dep.store.create(b);
      await http.ok("alice", "POST", "/api/projects", { action: "remove", database: db });
      assert.equal((await http.call("alice", "GET", `/api/imports/${db}`)).status, 404);
      const retired = await dep.store.get(b.id, "alice");
      assert.equal(retired.project!.superseded, true);
      assert.equal(retired.project!.removed?.database, db);
      assert.match(retired.project!.database, new RegExp(`^${db}~removed~\\d+$`));
      // A new import under the same name starts clean.
      await http.ok("alice", "POST", "/api/imports", create);
    } finally {
      await removeDatabases(db);
      await dep.close();
      await schema.close();
      await lean.close();
    }
  },
);
