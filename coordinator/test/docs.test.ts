import test from "node:test";
import assert from "node:assert/strict";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { moduleFile, parseUuid } from "../src/docs/names.js";
import { parseToml } from "../src/docs/toml.js";
import {
  DEFAULT_LEAN_PROJECT,
  type LeanProject,
  type Node,
  validateLatexProject,
  validateLeanProject,
  validateModules,
  DEFAULT_LATEX_PROJECT,
} from "../src/docs/model.js";
import { Snapshot, topoDepths, weakComponentSizes } from "../src/docs/graph.js";
import { LatexService } from "../src/docs/latex.js";
import { DocError } from "../src/docs/names.js";

const node = (title: string, depens: string[] = []): Node => {
  const fnode = randomUUID();
  return {
    fnode,
    title,
    module: `Lib.N_${fnode.replaceAll("-", "")}`,
    depens,
    blocks: [],
  };
};

// Ported from store.rs module_names_preserve_filename_boundaries.
test("module names preserve Lean filename boundaries", () => {
  assert.equal(moduleFile("Lib.EGA.«1-1.7.1»", "lean"), "Lib/EGA/1-1.7.1.lean");
  assert.equal(
    moduleFile("Mathlib.Data.Nat.Basic", "lean"),
    "Mathlib/Data/Nat/Basic.lean",
  );
  assert.equal(moduleFile("Mathlib", "lean"), "Mathlib.lean");
  for (const invalid of [
    "Lib.«..»",
    "Lib.«a/b»",
    "Lib.«a\\b»",
    "Lib.«unclosed",
    "Lib.«a»b",
    "Lib.a.",
    "Lib..a",
    "Lib.«a».",
    "",
    "«.lake».a",
    "lakefile",
  ])
    assert.throws(() => moduleFile(invalid, "lean"), DocError, invalid);
  assert.equal(
    parseUuid("A0B1C2D3-0000-4000-8000-000000000000"),
    "a0b1c2d3-0000-4000-8000-000000000000",
  );
  assert.equal(parseUuid("a0b1c2d3"), null);
});

// Ported from store.rs project_requires_immutable_libraries and
// native_project_preserves_configuration_and_rejects_file_collisions.
test("Lake projects require immutable libraries and safe files", () => {
  const p: LeanProject = structuredClone(DEFAULT_LEAN_PROJECT);
  validateLeanProject(p);
  p.lakefile +=
    '\n[[require]]\nname="example"\ngit="https://example.org/lib.git"\n';
  assert.throws(() => validateLeanProject(p), /external libraries/);
  p.manifest = JSON.stringify({
    packages: [{ name: "example", type: "path", dir: "../lib" }],
  });
  assert.throws(() => validateLeanProject(p), /full Git commit/);
  p.manifest = JSON.stringify({
    packages: [
      {
        name: "example",
        type: "git",
        rev: "0123456789012345678901234567890123456789",
      },
    ],
  });
  validateLeanProject(p);
  const native: LeanProject = {
    ...structuredClone(DEFAULT_LEAN_PROJECT),
    lakefile_name: "lakefile.lean",
    module_root: "Mathlib",
    lakefile: "import Lake\nopen Lake DSL\npackage mathlib\nlean_lib Mathlib\n",
    manifest: '{"packages":[]}',
    files: { "Cache/Main.lean": "def main : IO Unit := pure ()\n" },
  };
  validateLeanProject(native);
  const clash = { ...node("Cache"), module: "Cache.Main" };
  assert.throws(() => validateModules(native, [clash]), /collides/);
  for (const path of [
    "../outside",
    "/absolute",
    "a//b",
    "a/./b",
    "a/../b",
    ".lake/cache/x",
    "lakefile.lean",
    "a\\b",
  ])
    assert.throws(
      () => validateLeanProject({ ...native, files: { [path]: "" } }),
      /unsafe or reserved/,
      path,
    );
  assert.throws(
    () =>
      validateLeanProject({
        ...p,
        lakefile: 'srcDir = "src"\n[[lean_lib]]\nname = "Lib"\n',
      }),
    /custom srcDir/,
  );
  assert.throws(
    () => validateLeanProject({ ...p, toolchain: "leanprover/lean4:nightly" }),
    /pin a Lean toolchain/,
  );
});

