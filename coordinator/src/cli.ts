#!/usr/bin/env node
// `mdc`: a command-line client of the MathDoc HTTP API (docs/mathdoc.md §2.5; migration stage 5
// replaced the Rust CLI). Document and board commands are one or two HTTP requests with the
// caller's access token and print the JSON response. The proof verbs (pull, prove, split,
// done, note; src/mdc/) work on goal files and keep replay records in `.mdc/`. Node references
// accept an exact title or a UUID.
//
//   MDC_URL      server origin (default http://127.0.0.1:17843; a workspace's own server)
//   MDC_TOKEN    access token (required)
//   MDC_PROJECT  default DATABASE/BRANCH for branch commands (or -p DATABASE/BRANCH)
import { realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Client, MdcError } from "./mdc/client.js";
import { find as findWorkspace } from "./mdc/workspace.js";
import * as verbs from "./mdc/verbs.js";
import * as maintain from "./mdc/maintain.js";
import { serve as serveMcp } from "./mdc/mcp.js";

const USAGE = `usage: mdc [-p DATABASE/BRANCH] <command>

server:     status (you, your default branch, the server) | init DATABASE | remove DATABASE
            grant ACTOR owner|editor|viewer|none
branch:     branch new NAME [--attach] [--pick GOAL=CERT]... | branch del | history | export | import FILE
            project latex show | project latex set --preamble FILE --bib FILE
graph:      graph check|roots|full | search QUERY [-n N]
nodes:      show REF | new -t TITLE [--parent REF] | del REF | rename REF TITLE
            edit REF [--type text|lean|rocq|latex] [--delete]   (source from stdin)
            dep add REF -t TARGET | dep rm REF -t TARGET... | dep show|refs REF [-d DEPTH]
            dep leaf REF | dep candidates REF [QUERY] [-n N] | metric ior REF
proof:      proof env [set]             (the database's environment; set reads JSON from stdin;
                                         "mode": "modules" makes every Lean node a whole file)
            proof submit REF [--budget N] [--members a,b]   (create the project or attach the branch)
            proof list [REF] | proof status ID | proof command ID   (command JSON from stdin)
goals:      goals [--status S] [--branch B] | goal GOAL   (the project's goals; one goal in full)
            review [--state S] | review CERT --accept|--pause|--retire [-m COMMENT]
            pick GOAL CERT|auto [--branch B]   (the branch's route at GOAL; owners and its editors)
work:       pull GOAL [--branch B] [-o FILE] [--ttl S] [--allocation N] [--with K1,K2] [--no-claim]
            prove FILE [--wait] [--dry-run] [--again]
            split FILE -m DESCRIPTION [--wait] [--dry-run] [--again]
            check FILE                 (LeanGround checks it now: Lean's messages; no submission)
                                       module projects: pull writes a directory (see its GOAL.md);
                                       prove|split|check DIR [--root NAME], split DIR --shared FILE...
            done [FILE] [--spent N] [--outcome O] [--note TEXT]
            note FILE|GOAL -m TEXT     (what was tried, where it got stuck)
                                       DESCRIPTION, notes: Markdown, math in $…$ or $$…$$
            refute GOAL [pull flags]   (pull the goal's negation; proving it marks GOAL false)
            next [--branch B] [-n N] [--claim]   (the goals most worth working on; --claim pulls the first)
            lemma QUERY... [--kind theorem|definition|all] [-n N]   (library declarations by name, in the project's base)
            mcp [--dir DIR]            (the work commands as MCP tools over stdio)
external:   external list | external new DATABASE --source SOURCE --config NAME [--commit SHA]
            external show [DATABASE] | external check [DATABASE] [--wait] [--timeout S]
            external rebuild [DATABASE]
                                       (proofs imported from an upstream repository, checked by
                                        LeanGround with Comparator; DATABASE defaults to -p's;
                                        rebuild rewrites the nodes at the import's commit, the
                                        checks stay)
tokens:     token new NAME [--scope work,maintain,edit] [--expires 30d|12h|ISO-DATE]
            token list | token revoke NAME   (delegated tokens act as YOU/NAME)
            (GOAL: a GoalKey, a unique prefix of 8+ hex digits, a node title or UUID, or the
             name \`mdc goals\` shows; CERT: a certificate ID or a unique prefix of 8+ hex digits)
Board commands sent by review, pick, branch new and proof command can be rerun after a lost
reply: they take effect once (records in MDC_STATE_DIR, default ~/.local/state/mdc).
Write commands take --revision REV to require the revision printed by show.`;

