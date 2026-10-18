// Project stage 6 (docs/mathdoc.md §2.7.7): the owner reads goals, reviews decompositions and
// opens a route branch with only `mdc`; a branch's editor changes its picks, others cannot; and
// review, pick and proof command take effect once when rerun after a lost reply.
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
  type Actor,
} from "./harness.js";
import { leangroundDouble } from "./leanground-double.js";
import type { Board } from "../src/domain.js";

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);
const D = "def double (n : Nat) : Nat := n + n\n";
const R = "theorem double_add (n : Nat) : double n + 0 = n + n := sorry\n";
// Two ways to split the root: through double_eq, or through double_comm.
const EQ = "theorem double_eq (n : Nat) : double n = n + n := sorry\n";
const EQ_PROOF = "by\n  rw [Nat.add_zero, double_eq]\n";
const COMM = "theorem double_comm (n : Nat) : n + n = double n := sorry\n";
const COMM_PROOF = "by\n  rw [Nat.add_zero]\n  exact (double_comm n).symm\n";

async function flow(leanground: { url: string; token: string }) {
  const schema = await isolated();
  const db = `maint${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const dep = await deploy(schema.url, leanground);
  const http = client(() => dep, db);
  // A reply lost after the server ran the request: the client sees a dropped connection.
  let lose: ((url: string, init?: RequestInit) => boolean) | undefined;
  const io = (cwd: string) => ({
    cwd,
    sleep: () => dep.drain(),
    fetch: (async (url: string | URL | Request, init?: RequestInit) => {
      const response = await fetch(url, init);
      if (lose?.(String(url), init)) {
        lose = undefined;
        throw new TypeError("socket hang up");
      }
      return response;
    }) as typeof fetch,
  });
  const dirs: Partial<Record<Actor, string>> = {};
  const as = async (who: Actor, token = actors[who].token) => {
    const dir = (dirs[who] ??= await mkdtemp(
      join(tmpdir(), `mdc-maint-${who}-`),
    ));
    return (argv: string[], stdin = "") =>
      main(
        argv,
        {
          MDC_URL: dep.base,
          MDC_TOKEN: token,
          MDC_PROJECT: `${db}/main`,
          MDC_STATE_DIR: join(dir, "state"),
        },
        async () => stdin,
        io(dir),
      ) as Promise<any>;
  };
  const commands = (id: string) => (url: string, init?: RequestInit) =>
    init?.method === "POST" && url.endsWith(`/projects/${id}/commands`);
  try {
    await http.ok("alice", "POST", "/api/projects", {
      action: "init",
      name: db,
    });
    for (const who of ["bob", "carol"])
      await http.ok("alice", "POST", "/api/projects", {
        action: "grant",
        project: `${db}/main`,
        actor: who,
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
          members: ["bob", "carol"],
          budget: 1000,
        },
        { "idempotency-key": "create" },
      )
    ).id as string;
    await dep.drain();
    const status = async (c: typeof http, node: string) =>
      (await c.view(node)).formalization.lean_certification.status;
    // A definition has no goal: once LeanGround registered it, it shows as registered.
    assert.equal(await status(http, d), "registered");
    const events = async (kind: string) =>
      (
        await http.ok(
          "alice",
          "GET",
          `/api/coordination/projects/${id}/history`,
        )
      ).filter((e: { kind: string }) => e.kind === kind).length;

    // An agent proposes two ways to split the root.
    const bob = await as("bob");
    const split = async (
      output: string,
      premise: string,
      proof: string,
      m: string,
    ) => {
      const pulled = await bob([
        "pull",
        "double_add",
        "--no-claim",
        "-o",
        output,
      ]);
      const file = join(dirs.bob!, pulled.file);
      await writeFile(
        file,
        (await readFile(file, "utf8"))
          .replace("\n-- Goal.", `\n${premise}\n-- Goal.`)
          .replace(":= by\n  sorry\n", `:= ${proof}`),
      );
      const s = await bob(["split", pulled.file, "-m", m, "--wait"]);
      assert.equal(s.status, "certified", JSON.stringify(s));
      assert.equal(s.certificate_state, "proposed");
      return s;
    };
    const d1 = await split("eq.lean", EQ, EQ_PROOF, "unfold double");
    const d2 = await split("comm.lean", COMM, COMM_PROOF, "commute first");
    const [eq] = d1.premise_goals;
    const [comm] = d2.premise_goals;

    // The owner reads the goals and reviews, with mdc only.
    const alice = await as("alice");
    let p: Board = await http.project(id);
    const goals = await alice(["goals"]);
    const rootLine = goals.goals[0];
    assert.deepEqual(
      [rootLine.goal, rootLine.name, rootLine.root, rootLine.certificates],
      [p.root, "double_add", true, { proposed: 2 }],
    );
    assert.deepEqual(rootLine.needed_by, ["main"]);
    // Subgoals of proposed decompositions are on no route yet.
    const eqLine = goals.goals.find(
      (g: { goal: string }) => g.goal === eq.goal,
    );
    assert.deepEqual([eqLine.name, eqLine.needed_by], ["double_eq", []]);
    const pending = await alice(["review"]);
    assert.deepEqual(
      pending.decompositions.map((x: { id: string }) => x.id),
      [d1.certificate, d2.certificate],
    );
    assert.deepEqual(
      [
        pending.decompositions[0].description,
        pending.decompositions[0].submitter,
        pending.decompositions[0].goal.name,
        pending.decompositions[0].premises.map((x: { name: string }) => x.name),
      ],
      ["unfold double", "bob", "double_add", ["double_eq"]],
    );
    // The review view reads the decomposition's source with the definitions it uses.
    const read = await http.ok(
      "alice",
      "GET",
      `/api/coordination/projects/${id}/facts/${d1.certificate}`,
    );
    assert.match(read.source, /theorem double_eq/);
    assert.deepEqual(
      read.definition_sources.map((x: { name: string; source: string }) => [
        x.name,
        x.source.trim(),
      ]),
      [["double", D.trim()]],
    );
    await assert.rejects(
      alice(["review", d1.certificate.slice(0, 10)]),
      /--accept/,
    );
    const accepted = await alice([
      "review",
      d1.certificate.slice(0, 10),
      "--accept",
      "-m",
      "the right lemma",
    ]);
    assert.deepEqual(
      [accepted.state, accepted.review.comment, accepted.resent],
      ["active", "the right lemma", false],
    );
    // Only decompositions still awaiting review are listed.
    assert.deepEqual(
      (await alice(["review"])).decompositions.map((x: { id: string }) => x.id),
      [d2.certificate],
    );
    // A non-owner cannot review.
    await assert.rejects(
      bob(["review", d2.certificate, "--accept"]),
      /token_scope_required|owner_required/,
    );

    // Replay: the reply to a review is lost after it ran; the board moves on meanwhile.
    const reviews = await events("decomposition");
    lose = commands(id);
    await assert.rejects(
      alice(["review", d2.certificate, "--accept"]),
      /run the same command again/,
    );
    await http.done(id, "alice", {
      type: "notes",
      notes: "a write in between",
    });
    const again = await alice(["review", d2.certificate, "--accept"]);
    assert.deepEqual([again.state, again.resent], ["active", true]);
    assert.equal(await events("decomposition"), reviews + 1, "applied once");

    const detail = await alice(["goal", "double_add"]);
    assert.deepEqual(
      detail.certificates
        .map((c: { id: string; state: string }) => [c.id, c.state])
        .sort(),
      [
        [d1.certificate, "active"],
        [d2.certificate, "active"],
      ].sort(),
    );
    assert.equal(detail.needed_by[0], "main");
    // double_comm has no node yet: it is found by the name `mdc goals` shows.
    assert.equal(
      (await alice(["goal", "double_comm"])).premise_of[0].certificate,
      d2.certificate,
    );

    // A route branch through the second split, opened and picked with one command.
    const opened = await alice([
      "branch",
      "new",
      "alt",
      "--pick",
      `double_add=${d2.certificate.slice(0, 12)}`,
    ]);
    assert.deepEqual(opened.picks, { [p.root]: d2.certificate });
    p = await http.project(id);
    assert.ok(p.branches!.alt.request.bound);
    // On alt only the root and double_comm are needed; main still needs both subgoals.
    const onAlt = (await alice(["goals", "--branch", "alt"])).goals.map(
      (g: { goal: string }) => g.goal,
    );
    assert.deepEqual(onAlt.sort(), [p.root, comm.goal].sort());

    // The agent proves double_comm; the owner assembles alt through the second split.
    const proving = await bob(["pull", "double_comm", "-o", "c.lean"]);
    assert.equal(proving.goal, comm.goal);
    const cfile = join(dirs.bob!, proving.file);
    await writeFile(
      cfile,
      (await readFile(cfile, "utf8")).replace(
        ":= by\n  sorry\n",
        ":= by\n  rfl\n",
      ),
    );
    const proved = await bob(["prove", proving.file, "--wait"]);
    assert.equal(proved.status, "certified", JSON.stringify(proved));
    await bob(["done", proving.file]);
    await http.done(id, "alice", {
      type: "assemble",
      component: "lean-worker",
      branch: "alt",
    });
    p = await http.project(id);
    const run = Object.values(p.runs).find((r) => r.branch === "alt")!;
    assert.equal(run.status, "certified", JSON.stringify(run));
    assert.equal(run.route.root, d2.certificate);
    // Before accepting, the preview shows what the batch would write, and writes nothing.
    const previewPath = `/api/coordination/projects/${id}/runs/${run.id}/preview`;
    const preview = await http.ok("alice", "GET", previewPath);
    assert.equal(preview.status, "ready", JSON.stringify(preview));
    assert.equal(typeof preview.assembled, "string");
    const change = (kind: string) =>
      preview.changes.find((c: { kind: string }) => c.kind === kind);
    assert.deepEqual(
      preview.changes.map((c: { kind: string }) => c.kind).sort(),
      ["create_goal_node", "update_lean_block"],
    );
    assert.equal(change("update_lean_block").title, "double_add");
    assert.match(change("update_lean_block").before, /:= sorry/);
    assert.match(change("update_lean_block").after, /double_comm/);
    assert.equal(change("create_goal_node").before, null);
    assert.match(change("create_goal_node").after, /theorem double_comm/);
    assert.deepEqual((await http.project(id)).branches!.alt.writebacks, {});
    // Accepting writes alt's proof back: double_comm had no node, so the batch creates one,
    // which shows as certified on alt like the nodes converted from the document.
    await http.done(id, "alice", { type: "accept", run: run.id });
    p = await http.project(id);
    const batch = Object.values(p.branches!.alt.writebacks)[0];
    assert.equal(batch.status, "committed");
    // Once written, the preview names the batch that was.
    const after = await http.ok("alice", "GET", previewPath);
    assert.deepEqual(
      [after.status, after.writeback.batch_id],
      ["written", batch.batch_id],
    );
    assert.equal(batch.created.length, 1);
    const altDocs = client(() => dep, db, "alt");
    assert.equal(await status(altDocs, batch.created[0]), "certified");
    assert.equal(await status(altDocs, root), "certified");
    assert.equal(await status(altDocs, d), "registered");

    // Picks: alt's editor may change them; a reader of alt and an editor elsewhere may not.
    await http.ok("alice", "POST", "/api/projects", {
      action: "grant",
      project: `${db}/alt`,
      actor: "bob",
      role: "editor",
    });
    await http.ok("alice", "POST", "/api/projects", {
      action: "grant",
      project: `${db}/alt`,
      actor: "carol",
      role: "viewer",
    });
    const cleared = await bob([
      "pick",
      "double_add",
      "auto",
      "--branch",
      "alt",
    ]);
    assert.deepEqual([cleared.certificate, cleared.picks], [null, {}]);
    const back = await bob([
      "pick",
      "double_add",
      d2.certificate.slice(0, 12),
      "--branch",
      "alt",
    ]);
    assert.deepEqual(back.picks, { [p.root]: d2.certificate });
    const carol = await as("carol");
    await assert.rejects(
      carol(["pick", "double_add", "auto", "--branch", "alt"]),
      (e: any) => e.status === 403 && e.reason === "document_permission_denied",
    );
    await assert.rejects(
      bob(["pick", "double_add", d1.certificate, "--branch", "main"]),
      (e: any) => e.status === 403 && e.reason === "document_permission_denied",
    );
    // A delegated token needs `edit` to pick as an editor.
    const work = await bob(["token", "new", "picker"]);
    const delegated = await as("bob", work.token);
    await assert.rejects(
      delegated(["pick", "double_add", "auto", "--branch", "alt"]),
      (e: any) => e.status === 403 && e.reason === "token_scope_required",
    );
    // A certificate of another goal is refused before sending.
    await assert.rejects(
      alice(["pick", "double_add", proved.certificate.slice(0, 12)]),
      /unknown_certificate/,
    );

    // Replay: pick and proof command, each with a lost reply, apply once.
    const picks = await events("pick");
    lose = commands(id);
    await assert.rejects(
      alice(["pick", "double_add", d1.certificate.slice(0, 12)]),
      /run the same command again/,
    );
    assert.equal(
      (await alice(["pick", "double_add", d1.certificate.slice(0, 12)])).resent,
      true,
    );
    assert.equal(await events("pick"), picks + 1);
    assert.equal(
      (await http.project(id)).branches!.main.picks[p.root],
      d1.certificate,
    );
    const policies = await events("policy");
    const policy = JSON.stringify({ type: "policy", review: "none" });
    lose = commands(id);
    await assert.rejects(
      alice(["proof", "command", id], policy),
      /run the same command again/,
    );
    const reply = await alice(["proof", "command", id], policy);
    assert.deepEqual(reply.result, { review: "none" });
    assert.equal(await events("policy"), policies + 1);
    // Once a reply has arrived, the same command again is a new command.
    await alice(["proof", "command", id], policy);
    assert.equal(await events("policy"), policies + 2);

    // Readers of the project see goals; unknown goals are not found.
    assert.equal(
      (await carol(["goals"])).goals.length,
      Object.keys(p.goals).length,
    );
    await assert.rejects(
      http.ok(
        "alice",
        "GET",
        `/api/coordination/projects/${id}/goals/${"0".repeat(64)}`,
      ),
      /404/,
    );
  } finally {
    await removeDatabases(db);
    await dep.close();
    await schema.close();
  }
}

test(
  "the owner reviews and opens a route branch with mdc; editors pick; maintainer commands replay (LeanGround double)",
  { skip: !documents, timeout: 180000 },
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
  "maintainer workflow against the real LeanGround",
  { skip: !ready, timeout: 900000 },
  async () => {
    await flow({ url: env.lean!, token: env.leanToken! });
  },
);
