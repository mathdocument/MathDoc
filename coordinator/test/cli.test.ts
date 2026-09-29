// The TypeScript `mdc` against the real document backend (TerminusDB + PostgreSQL).
// Runs with the live harness variables (see harness.ts); LeanGround is only needed for
// the proof commands, which this test does not call.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.js";
import { actors, deploy, isolated, ready } from "./harness.js";

test(
  "mdc drives the document API with an access token",
  { skip: !ready },
  async () => {
    const schema = await isolated();
    const dep = await deploy(schema.url);
    const db = `cli${randomUUID().replaceAll("-", "").slice(0, 12)}`;
    const env = {
      MDC_URL: dep.base,
      MDC_TOKEN: actors.alice.token,
      MDC_PROJECT: `${db}/main`,
    };
    const mdc = (argv: string, input = "") =>
      main(argv.split(" "), env, async () => input) as Promise<any>;
    try {
      await mdc(`init ${db}`);
      const a = await mdc("new -t Alpha");
      const b = await mdc("new -t Beta");
      await mdc("dep add Alpha -t Beta");
      await mdc("edit Alpha --type lean", "theorem alpha : True := trivial\n");
      const shown = await mdc("show Alpha");
      assert.equal(shown.fnode, a.fnode);
      assert.deepEqual(shown.depens, [b.fnode]);
      assert.equal(
        shown.blocks[0].content,
        "theorem alpha : True := trivial\n",
      );
      assert.deepEqual(
        (await mdc("dep show Alpha")).map((n: { title: string }) => n.title),
        ["Beta"],
      );
      assert.deepEqual(
        (await mdc("dep refs Beta")).map((n: { title: string }) => n.title),
        ["Alpha"],
      );
      assert.equal((await mdc("graph check")).edges, 1);
      await mdc("rename Beta Gamma");
      assert.deepEqual(
        (await mdc("search Gam")).map((n: { title: string }) => n.title),
        ["Gamma"],
      );
      // A stale --revision is refused, and the error names the HTTP status.
      await assert.rejects(
        mdc("rename Gamma Delta --revision stale"),
        /HTTP 412/,
      );
      await mdc("dep rm Alpha -t Gamma");
      await mdc("del Gamma");
      const exported = await mdc("export");
      assert.deepEqual(
        exported.nodes.map((n: { title: string }) => n.title),
        ["Alpha"],
      );
      // The exported bundle restores into a fresh branch database.
      const file = join(
        await mkdtemp(join(tmpdir(), "mdc-cli-")),
        "graph.json",
      );
      await writeFile(file, JSON.stringify(exported));
      const copy = `${db}c`;
      await main(["init", copy], env, async () => "");
      await main(["-p", `${copy}/main`, "import", file], env, async () => "");
      assert.equal(
        ((await main(["-p", `${copy}/main`, "graph", "check"], env)) as any)
          .nodes,
        1,
      );
      await assert.rejects(
        main(["show", "Alpha"], { ...env, MDC_TOKEN: actors.eve.token }),
        /HTTP 404/,
      );
      await assert.rejects(main(["frobnicate"], env), /unknown command/);
      // Malformed bodies are client errors, and branch routes accept bundles over 2.2 MB.
      const post = (path: string, body: string) =>
        fetch(`${dep.base}${path}`, {
          method: "POST",
          headers: {
            authorization: `Bearer ${actors.alice.token}`,
            "content-type": "application/json",
          },
          body,
        });
      assert.equal((await post(`/p/${db}/main/api/node/new`, "{")).status, 400);
      const big = await post(
        `/p/${db}/main/api/node/new`,
        JSON.stringify({ title: "x", padding: "y".repeat(3_000_000) }),
      );
      assert.equal(
        big.status,
        422,
        "parsed (then rejected for the unknown field), not 413",
      );
      for (const name of [db, copy]) await mdc(`remove ${name}`);
    } finally {
      await dep.close();
      await schema.close();
    }
  },
);