class UsageError extends Error {}

function parse(argv: string[]) {
  const positional: string[] = [];
  const flags: Record<string, string[]> = {};
  const alias: Record<string, string> = {
    "-p": "--project",
    "-t": "--target",
    "-n": "--max",
    "-d": "--depth",
    "-m": "--message",
    "-o": "--output",
  };
  const boolean = new Set([
    "--delete",
    "--help",
    "-h",
    "--wait",
    "--dry-run",
    "--again",
    "--no-claim",
    "--claim",
    "--accept",
    "--pause",
    "--retire",
    "--attach",
  ]);
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg.startsWith("-") && !/^-\d/.test(arg)) {
      const name = alias[arg] ?? arg;
      if (boolean.has(name)) {
        flags[name] = ["true"];
        continue;
      }
      const values: string[] = [];
      // --target takes several values (dep rm); others take exactly one.
      do values.push(argv[++i] ?? "");
      while (
        name === "--target" &&
        i + 1 < argv.length &&
        !argv[i + 1].startsWith("-")
      );
      if (values.some((v) => v === ""))
        throw new UsageError(`${arg} needs a value`);
      flags[name] = [...(flags[name] ?? []), ...values];
    } else positional.push(arg);
  }
  return { positional, flags };
}

async function stdin(): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}

/** Test seams: the process's network, directory, clock and streams. */
export interface Io {
  fetch?: typeof fetch;
  cwd?: string;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  stdin?: NodeJS.ReadableStream;
  stdout?: NodeJS.WritableStream;
}

const VERSION = "0.7.0";

/** `30d`, `12h` or an ISO date, as the ISO time a token expires. */
function expiry(value: string, now: () => number) {
  const m = /^(\d+)([hd])$/.exec(value);
  if (m)
    return new Date(
      now() + Number(m[1]) * (m[2] === "h" ? 3600000 : 86400000),
    ).toISOString();
  const t = Date.parse(value);
  if (Number.isNaN(t))
    throw new UsageError("--expires takes 30d, 12h or an ISO date");
  return new Date(t).toISOString();
}

const WORK = new Set([
  "pull",
  "prove",
  "split",
  "check",
  "done",
  "note",
  "refute",
  "next",
  "lemma",
  "mcp",
]);

