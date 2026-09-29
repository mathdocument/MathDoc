// Plan stage 2 acceptance: the legacy Rust backend and the TypeScript backend share one
// TerminusDB database. Revisions must agree byte for byte, and alternating writes may only
// fail as revision conflicts. Requires a built `mdc` (MDC_TEST_RUST_BIN) plus the
// TerminusDB and PostgreSQL test variables used by docs.test.ts.
import test from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { createServer } from "node:net";

const run = promisify(execFile);
const rust = process.env.MDC_TEST_RUST_BIN;
const terminusUrl = process.env.MDC_TEST_TERMINUS_URL;
const terminusPassword = process.env.MDC_TEST_TERMINUS_PASSWORD;
const dsn = process.env.MDC_TEST_DATABASE_URL;

test(
  "Rust and TypeScript backends agree on revisions and interleave writes",
  { skip: !(rust && terminusUrl && terminusPassword && dsn), timeout: 180000 },
  async () => {
    const { Store } = await import("../src/store.js");
    const { server } = await import("../src/http.js");
    const store = new Store(dsn!);
    await store.migrate();
    const token = "cross-token-0123456789";
    const http = server(
      {
        dsn: dsn!,
        host: "127.0.0.1",
        port: 0,
        actors: { cross: { token, admin: true } },
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
    const ts = `http://127.0.0.1:${(http.address() as { port: number }).port}`;
    const database = `mdccross${randomUUID().replaceAll("-", "")}`;
    // An isolated Rust service: its own config, cache and port; never the user's.
    const home = await mkdtemp(join(tmpdir(), "mdc-cross-"));
    await writeFile(join(home, "config.toml"), "");
    const rustEnv = {
      ...process.env,
      MDC_CONFIG: join(home, "config.toml"),
      MDC_CACHE_DIR: join(home, "cache"),
      MDC_TERMINUS_URL: terminusUrl,
      MDC_TERMINUS_PASSWORD: terminusPassword,
    };
    const call = async (
      base: string,
      method: string,
      path: string,
      body?: unknown,
      revision?: string,
    ) => {
      const headers: Record<string, string> = {
        "content-type": "application/json",
      };
      if (base === ts) headers.authorization = `Bearer ${token}`;
      if (revision) headers["if-match"] = `"${revision}"`;
      const r = await fetch(`${base}${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      return { status: r.status, body: await r.json().catch(() => null) };
    };
    const ok = async (
      base: string,
      method: string,
      path: string,
      body?: unknown,
      revision?: string,
    ) => {
      const r = await call(base, method, path, body, revision);
      assert.ok(
        r.status < 300,
        `${base === ts ? "TS" : "Rust"} ${method} ${path}: ${r.status} ${JSON.stringify(r.body)}`,
      );
      return r.body;
    };
    const api = (p: string) => `/p/${database}/main/api${p}`;
    const fnode = () => randomUUID();
    const [a, b, c] = [fnode(), fnode(), fnode()];
    const mk = (
      id: string,
      title: string,
      depens: string[] = [],
      blocks: unknown[] = [],
    ) => ({
      fnode: id,
      title,
      module: `Lib.N_${id.replaceAll("-", "")}`,
      depens,
      blocks,
    });
    let rustBase = "";
    try {
      await ok(ts, "POST", "/api/projects", { action: "init", name: database });
      const exported = await ok(ts, "GET", api("/export"));
      await ok(ts, "POST", api("/import"), {
        ...exported,
        nodes: [
          mk(
            a,
            "Ordering ∀ℕ",
            [],
            [
              {
                srctype: "lean",
                content: "theorem t : True := trivial\n",
                metadata: {
                  "10": "ten",
                  "9": "nine",
                  "a,b": "comma",
                  a: "plain",
                  "￿": "bmp",
                  "😀": "astral",
                },
              },
            ],
          ),
          mk(b, "Beta", [c]),
          mk(
            c,
            'Gamma \\ "quoted"',
            [],
            [
              {
                srctype: "latex",
                content: "$\\mathbb{N}$\u0001\n",
                metadata: {},
              },
            ],
          ),
        ],
      });
      const reservation = createServer();
      await new Promise<void>((done) =>
        reservation.listen(0, "127.0.0.1", done),
      );
      const port = (reservation.address() as { port: number }).port;
      await new Promise((done) => reservation.close(done));
      const { stdout } = await run(
        rust!,
        ["start", `${database}/main`, "--port", String(port)],
        { env: rustEnv, timeout: 60000 },
      );
      rustBase = new URL(JSON.parse(stdout).url).origin;
      const revisions = async (base: string) =>
        Object.fromEntries(
          await Promise.all(
            [a, b, c].map(async (id) => [
              id,
              (await ok(base, "GET", api(`/node/${id}/view`))).node.revision,
            ]),
          ),
        );
      assert.deepEqual(
        await revisions(rustBase),
        await revisions(ts),
        "both backends compute identical node revisions",
      );

      // Alternating writes; each backend sees the other's commits.
      let r = await revisions(rustBase);
      await ok(
        ts,
        "PUT",
        api(`/node/${a}/title`),
        { title: "Renamed by TS" },
        r[a],
      );
      r = await revisions(rustBase);
      await ok(
        rustBase,
        "PUT",
        api(`/node/${a}/block/latex`),
        { content: "by Rust" },
        r[a],
      );
      r = await revisions(ts);
      assert.deepEqual(await revisions(rustBase), r);
      await ok(ts, "POST", api(`/node/${a}/dep/add`), { dep_fnode: b }, r[a]);
      assert.deepEqual(
        (await ok(rustBase, "GET", api(`/node/${a}/view`))).node.depens,
        [b],
      );
      r = await revisions(rustBase);
      await ok(rustBase, "DELETE", api(`/node/${c}`), undefined, r[c]);
      assert.deepEqual(
        (await ok(ts, "GET", api(`/node/${b}/view`))).node.depens,
        [],
        "TS sees Rust's detach",
      );
      const latex = await ok(ts, "GET", api("/project/latex"));
      await ok(
        ts,
        "PUT",
        api("/project/latex"),
        { ...latex.project, preamble: "\\newcommand{\\N}{\\mathbb{N}}" },
        latex.revision,
      );
      assert.equal(
        (await ok(rustBase, "GET", api("/project/latex"))).project.preamble,
        "\\newcommand{\\N}{\\mathbb{N}}",
      );
      const lean = await ok(rustBase, "GET", api("/project/lean"));
      assert.deepEqual(
        (await ok(ts, "GET", api("/project/lean"))).project,
        lean.project,
        "identical Lean project wire form",
      );

      // Stale revisions are refused by both, as revision failures only.
      // After Rust deleted c, compare the remaining node directly.
      const revisionOf = async (base: string, id: string) =>
        (await ok(base, "GET", api(`/node/${id}/view`))).node.revision;
      const stale = await revisionOf(ts, a);
      await ok(ts, "PUT", api(`/node/${a}/title`), { title: "Current" }, stale);
      assert.equal(
        (await call(ts, "PUT", api(`/node/${a}/title`), { title: "x" }, stale))
          .status,
        412,
      );
      assert.equal(
        (
          await call(
            rustBase,
            "PUT",
            api(`/node/${a}/title`),
            { title: "x" },
            stale,
          )
        ).status,
        412,
      );
      assert.equal(await revisionOf(ts, a), await revisionOf(rustBase, a));
      assert.equal((await ok(ts, "GET", api("/graph/check"))).nodes, 2);
      assert.equal((await ok(rustBase, "GET", api("/graph/check"))).nodes, 2);
    } finally {
      if (rustBase)
        await run(rust!, ["stop"], { env: rustEnv, timeout: 60000 }).catch(
          () => undefined,
        );
      await call(ts, "POST", "/api/projects", { action: "remove", database });
      await new Promise((done) => http.close(done));
      await store.pool.end();
    }
  },
);