test("the TOML subset is exact on what it accepts and fails closed on the rest", () => {
  assert.deepEqual(
    parseToml(
      '# c\nname = "M" # t\nversion = \'0.1\'\nn = 1_000\nb = true\nxs = ["a", "b"]\n[[lean_lib]]\nname = "Lib"\n[[lean_lib]]\n"quoted key" = "\\u00e9\\n"\n[x]\ny = -2\n',
    ),
    {
      name: "M",
      version: "0.1",
      n: 1000,
      b: true,
      xs: ["a", "b"],
      lean_lib: [{ name: "Lib" }, { "quoted key": "é\n" }],
      x: { y: -2 },
    },
  );
  for (const unsupported of [
    "a.b = 1",
    "[a.b]",
    "t = {x = 1}",
    'm = """x"""',
    "f = 1.5",
    "d = 1979-05-27",
    "xs = [\n1]",
    "a = 1\na = 2",
    "[t]\n[t]",
  ])
    assert.throws(
      () => parseToml(unsupported),
      /unsupported lakefile\.toml syntax/,
      unsupported,
    );
  const polluted = parseToml('__proto__ = "x"\n');
  assert.equal(Object.getPrototypeOf(polluted), Object.prototype);
  assert.equal(
    Object.getOwnPropertyDescriptor(polluted, "__proto__")?.value,
    "x",
  );
});

// Ported from store.rs references_and_cycles_are_explicit.
test("snapshots resolve references and reject cycles and module aliases", () => {
  const a = node("A");
  const b = node("B", [a.fnode]);
  const s = new Snapshot("v1", [a, b], structuredClone(DEFAULT_LEAN_PROJECT), {
    ...DEFAULT_LATEX_PROJECT,
  });
  assert.equal(s.resolve("A").fnode, a.fnode);
  assert.throws(() => s.resolve("A.mdoc"), /node not found/);
  assert.throws(() => s.resolve(a.fnode.slice(0, 8)), /node not found/);
  assert.equal(s.resolve(a.fnode.toUpperCase()).fnode, a.fnode);
  assert.throws(
    () => s.validateChanges([{ ...a, depens: [b.fnode] }]),
    /dependency cycle rejected/,
  );
  const alias = { ...node("Alias"), module: `Lib.«${a.module.split(".")[1]}»` };
  assert.equal(moduleFile(alias.module, "lean"), moduleFile(a.module, "lean"));
  assert.throws(() => s.validateChanges([alias]), /already assigned/);
  const original = { ...node("Other"), module: "Lib.Other" };
  assert.throws(
    () => s.validateChanges([original, { ...alias, module: "Lib.«Other»" }]),
    /already assigned/,
  );
  assert.throws(
    () => s.validateChanges([{ ...node("Dangling"), depens: [randomUUID()] }]),
    /does not exist/,
  );
  assert.deepEqual(Object.fromEntries(topoDepths(s.graph())), {
    [a.fnode]: 0,
    [b.fnode]: 1,
  });
  assert.equal(weakComponentSizes(s.graph(), new Set(s.ids())).get(a.fnode), 2);
  s.remove(a.fnode, "v2");
  assert.deepEqual(s.nodes.get(b.fnode)!.depens, []);
});

test("LaTeX project files are named files, not paths", () => {
  validateLatexProject({
    ...DEFAULT_LATEX_PROJECT,
    preamble: "\\newcommand{\\cA}{\\mathcal{A}}",
  });
  for (const name of [
    "../macros.tex",
    "/tmp/macros.tex",
    "macros.sty",
    "a\\b.tex",
  ])
    assert.throws(
      () =>
        validateLatexProject({ ...DEFAULT_LATEX_PROJECT, preamble_name: name }),
      /LaTeX project files/,
      name,
    );
});

