// Project stage 7 (docs/mathdoc.md §2.7.8): an agent has LeanGround check a draft (`mdc check`)
// and look lemmas up (`mdc lemma`) through MathDoc, which holds the LeanGround credential. A
// check records nothing and is no submission.
// Real PostgreSQL and TerminusDB; LeanGround is the contract double, or the real service when
// the live harness variables are set (the Lean below is valid in Init and Mathlib).
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

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);

async function flow(leanground: { url: string; token: string }) {
  const schema = await isolated();
  const dep = await deploy(schema.url, leanground);
  const db = `tools${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const http = client(() => dep, db);
  const dir = await mkdtemp(join(tmpdir(), "mdc-tools-"));
  const mdc = (...argv: string[]) =>
    main(
      argv,
      {
        MDC_URL: dep.base,
        MDC_TOKEN: actors.alice.token,
        MDC_PROJECT: `${db}/main`,
      },
      async () => "",
      { cwd: dir, sleep: () => dep.drain() },
    ) as Promise<any>;
  try {
    await http.ok("alice", "POST", "/api/projects", {
      action: "init",
      name: db,
    });
    const root = await http.node(
      "two",
      "theorem two_mul_eq (n : Nat) : n + n = 2 * n := sorry\n",
    );
    await http.ok("alice", "PUT", `/api/coordination/environments/${db}`, {
      base_key: 1,
    });
    const id = (
      await http.ok(
        "alice",
        "POST",
        "/api/coordination/projects",
        { document: { database: db, branch: "main", node: root }, budget: 10 },
        { "idempotency-key": "c" },
      )
    ).id as string;
    await dep.drain();
    const pulled = await mdc("pull", "two");
    const file = join(dir, pulled.file);
    const write = async (proof: string) =>
      writeFile(
        file,
        (await readFile(file, "utf8")).replace(
          /:= by\n[\s\S]*$/,
          `:= by\n  ${proof}\n`,
        ),
      );
    const jobs = async () =>
      (await http.ok("alice", "GET", `/api/coordination/projects/${id}/jobs`))
        .length as number;
    const before = {
      revision: (await http.project(id)).revision,
      jobs: await jobs(),
    };

    // A proof Lean refuses: the check reports Lean's message on the line of the proof.
    await write("rfl -- wrong: the two sides do not reduce to the same term");
    const refused = await mdc("check", pulled.file);
    assert.equal(refused.ok, false, JSON.stringify(refused));
    const line =
      (await readFile(file, "utf8"))
        .split("\n")
        .findIndex((l) => l.includes("-- wrong:")) + 1;
    assert.ok(
      refused.diagnostics.some(
        (d: { line: number; severity: string }) =>
          d.line === line && d.severity === "error",
      ),
      JSON.stringify(refused.diagnostics),
    );
    assert.match(refused.messages, /rfl/);
    assert.ok(refused.messages.startsWith(`${pulled.file}:${line}:`));

    // A proof Lean accepts.
    await write("omega");
    const accepted = await mdc("check", pulled.file);
    assert.equal(accepted.ok, true, JSON.stringify(accepted));
    assert.deepEqual(
      accepted.diagnostics.filter(
        (d: { severity: string }) => d.severity === "error",
      ),
      [],
    );

    // Checking records nothing on the board and queues no job.
    assert.deepEqual(
      { revision: (await http.project(id)).revision, jobs: await jobs() },
      before,
    );

    // Lemma lookup in the project's base.
    const found = await mdc(
      "lemma",
      "Nat.add_comm",
      "--kind",
      "all",
      "-n",
      "5",
    );
    const hit = found.hits.find(
      (h: { name: string }) => h.name === "Nat.add_comm",
    );
    assert.ok(hit, JSON.stringify(found));
    assert.match(hit.type, /n \+ m = m \+ n|m \+ n/);
    assert.equal(hit.matched_query, "Nat.add_comm");
    await assert.rejects(mdc("lemma"), /lemma QUERY/);

    // Only members reach LeanGround through the project.
    const outsider = await http.call(
      "eve",
      "POST",
      `/api/coordination/projects/${id}/check`,
      { source: "theorem t : True := trivial\n" },
    );
    assert.equal(outsider.status, 404, JSON.stringify(outsider));
  } finally {
    await removeDatabases(db);
    await dep.close();
    await schema.close();
  }
}

test(
  "mdc check and mdc lemma reach LeanGround through the project (LeanGround double)",
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
  "mdc check and mdc lemma against the real LeanGround",
  { skip: !ready, timeout: 900000 },
  async () => {
    await flow({ url: env.lean!, token: env.leanToken! });
  },
);