export async function main(
  argv: string[],
  env: NodeJS.ProcessEnv = process.env,
  input: () => Promise<string> = stdin,
  io: Io = {},
): Promise<unknown> {
  const { positional, flags } = parse(argv);
  const [command, ...args] = positional;
  if (!command || flags["--help"] || flags["-h"]) throw new UsageError(USAGE);
  const cwd = io.cwd ?? process.cwd();
  const flag = (name: string) => flags[name]?.[0];
  const dryRun = WORK.has(command) && flags["--dry-run"] !== undefined;
  // A workspace remembers its server, so verbs run inside one need no MDC_URL.
  const workspace = WORK.has(command)
    ? await findWorkspace(
        command === "mcp"
          ? resolve(cwd, flag("--dir") ?? ".")
          : command === "pull" || command === "lemma" || !args[0]
            ? cwd
            : dirname(resolve(cwd, args[0])),
      )
    : undefined;
  const origin = (
    env.MDC_URL ??
    workspace?.server ??
    "http://127.0.0.1:17843"
  ).replace(/\/$/, "");
  const token = env.MDC_TOKEN;
  if (!token && !dryRun)
    throw new UsageError("set MDC_TOKEN to your access token");
  const project = flag("--project") ?? env.MDC_PROJECT;
  const need = (value: string | undefined, what: string) => {
    if (!value) throw new UsageError(`missing ${what}\n\n${USAGE}`);
    return value;
  };
  const client = new Client(origin, token ?? "", io.fetch);
  const request = client.request.bind(client);
  const branch = () => {
    const p = need(project, "-p DATABASE/BRANCH (or MDC_PROJECT)");
    if (!/^[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+$/.test(p))
      throw new UsageError("use DATABASE/BRANCH, for example etp/main");
    return p;
  };
  const api = (path: string) => `/p/${branch()}/api${path}`;
  const node = async (ref: string) => {
    const { fnode } = await request(
      "GET",
      api(`/resolve?ref=${encodeURIComponent(ref)}`),
    );
    return (await request("GET", api(`/node/${fnode}/view`))).node;
  };
  const guarded = (revision: string) => ({
    "if-match": `"${flag("--revision") ?? revision}"`,
  });
  const directory = (body: unknown) => request("POST", "/api/projects", body);
  const coordination = (path: string) => `/api/coordination${path}`;

  if (command === "mcp") {
    // Serves until stdin ends; stdout carries only protocol messages.
    await serveMcp(
      {
        client,
        cwd: resolve(cwd, flag("--dir") ?? "."),
        project,
        now: io.now ?? Date.now,
        sleep: io.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms))),
      },
      io.stdin ?? process.stdin,
      io.stdout ?? process.stdout,
      VERSION,
    );
    return undefined;
  }
  const ctx: verbs.VerbContext = {
    client,
    cwd,
    project,
    stateDir: maintain.stateDir(env),
    flag,
    all: (name) => flags[name] ?? [],
    has: (name) => flags[name] !== undefined,
    now: io.now ?? Date.now,
    sleep: io.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms))),
  };
  const usage = async <T>(run: () => Promise<T>) => {
    try {
      return await run();
    } catch (e) {
      if (e instanceof MdcError && e.reason === "usage")
        throw new UsageError(`${e.details}\n\n${USAGE}`);
      throw e;
    }
  };
  // Proof work and goal commands: a usage error from them prints the usage text.
  const run: Record<string, () => Promise<unknown>> = {
    pull: () => verbs.pull(ctx, args[0]),
    prove: () => verbs.submit(ctx, args[0], "prove"),
    split: () => verbs.submit(ctx, args[0], "split"),
    check: () => verbs.check(ctx, args[0]),
    lemma: () => verbs.lemma(ctx, args),
    done: () => verbs.done(ctx, args[0]),
    note: () => verbs.note(ctx, args[0]),
    refute: () => verbs.refute(ctx, args[0]),
    next: () => verbs.next(ctx),
    goals: () => maintain.goals(ctx),
    goal: () => maintain.goal(ctx, args[0]),
    review: () => maintain.review(ctx, args[0]),
    pick: () => maintain.pick(ctx, args[0], args[1]),
  };
  if (Object.hasOwn(run, command)) return usage(run[command]);

  switch (command) {
    case "token": {
      // Delegated tokens (docs/mathdoc.md §2.7.4), managed with your own token.
      if (args[0] === "list") return request("GET", "/api/tokens");
      if (args[0] === "revoke")
        return request(
          "DELETE",
          `/api/tokens/${encodeURIComponent(need(args[1], "NAME"))}`,
        );
      if (args[0] === "new") {
        const scopes = flag("--scope")?.split(",").filter(Boolean);
        const expires = flag("--expires");
        return request("POST", "/api/tokens", {
          name: need(args[1], "NAME"),
          ...(scopes ? { scopes } : {}),
          ...(expires
            ? { expires_at: expiry(expires, io.now ?? Date.now) }
            : {}),
        });
      }
      break;
    }
    case "external": {
      // External proof imports (docs/external-proofs.md §3.3).
      const database = () =>
        args[1] ?? need(project, "DATABASE (or -p DATABASE/BRANCH)").split("/")[0];
      if (args[0] === "list") return request("GET", "/api/imports");
      if (args[0] === "new")
        return request("POST", "/api/imports", {
          database: need(args[1], "DATABASE"),
          source: need(flag("--source"), "--source SOURCE"),
          config: need(flag("--config"), "--config NAME"),
          ...(flag("--commit") ? { commit: flag("--commit") } : {}),
        });
      if (args[0] === "show")
        return request("GET", `/api/imports/${encodeURIComponent(database())}`);
      if (args[0] === "rebuild")
        return request("POST", `/api/imports/${encodeURIComponent(database())}/rebuild`);
      if (args[0] === "check") {
        const db = encodeURIComponent(database());
        let check = await request("POST", `/api/imports/${db}/checks`);
        if (!flags["--wait"]) return check;
        const sleep = io.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));
        const now = io.now ?? Date.now;
        const deadline = now() + Number(flag("--timeout") ?? 14400) * 1000;
        while (["submitting", "queued", "running"].includes(check.status)) {
          if (now() > deadline)
            throw new MdcError("timeout", `check ${check.id} is still ${check.status}`);
          await sleep(10000);
          const all = await request("GET", `/api/imports/${db}`);
          check = all.checks.find((c: { id: string }) => c.id === check.id) ?? check;
        }
        return check;
      }
      break;
    }
    case "status": {
      // Who you are and the default branch (-p or MDC_PROJECT), then the server and your branches.
      const [server, me] = await Promise.all([
        request("GET", "/api/status"),
        request("GET", "/api/me"),
      ]);
      return { actor: me.actor, project: project ?? null, ...server };
    }
    case "init":
      return directory({ action: "init", name: need(args[0], "DATABASE") });
    case "remove":
      return directory({
        action: "remove",
        database: need(args[0], "DATABASE"),
      });
    case "grant":
      return directory({
        action: "grant",
        project: branch(),
        actor: need(args[0], "ACTOR"),
        role: need(args[1], "ROLE"),
      });
    case "branch":
      if (args[0] === "new") {
        branch();
        return usage(() =>
          maintain.branchNew(
            ctx,
            need(args[1], "NAME"),
            flags["--pick"] ?? [],
            flags["--attach"] !== undefined,
          ),
        );
      }
      if (args[0] === "del")
        return directory({ action: "delete_branch", project: branch() });
      break;
    case "history":
      return request("GET", api("/history"));
    case "export":
      return request("GET", api("/export"));
    case "import":
      return request(
        "POST",
        api("/import"),
        JSON.parse(await readFile(need(args[0], "FILE"), "utf8")),
      );
    case "project":
      if (args[0] === "latex" && args[1] === "show")
        return request("GET", api("/project/latex"));
      if (args[0] === "latex" && args[1] === "set") {
        const current = await request("GET", api("/project/latex"));
        const preamble = need(flag("--preamble"), "--preamble FILE");
        const bib = need(flag("--bib"), "--bib FILE");
        return request(
          "PUT",
          api("/project/latex"),
          {
            preamble_name: preamble.split("/").pop(),
            preamble: await readFile(preamble, "utf8"),
            bibliography_name: bib.split("/").pop(),
            bibliography: await readFile(bib, "utf8"),
          },
          guarded(current.revision),
        );
      }
      break;
    case "graph":
      if (["check", "roots", "full"].includes(args[0]))
        return request("GET", api(`/graph/${args[0]}`));
      break;
    case "search":
      return request(
        "GET",
        api(
          `/search?q=${encodeURIComponent(args[0] ?? "")}&n=${flag("--max") ?? 200}`,
        ),
      );
    case "show":
      return node(need(args[0], "REF"));
    case "new": {
      const title = need(flag("--target") ?? flag("--title"), "-t TITLE");
      const parent = flag("--parent");
      if (!parent) return request("POST", api("/node/new"), { title });
      const p = await node(parent);
      return request(
        "POST",
        api("/node/new"),
        { title, parent_fnode: p.fnode },
        guarded(p.revision),
      );
    }
    case "del": {
      const n = await node(need(args[0], "REF"));
      return request(
        "DELETE",
        api(`/node/${n.fnode}`),
        undefined,
        guarded(n.revision),
      );
    }
    case "rename": {
      const n = await node(need(args[0], "REF"));
      return request(
        "PUT",
        api(`/node/${n.fnode}/title`),
        { title: need(args[1], "TITLE") },
        guarded(n.revision),
      );
    }
    case "edit": {
      const n = await node(need(args[0], "REF"));
      const type = flag("--type") ?? "lean";
      if (flags["--delete"])
        return request(
          "DELETE",
          api(`/node/${n.fnode}/block/${type}`),
          undefined,
          guarded(n.revision),
        );
      return request(
        "PUT",
        api(`/node/${n.fnode}/block/${type}`),
        { content: await input() },
        guarded(n.revision),
      );
    }
    case "dep": {
      const [sub, ref, query] = args;
      const n = await node(need(ref, "REF"));
      if (sub === "add") {
        const target = await node(need(flag("--target"), "-t TARGET"));
        return request(
          "POST",
          api(`/node/${n.fnode}/dep/add`),
          { dep_fnode: target.fnode },
          guarded(n.revision),
        );
      }
      if (sub === "rm") {
        const refs = flags["--target"] ?? [];
        if (!refs.length) throw new UsageError("missing -t TARGET");
        const targets = await Promise.all(refs.map(node));
        return request(
          "POST",
          api(`/node/${n.fnode}/dep/rm`),
          { dep_fnodes: targets.map((t) => t.fnode) },
          guarded(n.revision),
        );
      }
      if (sub === "show" || sub === "refs" || sub === "leaf")
        return request(
          "GET",
          api(
            `/node/${n.fnode}/dep?mode=${sub}&depth=${sub === "leaf" ? -1 : (flag("--depth") ?? 1)}`,
          ),
        );
      if (sub === "candidates")
        return request(
          "GET",
          api(
            `/node/${n.fnode}/dep/candidates?q=${encodeURIComponent(query ?? "")}&n=${flag("--max") ?? 200}`,
          ),
        );
      break;
    }
    case "metric":
      if (args[0] === "ior") {
        const n = await node(need(args[1], "REF"));
        return request("GET", api(`/node/${n.fnode}/metric/ior`));
      }
      break;
    case "proof": {
      const [sub, ref] = args;
      const [database, branchName] = branch().split("/");
      // The environment belongs to the database; -p only names it.
      if (sub === "env" && ref === "set")
        return request(
          "PUT",
          coordination(`/environments/${database}`),
          JSON.parse(await input()),
        );
      if (sub === "env")
        return request("GET", coordination(`/environments/${database}`));
      if (sub === "submit") {
        const n = await node(need(ref, "REF"));
        return request(
          "POST",
          coordination("/projects"),
          {
            document: { database, branch: branchName, node: n.fnode },
            members: (flag("--members") ?? "").split(",").filter(Boolean),
            budget: Number(flag("--budget") ?? 1000),
          },
          { "idempotency-key": crypto.randomUUID() },
        );
      }
      if (sub === "list") {
        const params = new URLSearchParams({ database, branch: branchName });
        if (ref) params.set("node", (await node(ref)).fnode);
        return request("GET", coordination(`/projects?${params}`));
      }
      if (sub === "status")
        return request("GET", coordination(`/projects/${need(ref, "ID")}`));
      if (sub === "command") {
        // Rerunning after a lost reply replays it (src/mdc/maintain.ts); the reply is the server's.
        const { resent: _, ...reply } = await maintain.sendRecorded(
          ctx,
          need(ref, "ID"),
          JSON.parse(await input()),
        );
        return reply;
      }
      break;
    }
  }
  throw new UsageError(
    `unknown command: ${[command, ...args].join(" ")}\n\n${USAGE}`,
  );
}

// Run when executed (directly or through the `mdc` bin link), not when imported by tests.
const invoked =
  process.argv[1] !== undefined &&
  realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked)
  main(process.argv.slice(2)).then(
    (out) => {
      if (out !== undefined)
        process.stdout.write(`${JSON.stringify(out, null, 2)}\n`);
      // A rejected proof: Lean's messages as they are, to read them line by line.
      const messages = (out as { messages?: unknown } | undefined)?.messages;
      if (typeof messages === "string") process.stderr.write(`\n${messages}\n`);
    },
    (e: unknown) => {
      // Usage errors print the usage text; failures print {reason, status?, details?, messages?},
      // then LeanGround's messages as they are, to read Lean's errors line by line.
      process.stderr.write(
        e instanceof UsageError
          ? `${e.message}\n`
          : `${JSON.stringify(e instanceof MdcError ? e : { reason: "error", details: e instanceof Error ? e.message : String(e) })}\n${e instanceof MdcError && e.messages ? `\n${e.messages}\n` : ""}`,
      );
      process.exitCode = e instanceof UsageError ? 2 : 1;
    },
  );
