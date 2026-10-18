// Project stage 2 (docs/mathdoc.md §2.7.2): an agent splits a goal and proves it with only
// `mdc pull / split / prove / done`, and a rerun after a lost reply submits nothing twice.
// Real PostgreSQL and TerminusDB. LeanGround is the contract double (no Lean proof is checked);
// with the live harness variables set, the same flow also runs against the real LeanGround.
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.js";
import { goalOf } from "../src/mdc/verbs.js";
import { type Client, MdcError } from "../src/mdc/client.js";
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

// Valid Lean in the Init base, so that the real LeanGround accepts the same flow.
const D = "def double (n : Nat) : Nat := n + n\n";
const R = "theorem double_add (n : Nat) : double n + 0 = n + n := sorry\n";
const SPLIT_PROOF = "rw [Nat.add_zero, double_eq]";
const PLACEHOLDER = "theorem double_eq (n : Nat) : double n = n + n := sorry\n";

async function flow(leanground: { url: string; token: string }) {
  const schema = await isolated();
  const db = `verbs${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const dep = await deploy(schema.url, leanground);
  const http = client(() => dep, db);
  const dir = await mkdtemp(join(tmpdir(), "mdc-verbs-"));
  // The worker runs between polls: `--wait` sleeps by draining the job queue.
  let fault:
    | ((url: string, init?: RequestInit) => "before" | "after" | undefined)
    | undefined;
  let clock = 0;
  const io = {
    cwd: dir,
    now: () => Date.now() + clock,
    sleep: () => dep.drain(),
    fetch: (async (url: string | URL | Request, init?: RequestInit) => {
      const when = fault?.(String(url), init);
      if (when === "before") throw new TypeError("connection refused");
      const response = await fetch(url, init);
      if (when === "after") throw new TypeError("socket hang up");
      return response;
    }) as typeof fetch,
  };
  const mdc = (...argv: string[]) =>
    main(
      argv,
      {
        MDC_URL: dep.base,
        MDC_TOKEN: actors.bob.token,
        MDC_PROJECT: `${db}/main`,
      },
      async () => "",
      io,
    ) as Promise<any>;
  const edit = async (file: string, fn: (text: string) => string) =>
    writeFile(join(dir, file), fn(await readFile(join(dir, file), "utf8")));
  const record = async (file: string) =>
    JSON.parse(
      await readFile(
        join(dir, ".mdc", "files", `${encodeURIComponent(file)}.json`),
        "utf8",
      ),
    );
  const submits = async (id: string) =>
    (
      await http.ok("bob", "GET", `/api/coordination/projects/${id}/jobs`)
    ).filter((j: { kind: string }) => j.kind === "submit").length;
  try {
    // A project whose root theorem is open; bob is a member with viewer rights on main.
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
    assert.ok(p.ready, JSON.stringify(p.branches));

    // pull: by node title; the file states the goal in the project's environment.
    const pulled = await mdc("pull", "double_add", "--ttl", "120");
    assert.equal(pulled.goal, p.root);
    assert.equal(pulled.statement_from, "node");
    assert.equal(pulled.lease.actor, "bob");
    const file = pulled.file as string;
    assert.equal(file, `${p.root.slice(0, 8)}-double_add.lean`);
    const text = await readFile(join(dir, file), "utf8");
    assert.ok(text.includes(`import ${p.base.root_module}\n`));
    assert.ok(text.includes(`-- definition double (`));
    assert.ok(text.includes(D));
    p = await http.project(id);
    assert.equal(
      Object.values(p.tasks).find((t) => t.goal === p.root)!.state,
      "leased",
    );
    await assert.rejects(mdc("pull", "double_add"), /file_exists/);
    // A lease is not exclusive: another agent learns who holds the task and may go on without one.
    const elsewhere = await mkdtemp(join(tmpdir(), "mdc-verbs-"));
    const alice = (...argv: string[]) =>
      main(
        argv,
        {
          MDC_URL: dep.base,
          MDC_TOKEN: actors.alice.token,
          MDC_PROJECT: `${db}/main`,
        },
        async () => "",
        { ...io, cwd: elsewhere },
      ) as Promise<any>;
    await assert.rejects(
      alice("pull", "double_add"),
      /bob holds the task until .*--no-claim/,
    );
    assert.equal((await alice("pull", "double_add", "--no-claim")).lease, null);

    // Every verb renews leases past half their time, even when it refuses the file.
    clock = 70_000;
    await assert.rejects(mdc("prove", file), /root_is_sorry/);
    const lease = (await record(file)).lease;
    assert.ok(Date.parse(lease.expires) > Date.parse(pulled.lease.expires));
    clock = 0;

    // split: a new placeholder becomes a subgoal, stated automatically from the sketch.
    await edit(file, (t) =>
      t
        .replace("\n-- Goal.", `\n${PLACEHOLDER}\n-- Goal.`)
        .replace(":= by\n  sorry\n", `:= by\n  ${SPLIT_PROOF}\n`),
    );
    await assert.rejects(mdc("prove", file), /has_premises/);
    // --dry-run shows the submission without sending it.
    const dry = await mdc("split", file, "-m", "x", "--dry-run");
    assert.equal(dry.mode, "sketch");
    assert.deepEqual(dry.premises, ["double_eq"]);
    assert.deepEqual(dry.dropped, []);
    assert.ok(
      dry.source.includes(PLACEHOLDER) && dry.source.includes(SPLIT_PROOF),
    );
    assert.ok(!dry.source.includes("import"));
    assert.equal(await submits(id), 0);
    await assert.rejects(mdc("split", file), /usage|DESCRIPTION/);
    // A description over the limit names the field and the limit, not only invalid_request.
    await assert.rejects(mdc("split", file, "-m", "x".repeat(2001)), /description: .*2000/);
    const split = await mdc("split", file, "-m", "unfold double", "--wait");
    assert.equal(split.status, "certified", JSON.stringify(split));
    assert.equal(split.mode, "sketch");
    assert.equal(split.premise_goals.length, 1);
    const [sub] = split.premise_goals;
    assert.deepEqual(
      { name: sub.name, new: sub.new, statement: sub.statement },
      { name: "double_eq", new: true, statement: "resolved" },
    );
    p = await http.project(id);
    const fact = p.facts[split.certificate];
    assert.equal(fact.description, "unfold double");
    assert.deepEqual(fact.premises, [sub.goal]);
    assert.ok(p.goals[sub.goal].identity, "the subgoal can take a proof");
    assert.equal(p.goals[sub.goal].declaration!.name, "double_eq");
    assert.equal(p.goals[sub.goal].statement, "double_eq");
    // Stage 3: an agent's decomposition waits for the owner; its subgoal schedules nothing.
    assert.equal(split.certificate_state, "proposed");
    assert.deepEqual([fact.state, fact.submitter], ["proposed", "bob"]);
    await assert.rejects(mdc("pull", sub.goal.slice(0, 10)), /inactive_goal/);
    const noted = await mdc(
      "note",
      sub.goal.slice(0, 10),
      "-m",
      "unfolding double should be enough",
    );
    assert.equal(noted.goal, sub.goal);
    await assert.rejects(mdc("note", sub.goal.slice(0, 10), "-m", "x".repeat(501)), /text: .*500/);
    await http.done(id, "alice", {
      type: "decomposition",
      certificate: split.certificate,
      state: "active",
      comment: "the right lemma",
    });
    // Goals record only definitions LeanGround reports as referenced, never IDs from a request:
    // drafts return their source. The coordinator can see another project's definition (it is
    // a member of every project), so a member names one in a resolve of this project's goal.
    const defined = p.goals[sub.goal].definitions!;
    assert.equal(defined.length, 1);
    const other = `${db}x`;
    const otherHttp = client(() => dep, other);
    await otherHttp.ok("alice", "POST", "/api/projects", {
      action: "init",
      name: other,
    });
    const secret = await otherHttp.node(
      "secret",
      "def secret_value : Nat := 42\n",
    );
    const otherRoot = await otherHttp.node(
      "secret_fact",
      "theorem secret_fact : secret_value = 42 := sorry\n",
      [secret],
    );
    await otherHttp.ok(
      "alice",
      "PUT",
      `/api/coordination/environments/${other}`,
      { base_key: 1 },
    );
    const otherId = (
      await otherHttp.ok(
        "alice",
        "POST",
        "/api/coordination/projects",
        {
          document: { database: other, branch: "main", node: otherRoot },
          budget: 1,
        },
        { "idempotency-key": "create-other" },
      )
    ).id as string;
    await dep.drain();
    const foreign = (await otherHttp.project(otherId)).branches!.main.nodes[
      secret
    ].definition_ids![0];
    assert.ok(foreign && !defined.includes(foreign));
    await http.done(id, "bob", {
      type: "resolve",
      goal: sub.goal,
      proposition: "∀ (n : Nat), double n = n + n",
      definitions: [...defined, foreign],
    });
    p = await http.project(id);
    assert.deepEqual(p.goals[sub.goal].definitions, defined);
    // Rerunning the same split reports the first submission.
    const again = await mdc("split", file, "-m", "unfold double");
    assert.equal(again.replayed, true);
    assert.equal(again.key, split.key);
    assert.equal(await submits(id), 1);

    // pull the subgoal by a key prefix; its statement comes from the sketch.
    const second = await mdc("pull", sub.goal.slice(0, 10));
    assert.equal(second.statement_from, "declaration");
    assert.equal(second.root, "double_eq");
    const file2 = second.file as string;
    const text2 = await readFile(join(dir, file2), "utf8");
    assert.ok(text2.includes(D));
    // The note is in the read-only prefix.
    const prefix2 = text2.slice(
      0,
      text2.indexOf("-- mdc: edit below this line"),
    );
    assert.match(prefix2, /-- \[\S+ bob\] unfolding double should be enough\n/);
    await edit(file2, (t) => t.replace(":= by\n  sorry\n", ":= by\n  rfl\n"));

    // Replay 1: the reply to prove is lost after the server ran it; meanwhile the board moves on.
    fault = (url, init) =>
      init?.method === "POST" &&
      url.endsWith("/commands") &&
      String(
        (init.headers as Record<string, string>)["idempotency-key"],
      ).startsWith("submit-")
        ? "after"
        : undefined;
    await assert.rejects(mdc("prove", file2), /run the same command again/);
    fault = undefined;
    assert.equal((await record(file2)).submissions[0].status, "sending");
    await http.done(id, "bob", { type: "notes", notes: "a write in between" });
    const proved = await mdc("prove", file2, "--wait");
    assert.equal(proved.status, "certified", JSON.stringify(proved));
    assert.equal(proved.replayed, false);
    assert.equal(
      await submits(id),
      2,
      "the lost reply was not submitted twice",
    );
    assert.equal((await record(file2)).submissions.length, 1);

    // done: both leases end; the outcome follows the submissions.
    const finished = await mdc(
      "done",
      file2,
      "--spent",
      "0",
      "--note",
      "rfl closes it",
    );
    assert.deepEqual(
      { lease: finished.lease, outcome: finished.outcome },
      { lease: "finished", outcome: "submitted" },
    );
    const last = await mdc("done");
    assert.equal(last.file, file);
    assert.equal(last.outcome, "submitted");
    await assert.rejects(mdc("done"), /no_lease/);

    // The split and the proof close main's route: the root assembles.
    p = await http.project(id);
    assert.deepEqual(
      p.goals[sub.goal].notes!.map((n) => n.text),
      ["unfolding double should be enough", "rfl closes it"],
    );
    assert.equal(p.goals[sub.goal].status, "certified");
    assert.equal(p.goals[p.root].status, "derivable");
    await http.done(id, "alice", {
      type: "assemble",
      component: "lean-worker",
      branch: "main",
    });
    p = await http.project(id);
    assert.ok(Object.values(p.runs).some((r) => r.status === "certified"));

    // Replay 2: the request is lost before it arrives, and the revision moves on. `submit` is
    // checked against the current state, not the revision (§2.7.5): the resend applies.
    const third = await mdc("pull", sub.goal, "--no-claim", "-o", "again.lean");
    assert.equal(third.lease, null);
    await edit("again.lean", (t) =>
      t.replace(":= by\n  sorry\n", ":= by\n  unfold double; rfl\n"),
    );
    fault = (url, init) =>
      init?.method === "POST" && url.endsWith("/commands")
        ? "before"
        : undefined;
    await assert.rejects(
      mdc("prove", "again.lean"),
      /run the same command again/,
    );
    fault = undefined;
    await http.done(id, "bob", { type: "notes", notes: "another write" });
    const before = await submits(id);
    const resent = await mdc("prove", "again.lean");
    assert.equal(resent.status, "queued");
    const subs = (await record("again.lean")).submissions;
    assert.deepEqual(
      subs.map((s: { status: string }) => s.status),
      ["queued"],
    );
    assert.equal(await submits(id), before + 1);
  } finally {
    await removeDatabases(db, `${db}x`);
    await dep.close();
    await schema.close();
  }
}

test(
  "an agent splits and proves a goal with the four verbs (LeanGround double)",
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
  "the same flow against the real LeanGround",
  { skip: !ready, timeout: 900000 },
  async () => {
    await flow({ url: env.lean!, token: env.leanToken! });
  },
);

test("a goal without a node is found by its name; a name two goals share is refused", async () => {
  // The branch has no node by that title.
  const c = {
    request: async () => {
      throw new MdcError("not_found", "no such node", 404);
    },
  } as unknown as Client;
  const goal = (name: string) => ({ statement: name, declaration: { name } });
  const [a, b, d] = ["a", "b", "d"].map((x) => x.repeat(64));
  const board = {
    goals: { [a]: goal("double_comm"), [b]: goal("twin"), [d]: goal("twin") },
    branches: {},
  };
  assert.equal(await goalOf(c, board, "double_comm", "db", "main"), a);
  await assert.rejects(
    goalOf(c, board, "twin", "db", "main"),
    (e: any) => e.reason === "ambiguous_goal",
  );
  await assert.rejects(
    goalOf(c, board, "nothing", "db", "main"),
    (e: any) => e.reason === "unknown_goal",
  );
});