test("the LaTeX bridge sends the project once per key and surfaces renderer errors", async () => {
  // A stand-in for the plasTeX worker speaking the same JSON-lines protocol.
  const dir = await mkdtemp(join(tmpdir(), "mdc-fake-latex-"));
  const fake = join(dir, "python");
  await writeFile(
    fake,
    `#!/usr/bin/env node
const rl = require("node:readline").createInterface({ input: process.stdin });
rl.on("line", line => {
  const r = JSON.parse(line);
  if (r.kind === "boom") return console.log(JSON.stringify({ error: "renderer said no" }));
  if (r.kind === "die") process.exit(3);
  console.log(JSON.stringify({ result: { kind: r.kind, got_project: "project" in r } }));
});
`,
  );
  await chmod(fake, 0o755);
  const saved = process.env.MDC_LATEX_PYTHON;
  process.env.MDC_LATEX_PYTHON = fake;
  const service = new LatexService();
  try {
    const project = { ...DEFAULT_LATEX_PROJECT };
    assert.deepEqual(
      await service.request(project, { kind: "catalog", project_key: "k1" }),
      { kind: "catalog", got_project: true },
    );
    assert.deepEqual(
      await service.request(project, { kind: "context", project_key: "k1" }),
      { kind: "context", got_project: false },
    );
    assert.deepEqual(
      await service.request(project, { kind: "context", project_key: "k2" }),
      { kind: "context", got_project: true },
    );
    await assert.rejects(
      service.request(project, { kind: "boom", project_key: "k2" }),
      /renderer said no/,
    );
    await assert.rejects(
      service.request(project, { kind: "die", project_key: "k2" }),
      /renderer stopped/,
    );
    // A restarted worker has no configuration and receives the project again.
    assert.deepEqual(
      await service.request(project, { kind: "context", project_key: "k2" }),
      { kind: "context", got_project: true },
    );
  } finally {
    service.shutdown();
    if (saved === undefined) delete process.env.MDC_LATEX_PYTHON;
    else process.env.MDC_LATEX_PYTHON = saved;
  }
});

// ---- Real TerminusDB + PostgreSQL ------------------------------------------------------

const terminusUrl = process.env.MDC_TEST_TERMINUS_URL;
const terminusPassword = process.env.MDC_TEST_TERMINUS_PASSWORD;
const dsn = process.env.MDC_TEST_DATABASE_URL;

