import test from "node:test";
import assert from "node:assert/strict";
import {
  ingest,
  selectRoute,
  command,
  expire,
  activeGoals,
} from "../src/domain.js";
import { board, fact } from "./fixtures.js";
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
