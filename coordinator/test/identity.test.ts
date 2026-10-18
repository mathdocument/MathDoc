// Project stage 4 (docs/mathdoc.md §2.7.4): delegated tokens give each agent its own identity
// (`person/name`) with limited scopes, and `mdc mcp` serves the proof verbs to chat clients.
// Real PostgreSQL and TerminusDB; LeanGround is the contract double, or the real service when
// the live harness variables are set (the Lean below is valid in Init and Mathlib).
import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
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
import type { Board } from "../src/domain.js";

const documents = Boolean(env.dsn && env.terminus && env.terminusPassword);
const D = "def double (n : Nat) : Nat := n + n\n";
const R = "theorem double_add (n : Nat) : double n + 0 = n + n := sorry\n";
const PREMISES =
  "theorem double_eq (n : Nat) : double n = n + n := sorry\n" +
  "theorem plus_zero (m : Nat) : m + 0 = m := sorry\n";
const coordinator = join(dirname(fileURLToPath(import.meta.url)), "..");

/** A scripted MCP client: `mdc mcp` as a child process, JSON-RPC over its stdio. */
function mcpClient(environment: NodeJS.ProcessEnv, dir: string) {
  const child = spawn(
    process.execPath,
    ["--import", "tsx", "src/cli.ts", "mcp", "--dir", dir],
    { cwd: coordinator, env: { ...process.env, ...environment } },
  );
  let stderr = "";
  child.stderr.on("data", (c) => (stderr += c));
  const waiting = new Map<number, (m: any) => void>();
  createInterface({ input: child.stdout }).on("line", (line) => {
    const m = JSON.parse(line);
    waiting.get(m.id)?.(m);
  });
  let next = 1;
  const rpc = (method: string, params?: unknown): Promise<any> => {
    const id = next++;
    child.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
    );
    return new Promise((resolve, reject) => {
      waiting.set(id, resolve);
      child.once("exit", () => reject(new Error(`mdc mcp exited: ${stderr}`)));
    });
  };
  const tool = async (name: string, args: unknown) => {
    const m = await rpc("tools/call", { name, arguments: args });
    assert.ok(!m.result.isError, `${name}: ${m.result.content[0].text}`);
    return m.result.structuredContent;
  };
  return {
    rpc,
    tool,
    notify: (method: string) =>
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method })}\n`),
    async close() {
      child.stdin.end();
      await new Promise((r) => child.once("exit", r));
    },
  };
}

async function flow(leanground: { url: string; token: string }) {
  const schema = await isolated();
  const db = `ident${randomUUID().replaceAll("-", "").slice(0, 12)}`;
  const dep = await deploy(schema.url, leanground);
  const http = client(() => dep, db);
  const call = async (
    token: string,
    method: string,
    path: string,
    body?: unknown,
    headers = {},
  ) => {
    const r = await fetch(`${dep.base}${path}`, {
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
  const mdc = (token: string, cwd: string, ...argv: string[]) =>
    main(
      argv,
      { MDC_URL: dep.base, MDC_TOKEN: token, MDC_PROJECT: `${db}/main` },
      async () => "",
      { cwd, sleep: () => dep.drain() },
    ) as Promise<any>;
  const edit = async (dir: string, file: string, fn: (t: string) => string) =>
    writeFile(join(dir, file), fn(await readFile(join(dir, file), "utf8")));
  try {
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
    const id = (
      await http.ok(
        "alice",
        "POST",
        "/api/coordination/projects",
        {
          document: { database: db, branch: "main", node: root },
          members: ["bob"],
          budget: 1000,
        },
        { "idempotency-key": "create" },
      )
    ).id as string;
    await dep.drain();

    // Delegated tokens: made with one's own token, shown once, listed without secrets.
    const home = await mkdtemp(join(tmpdir(), "mdc-ident-"));
    const newToken = (who: "alice" | "bob", name: string, ...scope: string[]) =>
      mdc(
        actors[who].token,
        home,
        "token",
        "new",
        name,
        ...(scope.length ? ["--scope", scope.join(",")] : []),
      );
    const agentA = await newToken("bob", "agent-a");
    assert.deepEqual(
      [agentA.actor, agentA.scopes],
      ["bob/agent-a", ["work"]],
      "work is the default scope",
    );
    assert.match(agentA.token, /^mdct_/);
    const agentB = (await newToken("bob", "agent-b")).token as string;
    const mcpToken = (await newToken("bob", "mcp")).token as string;
    const work = (await newToken("alice", "codex-1")).token as string;
    const lead = (await newToken("alice", "lead", "work", "maintain"))
      .token as string;
    await assert.rejects(newToken("bob", "agent-a"), /token_name_taken/);
    const listed = await mdc(actors.bob.token, home, "token", "list");
    assert.deepEqual(
      listed.map((t: { name: string }) => t.name),
      ["agent-a", "agent-b", "mcp"],
    );
    assert.ok(!JSON.stringify(listed).includes("mdct_"), "no secret is listed");
    assert.equal(
      (await call(agentA.token, "POST", "/api/tokens", { name: "more" })).body
        .reason,
      "own_token_required",
      "a delegated token cannot mint another",
    );
    assert.deepEqual((await call(agentA.token, "GET", "/api/me")).body, {
      actor: "bob/agent-a",
      principal: "bob",
      admin: false,
      scopes: ["work"],
    });

    // Scopes: work reads documents and the board but neither writes documents nor runs owner commands.
    assert.equal(
      (await call(agentA.token, "GET", `/p/${db}/main/api/graph/check`)).status,
      200,
    );
    assert.equal(
      (await call(work, "POST", `/p/${db}/main/api/node/new`, { title: "x" }))
        .status,
      403,
      "alice's work token cannot write although alice can",
    );
    const command = async (token: string, c: unknown) => {
      const b = await http.project(id);
      return call(
        token,
        "POST",
        `/api/coordination/projects/${id}/commands`,
        c,
        {
          "if-match": `"${b.revision}"`,
          "idempotency-key": randomUUID(),
        },
      );
    };
    const assemble = {
      type: "assemble",
      component: "lean-worker",
      branch: "main",
    };
    assert.deepEqual(
      [(await command(work, assemble)).body.reason],
      ["token_scope_required"],
    );
    assert.equal(
      (await command(lead, assemble)).body.reason,
      "no_active_route",
      "maintain lets alice's token reach the owner command itself",
    );
    assert.equal(
      (await command(agentA.token, { type: "policy", review: "none" })).body
        .reason,
      "token_scope_required",
    );

    // Two command-line agents, each with its own token, claim and submit.
    const dirA = await mkdtemp(join(tmpdir(), "mdc-agent-a-"));
    const dirB = await mkdtemp(join(tmpdir(), "mdc-agent-b-"));
    const pulled = await mdc(agentA.token, dirA, "pull", "double_add");
    assert.equal(pulled.lease.actor, "bob/agent-a");
    await edit(dirA, pulled.file, (t) =>
      t
        .replace("\n-- Goal.", `\n${PREMISES}\n-- Goal.`)
        .replace(":= by\n  sorry\n", ":= by\n  rw [plus_zero, double_eq]\n"),
    );
    const split = await mdc(
      agentA.token,
      dirA,
      "split",
      pulled.file,
      "-m",
      "two lemmas",
      "--wait",
    );
    assert.equal(split.status, "certified", JSON.stringify(split));
    await command(lead, {
      type: "decomposition",
      certificate: split.certificate,
      state: "active",
    });
    await dep.drain();
    const goalOf = (name: string) =>
      split.premise_goals.find((x: { name: string }) => x.name === name)
        .goal as string;
    const second = await mdc(agentB, dirB, "pull", goalOf("double_eq"));
    assert.equal(second.lease.actor, "bob/agent-b");
    // Changing members keeps a delegated token's lease while its person stays a member.
    await http.ok("alice", "POST", "/api/projects", {
      action: "grant",
      project: `${db}/main`,
      actor: "carol",
      role: "viewer",
    });
    await http.done(id, "alice", {
      type: "members",
      members: ["bob", "carol"],
    });
    let p: Board = await http.project(id);
    assert.equal(p.attempts[second.lease.attempt].outcome, undefined);
    await edit(dirB, second.file, (t) =>
      t.replace(":= by\n  sorry\n", ":= by\n  rfl\n"),
    );
    const proved = await mdc(agentB, dirB, "prove", second.file, "--wait");
    assert.equal(proved.status, "certified", JSON.stringify(proved));
    assert.equal(
      (await mdc(agentB, dirB, "done", second.file)).lease,
      "finished",
    );

    // A scripted MCP client pulls and proves the other premise with its own token.
    const mcpDir = await mkdtemp(join(tmpdir(), "mdc-mcp-"));
    const mcp = mcpClient(
      { MDC_URL: dep.base, MDC_TOKEN: mcpToken, MDC_PROJECT: `${db}/main` },
      mcpDir,
    );
    // The worker runs here; keep draining while the MCP server waits for its job.
    let draining = false;
    const pump = setInterval(async () => {
      if (draining) return;
      draining = true;
      try {
        await dep.drain();
      } catch {
        /* reported by the assertions */
      } finally {
        draining = false;
      }
    }, 100);
    try {
      const init = await mcp.rpc("initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "script", version: "1" },
      });
      assert.equal(init.result.serverInfo.name, "mdc");
      mcp.notify("notifications/initialized");
      const tools = (await mcp.rpc("tools/list")).result.tools.map(
        (t: { name: string }) => t.name,
      );
      assert.deepEqual(tools, [
        "pull",
        "prove",
        "split",
        "check",
        "lemma",
        "done",
        "note",
      ]);
      const got = await mcp.tool("pull", { goal: goalOf("plus_zero") });
      assert.equal(got.lease.actor, "bob/mcp");
      assert.match(got.text, /-- mdc: edit below this line\n/);
      const text = (got.text as string).replace(
        ":= by\n  sorry\n",
        ":= by\n  rfl\n",
      );
      const sent = await mcp.tool("prove", {
        file: got.file,
        text,
        wait: true,
      });
      assert.equal(sent.status, "certified", JSON.stringify(sent));
      await mcp.tool("done", {
        file: got.file,
        note: "rfl by the definition of +",
      });
      const refused = await mcp.rpc("tools/call", {
        name: "prove",
        arguments: { file: "../escape.lean", text: "x" },
      });
      assert.equal(
        refused.result.isError,
        true,
        "only pulled files are written",
      );
    } finally {
      clearInterval(pump);
      await mcp.close();
    }

    // Audit: leases, submissions and events tell the three agents apart.
    p = await http.project(id);
    const by = (actor: string) =>
      Object.values(p.attempts).filter((a) => a.actor === actor);
    for (const actor of ["bob/agent-a", "bob/agent-b", "bob/mcp"])
      assert.equal(by(actor).length, 1, actor);
    assert.deepEqual(
      Object.values(p.facts)
        .map((f) => f.submitter)
        .filter((x) => x?.startsWith("bob/"))
        .sort(),
      ["bob/agent-a", "bob/agent-b", "bob/mcp"],
    );
    const events = await http.ok(
      "alice",
      "GET",
      `/api/coordination/projects/${id}/history`,
    );
    const who = new Set(events.map((e: { actor: string }) => e.actor));
    for (const actor of ["bob/agent-a", "bob/agent-b", "bob/mcp"])
      assert.ok(who.has(actor), actor);
    assert.equal(p.goals[p.root].status, "derivable");

    // Revocation and expiry take effect on the next request.
    assert.equal((await call(agentB, "GET", "/api/me")).status, 200);
    await mdc(actors.bob.token, home, "token", "revoke", "agent-b");
    assert.equal((await call(agentB, "GET", "/api/me")).status, 401);
    assert.equal(
      (await call(agentB, "GET", `/p/${db}/main/api/graph/check`)).status,
      401,
    );
    await dep.store.pool.query(
      "UPDATE mdc_token SET expires_at=now()-interval '1 second' WHERE name='agent-a'",
    );
    assert.equal((await call(agentA.token, "GET", "/api/me")).status, 401);
    // A person removed from the project takes their tokens' standing with them.
    await http.done(id, "alice", { type: "members", members: ["carol"] });
    assert.equal(
      (await call(mcpToken, "GET", `/api/coordination/projects/${id}`)).status,
      404,
    );
  } finally {
    await removeDatabases(db);
    await dep.close();
    await schema.close();
  }
}

test(
  "delegated tokens: own identities and scopes for agents, and the verbs over MCP (LeanGround double)",
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
  "delegated tokens against the real LeanGround",
  { skip: !ready, timeout: 900000 },
  async () => {
    await flow({ url: env.lean!, token: env.leanToken! });
  },
);