test(
  "document API enforces workspace roles, revisions and conflicts on a real TerminusDB",
  { skip: !(terminusUrl && terminusPassword && dsn) },
  async () => {
    const { Store } = await import("../src/store.js");
    const { server } = await import("../src/http.js");
    const { Database, Terminus } = await import("../src/docs/terminus.js");
    const store = new Store(dsn!);
    await store.migrate();
    const actors = {
      root: { token: "root-token-0123456789ab", admin: true },
      olga: { token: "olga-token-0123456789ab", admin: false },
      eddy: { token: "eddy-token-0123456789ab", admin: false },
      vera: { token: "vera-token-0123456789ab", admin: false },
      zed: { token: "zed-token-0123456789abc", admin: false },
    };
    const http = server(
      {
        dsn: dsn!,
        host: "127.0.0.1",
        port: 0,
        actors,
        leanUrl: "http://127.0.0.1:9",
        leanToken: "unused",
        leanActor: "coordinator",
        terminusUrl,
        terminusUser: "admin",
        terminusPassword,
        webDir: "web/dist",
      },
      store,
      {} as never,
      {} as never,
    );
    await new Promise<void>((ok) => http.listen(0, "127.0.0.1", ok));
    const base = `http://127.0.0.1:${(http.address() as { port: number }).port}`;
    const database = `mdcdoctest${randomUUID().replaceAll("-", "")}`;
    const call = async (
      actor: keyof typeof actors | null,
      method: string,
      path: string,
      body?: unknown,
      revision?: string,
    ) => {
      const headers: Record<string, string> = {
        "content-type": "application/json",
      };
      if (actor) headers.authorization = `Bearer ${actors[actor].token}`;
      if (revision) headers["if-match"] = `"${revision}"`;
      const r = await fetch(`${base}${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      return { status: r.status, body: await r.json().catch(() => null) };
    };
    const api = (p: string) => `/p/${database}/main/api${p}`;
    try {
      assert.equal(
        (
          await call("olga", "POST", "/api/projects", {
            action: "init",
            name: database,
          })
        ).status,
        403,
      );
      assert.equal(
        (
          await call("root", "POST", "/api/projects", {
            action: "init",
            name: database,
          })
        ).status,
        200,
      );
      assert.equal(
        (
          await call("root", "POST", "/api/projects", {
            action: "set_owner",
            database,
            owner: "olga",
          })
        ).status,
        200,
      );
      for (const [actor, role] of [
        ["eddy", "editor"],
        ["vera", "viewer"],
      ] as const)
        assert.equal(
          (
            await call("olga", "POST", "/api/projects", {
              action: "grant",
              project: `${database}/main`,
              actor,
              role,
            })
          ).status,
          200,
        );
      const a = node("A");
      const exported = (await call("olga", "GET", api("/export"))).body;
      assert.equal(
        (
          await call("eddy", "POST", api("/import"), {
            ...exported,
            nodes: [a],
          })
        ).status,
        403,
        "import is owner-only",
      );
      assert.equal(
        (
          await call("olga", "POST", api("/import"), {
            ...exported,
            nodes: [a],
          })
        ).status,
        200,
      );
      // Existence does not leak to actors without a role; the directory hides it too.
      assert.deepEqual((await call("zed", "GET", api("/graph/check"))).body, {
        error: "project not found",
      });
      assert.equal((await call("zed", "GET", api("/graph/check"))).status, 404);
      assert.equal(
        Object.keys(
          (await call("zed", "GET", "/api/projects")).body.projects,
        ).some((p) => p.startsWith(database)),
        false,
      );
      assert.equal((await call(null, "GET", api("/graph/check"))).status, 401);
      // Viewer reads, history and export; no writes, no branches.
      const view = (await call("vera", "GET", api(`/node/${a.fnode}/view`)))
        .body.node;
      assert.equal(
        (
          await call(
            "vera",
            "PUT",
            api(`/node/${a.fnode}/title`),
            { title: "V" },
            view.revision,
          )
        ).status,
        403,
      );
      assert.equal((await call("vera", "GET", api("/history"))).status, 200);
      assert.equal(
        (await call("vera", "POST", api("/branches"), { name: "x" })).status,
        403,
      );
      // Editor writes under If-Match; stale and missing revisions are refused.
      assert.equal(
        (
          await call("eddy", "PUT", api(`/node/${a.fnode}/title`), {
            title: "E",
          })
        ).status,
        428,
      );
      assert.equal(
        (
          await call(
            "eddy",
            "PUT",
            api(`/node/${a.fnode}/title`),
            { title: "E" },
            "0".repeat(64),
          )
        ).status,
        412,
      );
      const renamed = await call(
        "eddy",
        "PUT",
        api(`/node/${a.fnode}/title`),
        { title: "E" },
        view.revision,
      );
      assert.equal(renamed.status, 200);
      // Plan §7.1: certification is its own dimension; this backend never runs a local check.
      assert.equal(renamed.body.formalization.lean_certification, null);
      const withLean = await call(
        "eddy",
        "PUT",
        api(`/node/${a.fnode}/block/lean`),
        { content: "theorem t : True := trivial\n" },
        renamed.body.revision,
      );
      assert.deepEqual(withLean.body.formalization, {
        lean: "unverified",
        rocq: "no_code",
        lean_certification: { status: "not_submitted" },
      });
      assert.equal(
        (await call("eddy", "POST", api("/branches"), { name: "fork" })).status,
        200,
      );
      assert.equal(
        (await call("eddy", "GET", `/p/${database}/fork/api/graph/check`))
          .status,
        200,
        "a fork inherits its origin's grants",
      );
      assert.equal(
        (
          await call("eddy", "POST", "/api/projects", {
            action: "delete_branch",
            project: `${database}/fork`,
          })
        ).status,
        403,
      );
      assert.equal(
        (
          await call("olga", "POST", "/api/projects", {
            action: "delete_branch",
            project: `${database}/fork`,
          })
        ).status,
        200,
      );
      assert.equal(
        (
          await call("olga", "POST", "/api/projects", {
            action: "remove",
            database,
          })
        ).status,
        403,
        "only administrators remove databases",
      );
      // A commit made between a read and a write is a conflict, never an overwrite.
      const db = new Database(
        new Terminus({
          url: terminusUrl!,
          user: "admin",
          password: terminusPassword!,
        }),
        database,
        "main",
      );
      const stale = await db.version();
      await db.putBundle(
        [{ ...a, title: "Moved" }],
        null,
        null,
        stale,
        "external",
        "test",
      );
      await assert.rejects(
        db.putBundle(
          [{ ...a, title: "Lost" }],
          null,
          null,
          stale,
          "late",
          "test",
        ),
        (e: DocError) => e.status === 409,
      );
      assert.equal(
        (await call("olga", "GET", api(`/node/${a.fnode}/view`))).body.node
          .title,
        "Moved",
        "the service reloads the moved branch",
      );
    } finally {
      await call("root", "POST", "/api/projects", {
        action: "remove",
        database,
      });
      await new Promise((ok) => http.close(ok));
      await store.pool.end();
    }
  },
);
