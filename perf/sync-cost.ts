// Sync job cost (docs/mathdoc.md §2.7.5): a board with F certificates, every one of which a sync
// reads again from LeanGround (here the contract double, on loopback) and checks. Measures the
// worker's sync job end to end; no TerminusDB branch is attached, so only certificates count.
// Needs MDC_TEST_DATABASE_URL; uses a fresh schema and removes it.
//
//   npx tsx perf/sync-cost.ts [--facts 358] [--runs 3]
import { createHash, randomUUID } from "node:crypto";
import { performance } from "node:perf_hooks";
import { Store } from "../coordinator/src/store.js";
import { Worker } from "../coordinator/src/worker.js";
import { LeanGround } from "../coordinator/src/remote.js";
import { Documents } from "../coordinator/src/documents.js";
import { addGoal, newBoard, recompute } from "../coordinator/src/domain.js";
import { leangroundDouble } from "../coordinator/test/leanground-double.js";

const args = process.argv.slice(2);
const arg = (name: string, fallback: string) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const F = Number(arg("facts", "358"));
const runs = Number(arg("runs", "3"));
const dsn = process.env.MDC_TEST_DATABASE_URL;
if (!dsn) throw new Error("set MDC_TEST_DATABASE_URL");

const schema = `sync_${randomUUID().replaceAll("-", "")}`;
const admin = new Store(dsn);
await admin.pool.query(`CREATE SCHEMA ${schema}`);
const u = new URL(dsn);
u.searchParams.set("options", `-c search_path=${schema}`);
const store = new Store(u.toString());
await store.migrate();
const lean = await leangroundDouble();
try {
  const key = (i: number) => createHash("sha256").update(`g${i}`).digest("hex");
  const identity = (i: number) => ({
    key: key(i),
    base_fingerprint: "base",
    local_context_hash: "context",
    relevant_options_hash: "options",
  });
  const b = newBoard(
    `mdc-${randomUUID().replaceAll("-", "")}`,
    "alice",
    {
      title: "sync cost",
      proposition: "root",
      base_key: 1,
      context: {},
      options: {},
      members: [],
      budget: 1,
      minimum_trust: "audited",
    },
    { base_key: 1 },
    identity(0),
  );
  // A tree of F certificates: certificate i proves goal i from goals 2i+1 and 2i+2 (leaves below).
  for (let i = 0; i < F; i++) {
    const premises = [2 * i + 1, 2 * i + 2].filter((p) => p < F).map(key);
    const id = createHash("sha256").update(`c${i}`).digest("hex");
    for (const p of [i, 2 * i + 1, 2 * i + 2]) addGoal(b, key(p), identity(p), `g${p}`);
    b.facts[id] = { id, goal: key(i), premises, trust: "trusted", state: "active", available: true };
    lean.certificates.set(id, {
      id,
      goal: key(i),
      premises,
      trust: "trusted",
      request: { goal: identity(i) },
      access: { visibility: "project", project_id: b.id },
    });
  }
  recompute(b);
  b.ready = true;
  await store.create(b);
  await store.pool.query("DELETE FROM mdc_job WHERE project=$1", [b.id]);
  const worker = new Worker(store, new LeanGround(lean.url, "x"), new Documents(), "coordinator");
  const times: number[] = [];
  for (let r = 0; r < runs; r++) {
    await store.apply(b.id, "alice", null, randomUUID(), { type: "sync" });
    const t = performance.now();
    await worker.tick();
    times.push(Math.round(performance.now() - t));
  }
  const jobs = (
    await store.pool.query("SELECT status, error FROM mdc_job WHERE project=$1 AND kind='sync'", [b.id])
  ).rows;
  const after = await store.get(b.id, "alice");
  process.stdout.write(
    `${JSON.stringify({
      facts: F,
      goals: Object.keys(after.goals).length,
      sync_ms: times,
      jobs: jobs.map((j) => j.status),
      unavailable: Object.values(after.facts).filter((f) => !f.available).length,
    })}\n`,
  );
} finally {
  await lean.close();
  await store.pool.end();
  await admin.pool.query(`DROP SCHEMA ${schema} CASCADE`);
  await admin.pool.end();
}
