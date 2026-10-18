import test from "node:test";
import assert from "node:assert/strict";
import {
  ingest,
  selectRoute,
  command,
  expire,
  activeGoals,
  commandSchema,
  stateGoal,
  addGoal,
  rankGoals,
} from "../src/domain.js";
import { board, fact, identity, projectBoard } from "./fixtures.js";
test("shared premises deduplicate, cycles cannot prove themselves, pause only affects selected route", () => {
  const b = board();
  ingest(b, fact("t", "T", ["A", "B"]));
  ingest(b, fact("a", "A", ["B"]));
  ingest(b, fact("cycle", "B", ["T"]));
  assert.equal(Object.keys(b.goals).length, 3);
  assert.equal(Object.keys(b.tasks).length, 3);
  assert.equal(b.goals.T.status, "insufficient");
  assert.throws(() => selectRoute(b), /no_active_route/);
  ingest(b, fact("b", "B"));
  assert.equal(b.goals.T.status, "derivable");
  assert.equal(selectRoute(b).steps.length, 3);
  command(
    b,
    "alice",
    { type: "decomposition", certificate: "a", state: "paused" },
    0,
  );
  assert.equal(b.goals.T.status, "derivable");
  assert.throws(() => selectRoute(b), /no_active_route/);
  assert(activeGoals(b).has("B"));
  ingest(b, fact("a2", "A"));
  assert(selectRoute(b).steps.some((s) => s.certificate_id === "a2"));
});
test("leases fence old worker, expired allocation is charged, agent outcomes never certify goals", () => {
  const b = board();
  const task = Object.values(b.tasks)[0];
  const a = command(
    b,
    "alice",
    { type: "claim", task: task.id, allocation: 40, ttl: 30 },
    0,
  ).result as { id: string; epoch: number };
  assert.throws(
    () =>
      command(
        b,
        "bob",
        { type: "claim", task: task.id, allocation: 10, ttl: 30 },
        1,
      ),
    /task_unavailable/,
  );
  expire(b, 30000);
  assert.equal(b.spent, 40);
  const second = command(
    b,
    "bob",
    { type: "claim", task: task.id, allocation: 50, ttl: 30 },
    31000,
  ).result as { id: string; epoch: number };
  assert.throws(
    () =>
      command(
        b,
        "alice",
        {
          type: "finish",
          attempt: a.id,
          epoch: a.epoch,
          spent: 10,
          outcome: "submitted",
        },
        32000,
      ),
    /stale_lease/,
  );
  command(
    b,
    "bob",
    {
      type: "finish",
      attempt: second.id,
      epoch: second.epoch,
      spent: 15,
      outcome: "submitted",
    },
    32000,
  );
  assert.equal(b.spent, 55);
  assert.equal(b.goals.T.status, "insufficient");
  assert.throws(
    () =>
      command(
        b,
        "alice",
        { type: "claim", task: task.id, allocation: 46, ttl: 30 },
        33000,
      ),
    /budget_exhausted/,
  );
});
test("reject private, foreign environment, low trust and non-member state updates", () => {
  const b = board();
  const f = fact("a", "T");
  f.access.visibility = "private";
  assert.throws(() => ingest(b, f), /fact_not_shared/);
  f.access.visibility = "project";
  f.request.goal.base_fingerprint = "other";
  assert.throws(() => ingest(b, f), /environment_mismatch/);
  f.request.goal.base_fingerprint = "base";
  f.trust = "claimed";
  assert.throws(() => ingest(b, f), /trust_rejected/);
  assert.throws(
    () => command(b, "mallory", { type: "notes", notes: "bad" }, 0),
    /not_found/,
  );
  assert.throws(
    () => command(b, "bob", { type: "assemble", component: "lean-worker" }, 0),
    /owner_required/,
  );
});
test("member removal fences leases immediately; owner remains and budget is conservative", () => {
  const b = board();
  const t = Object.values(b.tasks)[0];
  const a = command(
    b,
    "bob",
    { type: "claim", task: t.id, allocation: 35, ttl: 300 },
    0,
  ).result as { id: string };
  command(b, "alice", { type: "members", members: [] }, 1000);
  assert.deepEqual(b.members, ["alice"]);
  assert.equal(b.ready, false);
  assert.equal(b.attempts[a.id].outcome, "revoked");
  assert.equal(b.spent, 35);
  assert.throws(
    () => command(b, "bob", { type: "notes", notes: "after removal" }, 2000),
    /not_found/,
  );
  assert.equal(
    command(b, "alice", { type: "sync" }, 2000).job?.kind,
    "provision",
  );
});
test("retry pins the original plan and creates a new attempt identity", () => {
  const b = board();
  b.runs.first = {
    id: "first",
    route: { root: "t", steps: [] },
    component: "lean-worker",
    plan_id: "fixed",
    status: "failed",
    retryable: true,
  };
  const out = command(b, "alice", { type: "retry", run: "first" }, 0)
    .result as { id: string; plan_id: string };
  assert.notEqual(out.id, "first");
  assert.equal(out.plan_id, "fixed");
  assert.throws(
    () => command(b, "alice", { type: "retry", run: "first" }, 0),
    /assembly_in_progress/,
  );
});
test("project boards: a branch's picks choose its route; picks are checked and owner-only", () => {
  const b = projectBoard();
  ingest(b, fact("t1", "T", ["A", "B"]), { source: "document" });
  ingest(b, fact("t2", "T", ["C"]), { source: "document" });
  for (const g of ["A", "B", "C"]) ingest(b, fact(g.toLowerCase(), g));
  // Unpicked goals prefer fewer premises; a pick is strict.
  assert.equal(selectRoute(b).root, "t2");
  assert.equal(selectRoute(b, { T: "t1" }).root, "t1");
  b.facts.t1.state = "paused";
  assert.throws(() => selectRoute(b, { T: "t1" }), /no_active_route/);
  b.facts.t1.state = "active";
  const pick = (who: string, branch: string, certificate: string) =>
    command(b, who, { type: "pick", branch, goal: "T", certificate }, 0);
  assert.throws(() => pick("bob", "main", "t1"), /owner_required/);
  assert.throws(() => pick("alice", "main", "a"), /certificate_goal_mismatch/);
  assert.throws(() => pick("alice", "alt", "t1"), /branch_not_attached/);
  pick("alice", "main", "t1");
  const run = command(
    b,
    "alice",
    { type: "assemble", component: "lean-worker", branch: "main" },
    0,
  ).result as { branch: string; route: { root: string } };
  assert.deepEqual([run.branch, run.route.root], ["main", "t1"]);
  // Assembly on a project board names its branch.
  b.runs = {};
  assert.throws(
    () =>
      command(b, "alice", { type: "assemble", component: "lean-worker" }, 0),
    /branch_not_attached/,
  );
});
test("project boards: attach adds the branch; stage 4 requests and superseded boards are read-only", () => {
  const b = projectBoard();
  const out = command(b, "bob", { type: "attach", branch: "alt" }, 0);
  assert.deepEqual(out.job, { kind: "attach", payload: { branch: "alt" } });
  assert.equal(b.branches!.alt.request.node, b.project!.root_node);
  assert.equal(b.branches!.alt.request.bound, false);
  assert.throws(
    () =>
      command(
        b,
        "bob",
        { type: "submit_node", branch: "alt", node: b.project!.root_node },
        0,
      ),
    /branch_not_bound/,
  );
  assert.throws(
    () => commandSchema.parse({ type: "attach", branch: "__proto__" }),
    /reserved branch name/,
  );
  b.project!.superseded = true;
  assert.throws(
    () => command(b, "alice", { type: "sync" }, 0),
    /project_superseded/,
  );
  const legacy = board();
  legacy.request = {
    database: "d",
    branch: "main",
    node: "n",
    environment_id: "e",
    bound: true,
    stale: false,
  };
  assert.throws(
    () => command(legacy, "alice", { type: "notes", notes: "x" }, 0),
    /proof_request_read_only/,
  );
  // A free-standing board (no document binding) is unaffected.
  command(board(), "alice", { type: "notes", notes: "x" }, 0);
});
test("a decomposition's description and a goal's first statement survive re-ingestion", () => {
  const b = board();
  ingest(b, fact("t", "T", ["A"]));
  b.facts.t.description = "split T at A";
  b.facts.t.state = "paused";
  // Every sync re-ingests every known certificate.
  ingest(b, fact("t", "T", ["A"]));
  assert.equal(b.facts.t.description, "split T at A");
  assert.equal(b.facts.t.state, "paused");
  const declaration = { name: "a_lemma", binders: "(n : Nat)", type: "n = n" };
  stateGoal(b, "A", {
    proposition: "∀ (n : Nat), n = n",
    definitions: ["d2"],
    declaration,
  });
  stateGoal(b, "A", {
    proposition: "∀ (m : Nat), m = m",
    definitions: ["d1"],
    declaration: { ...declaration, name: "other" },
  });
  assert.equal(b.goals.A.proposition, "∀ (n : Nat), n = n");
  assert.deepEqual(b.goals.A.definitions, ["d1", "d2"]);
  assert.equal(b.goals.A.declaration!.name, "a_lemma");
  assert.equal(
    b.goals.A.statement,
    "a_lemma",
    "a goal known only by key gets a label",
  );
  stateGoal(b, "missing", { proposition: "True", definitions: [] });
  assert.ok(!Object.hasOwn(b.goals, "missing"));
});
test("stage 3: new decompositions wait for review on project boards; complete proofs and document sketches do not", () => {
  const b = projectBoard();
  ingest(b, fact("leaf", "A"), { source: "agent", actor: "bob", at: 5 });
  ingest(b, fact("agent", "T", ["A", "B"]), {
    source: "agent",
    actor: "bob",
    at: 6,
  });
  ingest(b, fact("node", "T", ["C"]), { source: "document", actor: "alice" });
  ingest(b, fact("found", "T", ["D"]), { source: "sync" });
  ingest(b, fact("loop", "B", ["B"]), { source: "document" });
  assert.equal(
    b.facts.leaf.state,
    "active",
    "a complete proof needs no review",
  );
  assert.equal(b.facts.agent.state, "proposed");
  assert.equal(b.facts.node.state, "active", "the branch's editor wrote it");
  assert.equal(b.facts.found.state, "proposed");
  assert.equal(b.facts.loop.state, "retired", "T ⇐ {T} can never help");
  assert.equal(b.facts.loop.retired_reason, "premise_is_goal");
  assert.deepEqual(
    [b.facts.agent.submitter, b.facts.agent.submitted_at],
    ["bob", 6],
  );
  assert.equal(b.facts.found.submitter, undefined);
  // A proposed decomposition schedules nothing: its new premises cannot be claimed.
  const taskOf = (goal: string) =>
    Object.values(b.tasks).find((t) => t.goal === goal)!.id;
  const claim = (goal: string) =>
    command(
      b,
      "bob",
      { type: "claim", task: taskOf(goal), allocation: 0, ttl: 60 },
      10,
    );
  assert.throws(() => claim("B"), /inactive_goal/);
  claim("C");
  // Only the owner reviews; the review is kept with its comment.
  const review = (who: string, state: "active" | "retired", comment?: string) =>
    command(
      b,
      who,
      {
        type: "decomposition",
        certificate: "agent",
        state,
        ...(comment ? { comment } : {}),
      },
      20,
    );
  assert.throws(() => review("bob", "active"), /owner_required/);
  review("alice", "active", "B is the right lemma");
  assert.deepEqual(b.facts.agent.review, {
    actor: "alice",
    at: 20,
    state: "active",
    comment: "B is the right lemma",
  });
  claim("B");
  assert.throws(
    () =>
      commandSchema.parse({
        type: "decomposition",
        certificate: "agent",
        state: "proposed",
      }),
    /invalid/i,
    "nothing is set back to proposed",
  );
  // Re-ingestion keeps the board's own fields.
  ingest(b, fact("agent", "T", ["A", "B"]), { source: "sync" });
  assert.equal(b.facts.agent.state, "active");
  assert.equal(b.facts.agent.submitter, "bob");
  assert.equal(b.facts.agent.review!.comment, "B is the right lemma");
  // An owner may waive review for new sketches; free-standing boards never review.
  assert.throws(
    () => command(b, "bob", { type: "policy", review: "none" }, 30),
    /owner_required/,
  );
  command(b, "alice", { type: "policy", review: "none" }, 30);
  ingest(b, fact("later", "T", ["E"]), { source: "agent" });
  assert.equal(b.facts.later.state, "active");
  const legacy = board();
  ingest(legacy, fact("t", "T", ["A"]));
  assert.equal(legacy.facts.t.state, "active");
  assert.throws(
    () => command(legacy, "alice", { type: "policy", review: "none" }, 0),
    /not_a_project_board/,
  );
});
test("stage 3: notes stay on the goal, with finish, at most 50 of at most 500 characters", () => {
  const b = projectBoard();
  const note = (text: string, at: number) =>
    command(
      b,
      "bob",
      commandSchema.parse({ type: "note", goal: "T", text }),
      at,
    );
  note("  tried induction on n; stuck at the succ case  ", 1);
  assert.deepEqual(b.goals.T.notes, [
    {
      actor: "bob",
      at: 1,
      text: "tried induction on n; stuck at the succ case",
    },
  ]);
  assert.throws(
    () => command(b, "bob", { type: "note", goal: "X", text: "x" }, 2),
    /unknown_goal/,
  );
  assert.throws(
    () =>
      commandSchema.parse({ type: "note", goal: "T", text: "x".repeat(501) }),
    /500/,
  );
  assert.throws(
    () => commandSchema.parse({ type: "note", goal: "T", text: "   " }),
    /1/,
  );
  for (let i = 0; i < 60; i++) note(`n${i}`, 10 + i);
  assert.equal(b.goals.T.notes!.length, 50);
  assert.equal(b.goals.T.notes![0].text, "n10", "the oldest are dropped");
  const task = Object.values(b.tasks).find((t) => t.goal === "T")!;
  const a = command(
    b,
    "bob",
    { type: "claim", task: task.id, allocation: 0, ttl: 60 },
    100,
  ).result as { id: string; epoch: number };
  command(
    b,
    "bob",
    {
      type: "finish",
      attempt: a.id,
      epoch: a.epoch,
      spent: 0,
      outcome: "no_progress",
      note: "omega fails on the nonlinear term",
    },
    101,
  );
  assert.deepEqual(b.goals.T.notes!.at(-1), {
    actor: "bob",
    at: 101,
    text: "omega fails on the nonlinear term",
    attempt: a.id,
  });
});
test("notes change only by their author, the author's person, or the owner removing one", () => {
  const b = projectBoard();
  b.members.push("carol");
  const run = (actor: string, input: unknown, at = 10, grants = {}) =>
    command(b, actor, commandSchema.parse(input), at, grants);
  run("bob/agent", { type: "note", goal: "T", text: "tried induction" }, 1);
  run("carol", { type: "note", goal: "T", text: "see A first" }, 2);
  const agent = { actor: "bob/agent", at: 1 };
  const edit = (actor: string, note: object, text: string) =>
    run(actor, { type: "edit_note", goal: "T", note, text });
  // The token itself and its person edit; another member, another token or the owner do not.
  edit("bob/agent", agent, "tried induction on n");
  assert.deepEqual(edit("bob", agent, "  tried strong induction  ").result, {
    actor: "bob/agent",
    at: 1,
    text: "tried strong induction",
    edited: 10,
  });
  for (const actor of ["carol", "bob/other", "alice"])
    assert.throws(() => edit(actor, agent, "x"), /note_author_required/);
  assert.throws(
    () => edit("bob", { actor: "bob/agent", at: 3 }, "x"),
    /unknown_note/,
  );
  // The owner removes another member's note only with `maintain`.
  const remove = (actor: string, note: object, grants = {}) =>
    run(actor, { type: "delete_note", goal: "T", note }, 11, grants);
  const carols = { actor: "carol", at: 2 };
  assert.throws(() => remove("bob", carols), /note_author_required/);
  assert.throws(() => remove("alice/bot", carols), /note_author_required/);
  assert.throws(
    () => remove("alice/bot", carols, { maintains: false }),
    /note_author_required/,
  );
  remove("alice", carols, { maintains: true });
  remove("bob", agent);
  assert.deepEqual(b.goals.T.notes, []);
});
test("stage 4: a delegated token acts for its person; leases and audit keep the full identity", () => {
  const b = projectBoard();
  ingest(b, fact("t", "T", ["A", "B"]), { source: "document" });
  const taskOf = (goal: string) =>
    Object.values(b.tasks).find((t) => t.goal === goal)!.id;
  const claim = (who: string, goal: string) =>
    command(
      b,
      who,
      { type: "claim", task: taskOf(goal), allocation: 0, ttl: 60 },
      0,
    ).result as { id: string; actor: string; epoch: number };
  // bob is a member, so bob/agent-a is; alice owns the board, so alice/lead may act as owner.
  const a = claim("bob/agent-a", "A");
  const c = claim("bob/agent-b", "B");
  assert.deepEqual([a.actor, c.actor], ["bob/agent-a", "bob/agent-b"]);
  assert.throws(() => claim("mallory/agent", "T"), /not_found/);
  assert.throws(
    () => command(b, "bob/agent-a", { type: "policy", review: "none" }, 0),
    /owner_required/,
  );
  command(b, "alice/lead", { type: "policy", review: "none" }, 0);
  // A lease belongs to the token that took it, not to its person's other tokens.
  assert.throws(
    () =>
      command(
        b,
        "bob/agent-b",
        { type: "heartbeat", attempt: a.id, epoch: a.epoch, ttl: 60 },
        1,
      ),
    /stale_lease/,
  );
  command(
    b,
    "bob/agent-a",
    { type: "heartbeat", attempt: a.id, epoch: a.epoch, ttl: 60 },
    1,
  );
  // A lease its session left behind: the token's person, or the owner with `maintain`, releases
  // it, spending nothing; nobody else, and no other outcome.
  const release = (
    who: string,
    attempt: { id: string; epoch: number },
    extra = {},
    grants = {},
  ) =>
    command(
      b,
      who,
      commandSchema.parse({
        type: "finish",
        attempt: attempt.id,
        epoch: attempt.epoch,
        spent: 0,
        outcome: "released",
        ...extra,
      }),
      1,
      grants,
    );
  // c is bob/agent-b's: bob's other token and other members cannot release it.
  assert.throws(() => release("bob/agent-a", c), /stale_lease/);
  assert.throws(() => release("carol", c), /not_found|stale_lease/);
  assert.throws(
    () => release("bob", c, { outcome: "submitted" }),
    /stale_lease/,
  );
  assert.throws(() => release("alice", c), /stale_lease/);
  assert.throws(
    () => release("alice/lead", c, {}, { maintains: false }),
    /stale_lease/,
  );
  release("alice", c, {}, { maintains: true });
  assert.equal(b.attempts[c.id].outcome, "released");
  assert.equal(b.tasks[taskOf("B")].state, "ready");
  const again = claim("bob/agent-b", "B");
  release("bob", again);
  assert.equal(b.attempts[again.id].outcome, "released");
  const c2 = claim("bob/agent-b", "B");
  // Changing members keeps the leases of tokens whose person stays a member.
  command(b, "alice", { type: "members", members: ["bob", "carol"] }, 2);
  assert.equal(b.attempts[a.id].outcome, undefined);
  b.ready = true; // the provision job that re-registers members has run
  command(b, "alice", { type: "members", members: ["carol"] }, 3);
  assert.equal(b.attempts[a.id].outcome, "revoked");
  assert.equal(b.attempts[c2.id].outcome, "revoked");
});
test("stage 5: a proved negation refutes its goal; decompositions using it leave routes and their tasks end", () => {
  const b = projectBoard();
  const doc = { source: "document" as const };
  // T ⇐ {A, B} by d1; T ⇐ {C} by d2; B ⇐ {X} by d3. X is reachable only through d1 and d3.
  ingest(b, fact("d1", "T", ["A", "B"]), doc);
  ingest(b, fact("d2", "T", ["C"]), doc);
  ingest(b, fact("d3", "B", ["X"]), doc);
  ingest(b, fact("c", "C"), doc);
  b.facts.d1.submitter = "bob/agent-a";
  const taskOf = (goal: string) =>
    Object.values(b.tasks).find((t) => t.goal === goal)!;
  const lease = command(
    b,
    "bob",
    { type: "claim", task: taskOf("X").id, allocation: 2, ttl: 60 },
    0,
  ).result as { id: string };
  // ¬A is registered as goal N and can be claimed; a sketch of it refutes nothing.
  b.goals.A.proposition = "∀ (n : Nat), n = n + 1";
  b.goals.A.identity = identity("A");
  addGoal(b, "N", identity("N"), "not_a");
  b.negations = { N: "A" };
  command(
    b,
    "bob",
    { type: "claim", task: taskOf("N").id, allocation: 0, ttl: 60 },
    0,
  );
  ingest(b, fact("n-sketch", "N", ["M"]), doc);
  assert.equal(b.goals.A.refuted, undefined);
  // A complete proof of ¬A refutes A.
  ingest(b, fact("n-proof", "N"), { source: "agent", actor: "carol", at: 9 });
  assert.deepEqual(b.goals.A.refuted, { certificate: "n-proof", at: 9 });
  assert.equal(b.facts.d1.state, "refuted");
  assert.equal(b.facts.d1.refuted_by, "A");
  assert.equal(b.facts.d2.state, "active");
  assert.deepEqual(
    selectRoute(b, {}).root,
    "d2",
    "routes avoid the refuted decomposition",
  );
  assert.throws(() => selectRoute(b, { T: "d1" }), /no_active_route/);
  // A and what only d1 reached (B, X) are cancelled; T and C stay; X's lease ends, charged.
  for (const g of ["A", "B", "X"])
    assert.equal(taskOf(g).state, "cancelled", g);
  for (const g of ["T", "C"]) assert.notEqual(taskOf(g).state, "cancelled", g);
  assert.equal(b.attempts[lease.id].outcome, "cancelled");
  assert.equal(b.attempts[lease.id].spent, 2);
  assert.throws(
    () =>
      command(
        b,
        "alice",
        { type: "decomposition", certificate: "d1", state: "active" },
        10,
      ),
    /certificate_refuted/,
  );
  // Whoever submitted the refuted decomposition finds why on its goal.
  assert.match(
    b.goals.T.notes!.at(-1)!.text,
    /d1 by bob\/agent-a is refuted: its premise .* is false/,
  );
  assert.match(b.goals.A.notes!.at(-1)!.text, /Refuted: certificate n-proof/);
  // negate needs a stated goal and answers with the known negation.
  assert.deepEqual(
    command(b, "bob", { type: "negate", goal: "A" }, 11).result,
    {
      goal: "A",
      negation: "N",
    },
  );
  assert.throws(
    () => command(b, "bob", { type: "negate", goal: "C" }, 11),
    /goal_unresolved/,
  );
});
test("stage 5: next recommends the frontier of the routes, closest routes and fewest failures first", () => {
  // Handwritten: main picks T ⇐ {Q, B}; alt picks T ⇐ {B, C, D}; third picks T ⇐ {C}.
  // Unsolved goals: main {T, Q, B} (each 1/3), alt {T, B, C, D} (1/4), third {T, C} (1/2).
  // C failed once. By hand: B = 1/3 + 1/4 ≈ 0.58; C = (1/4 + 1/2) / 2 ≈ 0.38; Q = 1/3;
  // D = 1/4. T has active decompositions, so it is not on the frontier.
  const b = projectBoard();
  const doc = { source: "document" as const };
  ingest(b, fact("d1", "T", ["Q", "B"]), doc);
  ingest(b, fact("d2", "T", ["B", "C", "D"]), doc);
  ingest(b, fact("d3", "T", ["C"]), doc);
  b.branches!.main.picks = { T: "d1" };
  b.branches!.alt = structuredClone(b.branches!.main);
  b.branches!.alt.picks = { T: "d2" };
  b.branches!.third = structuredClone(b.branches!.main);
  b.branches!.third.picks = { T: "d3" };
  const taskOf = (goal: string) =>
    Object.values(b.tasks).find((t) => t.goal === goal)!;
  const a = command(
    b,
    "bob",
    { type: "claim", task: taskOf("C").id, allocation: 0, ttl: 60 },
    0,
  ).result as { id: string; epoch: number };
  command(
    b,
    "bob",
    {
      type: "finish",
      attempt: a.id,
      epoch: a.epoch,
      spent: 0,
      outcome: "no_progress",
    },
    1,
  );
  const ranked = rankGoals(b);
  assert.deepEqual(
    ranked.map((c) => c.goal),
    ["B", "C", "Q", "D"],
  );
  assert.deepEqual(ranked[0].branches.sort(), ["alt", "main"]);
  assert.equal(ranked[0].score, Math.round((1 / 3 + 1 / 4) * 1e6) / 1e6);
  assert.equal(ranked[1].failures, 1);
  // One branch's view; a leased task and a proved goal drop out; a proved lemma shortens routes.
  assert.deepEqual(
    rankGoals(b, "main").map((c) => c.goal),
    ["B", "Q"],
  );
  command(
    b,
    "bob",
    { type: "claim", task: taskOf("Q").id, allocation: 0, ttl: 60 },
    2,
  );
  ingest(b, fact("b", "B"), doc);
  // alt {T, C, D} (1/3 each), third {T, C} (1/2): C = (1/3 + 1/2) / 2 ≈ 0.42, D = 1/3.
  assert.deepEqual(
    rankGoals(b).map((c) => c.goal),
    ["C", "D"],
  );
});
