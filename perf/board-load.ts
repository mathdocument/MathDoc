// Board load test (docs/mathdoc.md §2.7.5). K agent processes work on one project board with N
// goals the way `mdc` does: lease and job commands go with `If-Match: *` (checked against the
// current board, docs/mathdoc.md §2.6 stage 5 decision 2). Each agent repeats claim, heartbeat (twice),
// submit and finish. The server runs as its own process (coordinator/src/main.ts serve), so
// agents' JSON parsing does not slow it down. LeanGround is not called: `submit` only queues a
// job, and no worker runs.
//
// Needs MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL and MDC_TEST_TERMINUS_PASSWORD; uses a fresh
// PostgreSQL schema and TerminusDB database and removes both.
//
//   npx tsx perf/board-load.ts [--agents 20] [--goals 1000] [--rounds 10] [--target-p95 500] [--seed 1]
import { spawn, type ChildProcess } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const arg = (name: string, fallback: string) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const OPS = ["read", "claim", "heartbeat", "submit", "finish"] as const;
type Op = (typeof OPS)[number];

/** Deterministic pseudo-random numbers (mulberry32). */
function random(seed: number) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---------------------------------------------------------------- one agent (child process)
async function agent() {
  const url = arg("url", "");
  const project = arg("project", "");
  const rounds = Number(arg("rounds", "10"));
  const rand = random(Number(arg("seed", "1")));
  const token = process.env.AGENT_TOKEN!;
  const samples: Record<Op, number[]> = {
    read: [],
    claim: [],
    heartbeat: [],
    submit: [],
    finish: [],
  };
  const post: number[] = [];
  let conflicts = 0,
    taken = 0,
    errors = 0;
  const http = async (method: string, path: string, body?: unknown, headers = {}) => {
    const r = await fetch(`${url}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
        ...headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await r.text();
    return { status: r.status, body: text ? JSON.parse(text) : null };
  };
  const read = async () => {
    const t = performance.now();
    const b = await http("GET", `/api/coordination/projects/${project}`);
    samples.read.push(performance.now() - t);
    if (b.status !== 200) throw new Error(`read ${b.status}`);
    return b.body;
  };
  /**
   * One command as mdc sends it. Lease and job commands are checked against the current state
   * (`If-Match: *`), so only a claim reads the board, to choose a task; the latency includes
   * that read and any retry.
   */
  let current: any;
  const command = async (op: Op, body: (board: any) => unknown | null) => {
    const start = performance.now();
    for (let attempt = 0; attempt < 50; attempt++) {
      const board = op === "claim" ? await read() : current;
      if (op === "claim") current = board;
      const c = body(board);
      if (c === null) return null;
      const t = performance.now();
      const r = await http("POST", `/api/coordination/projects/${project}/commands`, c, {
        "if-match": "*",
        "idempotency-key": randomUUID(),
      });
      post.push(performance.now() - t);
      if (r.status === 200) {
        samples[op].push(performance.now() - start);
        return r.body.result;
      }
      if (r.body?.reason === "revision_conflict") conflicts++;
      else if (r.body?.reason === "task_unavailable") taken++;
      else {
        errors++;
        return null;
      }
    }
    errors++;
    return null;
  };
  for (let round = 0; round < rounds; round++) {
    const attempt = await command("claim", (b) => {
      const ready = Object.values(b.tasks as Record<string, any>).filter(
        (t) => t.state === "ready" && b.goals[t.goal].status !== "certified",
      );
      if (!ready.length) return null;
      const t = ready[Math.floor(rand() * ready.length)];
      return { type: "claim", task: t.id, allocation: 1, ttl: 600 };
    });
    if (!attempt) continue;
    for (let i = 0; i < 2; i++)
      await command("heartbeat", () => ({
        type: "heartbeat",
        attempt: attempt.id,
        epoch: attempt.epoch,
        ttl: 600,
      }));
    await command("submit", (b) => ({
      type: "submit",
      goal: b.tasks[attempt.task].goal,
      source: `theorem attempt_${round} : True := trivial\n`,
      expected_root: `attempt_${round}`,
      component: "lean-worker",
      mode: "leaf",
    }));
    await command("finish", () => ({
      type: "finish",
      attempt: attempt.id,
      epoch: attempt.epoch,
      spent: 1,
      outcome: "submitted",
    }));
  }
  process.stdout.write(`${JSON.stringify({ samples, post, conflicts, taken, errors })}\n`);
}

// ---------------------------------------------------------------- the run (parent process)
const quantile = (xs: number[], q: number) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return Math.round(s[Math.min(s.length - 1, Math.floor(q * s.length))] * 10) / 10;
};
const freePort = () =>
  new Promise<number>((done) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => done(port));
    });
  });

async function run() {
  const k = Number(arg("agents", "20"));
  const n = Number(arg("goals", "1000"));
  const rounds = Number(arg("rounds", "10"));
  const target = Number(arg("target-p95", "500"));
  const seed = Number(arg("seed", "1"));
  const dsn = process.env.MDC_TEST_DATABASE_URL;
  const terminus = process.env.MDC_TEST_TERMINUS_URL;
  const terminusPassword = process.env.MDC_TEST_TERMINUS_PASSWORD;
  if (!dsn || !terminus || !terminusPassword)
    throw new Error("set MDC_TEST_DATABASE_URL, MDC_TEST_TERMINUS_URL, MDC_TEST_TERMINUS_PASSWORD");
  const { Store } = await import("../coordinator/src/store.js");
  const { newProjectBoard, addGoal, stateGoal, recompute } = await import(
    "../coordinator/src/domain.js"
  );

  const schema = `load_${randomUUID().replaceAll("-", "")}`;
  const admin = new Store(dsn);
  await admin.pool.query(`CREATE SCHEMA ${schema}`);
  const u = new URL(dsn);
  u.searchParams.set("options", `-c search_path=${schema}`);
  const store = new Store(u.toString());
  await store.migrate();
  const database = `load${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const actors: Record<string, { token: string; admin: boolean }> = {
    lead: { token: `lead-${randomBytes(16).toString("hex")}`, admin: true },
  };
  const agents = Array.from({ length: k }, (_, i) => `agent${String(i + 1).padStart(2, "0")}`);
  for (const a of agents) actors[a] = { token: `${a}-${randomBytes(16).toString("hex")}`, admin: false };

  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  let server: ChildProcess | undefined;
  try {
    server = spawn(process.execPath, ["--import", "tsx", "coordinator/src/main.ts", "serve"], {
      cwd: root,
      env: {
        ...process.env,
        MDC_DATABASE_URL: u.toString(),
        MDC_ACTORS: JSON.stringify(actors),
        MDC_HOST: "127.0.0.1",
        MDC_PORT: String(port),
        MDC_TERMINUS_URL: terminus,
        MDC_TERMINUS_PASSWORD: terminusPassword,
        LEANGROUND_SERVER_URL: "http://127.0.0.1:9",
        LEANGROUND_FACT_TOKEN: "unused",
        LEANGROUND_ACTOR: "coordinator",
        MDC_WEB_DIR: "/nonexistent",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let log = "";
    server.stdout!.on("data", (c) => (log += c));
    server.stderr!.on("data", (c) => (log += c));
    for (const deadline = Date.now() + 30000; !log.includes("MathDoc http://"); ) {
      if (Date.now() > deadline || server.exitCode !== null) throw new Error(`server did not start:\n${log}`);
      await new Promise((r) => setTimeout(r, 100));
    }
    const lead = async (body: unknown) => {
      const r = await fetch(`${base}/api/projects`, {
        method: "POST",
        headers: { authorization: `Bearer ${actors.lead.token}`, "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!r.ok) throw new Error(`${JSON.stringify(body)} → ${r.status} ${await r.text()}`);
    };
    await lead({ action: "init", name: database });
    for (const a of agents)
      await lead({ action: "grant", project: `${database}/main`, actor: a, role: "viewer" });

    // A board of N goals: goal 0 is the root; about 40% of the goals have one active
    // decomposition into later goals, so every goal is reachable and the rest are open leaves.
    const rand = random(seed);
    const key = (i: number) => createHash("sha256").update(`goal ${i}`).digest("hex");
    const identity = (i: number) => ({
      key: key(i),
      base_fingerprint: "base",
      local_context_hash: "context",
      relevant_options_hash: "options",
    });
    const b = newProjectBoard(
      `mdc-${randomUUID().replaceAll("-", "")}`,
      "lead",
      {
        document: { database, branch: "main", node: randomUUID() },
        members: agents,
        budget: 1e9,
      },
      "Load test",
      { base: { base_key: 1 }, context: {}, options: {}, minimum_trust: "audited", environment_id: "env" },
    );
    b.root = key(0);
    const inner = Math.max(1, Math.floor(n * 0.4));
    const premises: number[][] = Array.from({ length: inner }, () => []);
    for (let j = 1; j < n; j++) premises[Math.floor(rand() * Math.min(j, inner))].push(j);
    for (let i = 0; i < n; i++) {
      addGoal(b, key(i), identity(i), `lemma_${i}`);
      stateGoal(b, key(i), {
        proposition: `∀ (n : Nat), f${i} n = g${i} n + ${i}`,
        definitions: [],
        declaration: { name: `lemma_${i}`, binders: "(n : Nat)", type: `f${i} n = g${i} n + ${i}` },
      });
    }
    premises.forEach((ps, i) => {
      if (!ps.length) return;
      const id = createHash("sha256").update(`fact ${i}`).digest("hex");
      b.facts[id] = {
        id,
        goal: key(i),
        premises: ps.map(key),
        trust: "trusted",
        state: "active",
        available: true,
        submitter: "lead",
      };
    });
    recompute(b);
    b.branches!.main.request.bound = true;
    b.ready = true;
    await store.create(b);
    const size = async () =>
      (
        await store.pool.query(
          "SELECT pg_column_size(body) AS stored, octet_length(body::text) AS json FROM mdc_project WHERE id=$1",
          [b.id],
        )
      ).rows[0];
    const before = await size();

    // K agents as separate processes, started together.
    const start = performance.now();
    const results = await Promise.all(
      agents.map(
        (a, i) =>
          new Promise<any>((done, fail) => {
            const child = spawn(
              process.execPath,
              ["--import", "tsx", "perf/board-load.ts", "--agent", "--url", base, "--project", b.id,
                "--rounds", String(rounds), "--seed", String(seed * 1000 + i)],
              { cwd: root, env: { ...process.env, AGENT_TOKEN: actors[a].token }, stdio: ["ignore", "pipe", "inherit"] },
            );
            let out = "";
            child.stdout!.on("data", (c) => (out += c));
            child.on("exit", (code) => (code === 0 ? done(JSON.parse(out)) : fail(new Error(`${a} exited ${code}`))));
          }),
      ),
    );
    const wall = (performance.now() - start) / 1000;
    const after = await size();
    const all = (op: Op) => results.flatMap((r) => r.samples[op]);
    const commands = OPS.filter((o) => o !== "read").reduce((s, o) => s + all(o).length, 0);
    const summary = {
      agents: k,
      goals: n,
      rounds,
      seed,
      facts: Object.keys(b.facts).length,
      board_bytes: { before: Number(before.json), after: Number(after.json), stored_after: Number(after.stored) },
      wall_seconds: Math.round(wall * 10) / 10,
      commands,
      commands_per_second: Math.round((commands / wall) * 10) / 10,
      revision_conflicts: results.reduce((s, r) => s + r.conflicts, 0),
      claims_lost: results.reduce((s, r) => s + r.taken, 0),
      errors: results.reduce((s, r) => s + r.errors, 0),
      post_ms: { p50: quantile(results.flatMap((r) => r.post), 0.5), p95: quantile(results.flatMap((r) => r.post), 0.95) },
      // End to end, as an agent sees it: read the board, send, retry on conflict.
      latency_ms: Object.fromEntries(
        OPS.map((o) => [o, { n: all(o).length, p50: quantile(all(o), 0.5), p95: quantile(all(o), 0.95), max: quantile(all(o), 1) }]),
      ),
      target_p95_ms: target,
    };
    const worst = Math.max(...OPS.filter((o) => o !== "read").map((o) => summary.latency_ms[o].p95 ?? 0));
    process.stdout.write(`${JSON.stringify({ ...summary, meets_target: worst <= target }, null, 2)}\n`);
  } finally {
    server?.kill();
    await store.pool.end();
    await admin.pool.query(`DROP SCHEMA ${schema} CASCADE`);
    await admin.pool.end();
    await fetch(`${terminus}/api/db/admin/${database}`, {
      method: "DELETE",
      headers: { authorization: `Basic ${Buffer.from(`admin:${terminusPassword}`).toString("base64")}` },
    }).catch(() => {});
  }
}

if (args.includes("--agent")) await agent();
else await run();
