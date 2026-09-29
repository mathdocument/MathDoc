#!/usr/bin/env node
// `mdc`: a thin command-line client of the MathDoc HTTP API (plan §11; stage 5 replaces the
// Rust CLI). Every command is one or two HTTP requests with the caller's access token;
// output is the JSON response. Node references accept an exact title or a UUID.
//
//   MDC_URL      server origin (default http://127.0.0.1:17843)
//   MDC_TOKEN    access token (required)
//   MDC_PROJECT  default DATABASE/BRANCH for branch commands (or -p DATABASE/BRANCH)
import { realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const USAGE = `usage: mdc [-p DATABASE/BRANCH] <command>

server:     status | init DATABASE | remove DATABASE
            grant ACTOR owner|editor|viewer|none
branch:     branch new NAME | branch del | history | export | import FILE
            project latex show | project latex set --preamble FILE --bib FILE
graph:      graph check|roots|full | search QUERY [-n N]
nodes:      show REF | new -t TITLE [--parent REF] | del REF | rename REF TITLE
            edit REF [--type text|lean|rocq|latex] [--delete]   (source from stdin)
            dep add REF -t TARGET | dep rm REF -t TARGET... | dep show|refs REF [-d DEPTH]
            dep leaf REF | dep candidates REF [QUERY] [-n N] | metric ior REF
proof:      proof env [set]             (set reads the environment JSON from stdin)
            proof submit REF [--budget N] [--members a,b]
            proof list [REF] | proof status ID | proof command ID   (command JSON from stdin)
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
  };
  const boolean = new Set(["--delete", "--help", "-h"]);
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

export async function main(
  argv: string[],
  env: NodeJS.ProcessEnv = process.env,
  input: () => Promise<string> = stdin,
): Promise<unknown> {
  const { positional, flags } = parse(argv);
  const [command, ...args] = positional;
  if (!command || flags["--help"] || flags["-h"]) throw new UsageError(USAGE);
  const origin = (env.MDC_URL ?? "http://127.0.0.1:17843").replace(/\/$/, "");
  const token = env.MDC_TOKEN;
  if (!token) throw new UsageError("set MDC_TOKEN to your access token");
  const flag = (name: string) => flags[name]?.[0];
  const project = flag("--project") ?? env.MDC_PROJECT;
  const need = (value: string | undefined, what: string) => {
    if (!value) throw new UsageError(`missing ${what}\n\n${USAGE}`);
    return value;
  };

  async function request(
    method: string,
    path: string,
    body?: unknown,
    headers: Record<string, string> = {},
  ): Promise<any> {
    const response = await fetch(`${origin}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
        ...headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    }).catch(() => {
      throw new Error(`cannot reach MathDoc at ${origin} (MDC_URL)`);
    });
    const text = await response.text();
    let value: any = null;
    try {
      value = text ? JSON.parse(text) : null;
    } catch {
      value = { error: text };
    }
    if (!response.ok)
      throw new Error(
        `HTTP ${response.status}: ${value?.error ?? value?.reason ?? "request failed"}`,
      );
    return value;
  }
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

  switch (command) {
    case "status":
      return request("GET", "/api/status");
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
      if (args[0] === "new")
        return directory({
          action: "new_branch",
          project: branch(),
          name: need(args[1], "NAME"),
        });
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
      if (sub === "env" && ref === "set")
        return request(
          "PUT",
          coordination(`/environments/${database}/${branchName}`),
          JSON.parse(await input()),
        );
      if (sub === "env")
        return request(
          "GET",
          coordination(`/environments/${database}/${branchName}`),
        );
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
        const id = need(ref, "ID");
        const board = await request("GET", coordination(`/projects/${id}`));
        return request(
          "POST",
          coordination(`/projects/${id}/commands`),
          JSON.parse(await input()),
          {
            "if-match": `"${board.revision}"`,
            "idempotency-key": crypto.randomUUID(),
          },
        );
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
      process.stdout.write(`${JSON.stringify(out, null, 2)}\n`);
    },
    (e: unknown) => {
      process.stderr.write(`${e instanceof Error ? e.message : String(e)}\n`);
      process.exitCode = e instanceof UsageError ? 2 : 1;
    },
  );
