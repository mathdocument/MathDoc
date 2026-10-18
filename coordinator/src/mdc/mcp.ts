// `mdc mcp`: the proof verbs as tools of a Model Context Protocol server over stdio
// (docs/mathdoc.md §2.5, §2.7.4). Newline-delimited JSON-RPC 2.0 on stdin/stdout. Chat clients
// may have no file system, so the tools take and return source text; the server keeps the goal
// files and their `.mdc/` records in its own directory, so replays and leases work as in the CLI.
import { createInterface } from "node:readline";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { MODULE_PATH } from "../domain.js";
import { MdcError } from "./client.js";
import * as verbs from "./verbs.js";
import type { VerbContext } from "./verbs.js";

const VERSIONS = ["2025-06-18", "2025-03-26", "2024-11-05"];

const file = {
  type: "string",
  description:
    "The goal file name returned by pull; in a module project, the goal directory it returned",
};
/** Module projects (docs/mathdoc.md §4.13): a goal is a directory of Lean files. */
const files = {
  type: "object",
  additionalProperties: { type: "string" },
  description:
    "Module projects instead of text: Lean files of the goal directory by path (e.g. Work/Main.lean), written before the call; shared modules cannot be changed",
};
export const TOOLS = [
  {
    name: "pull",
    description:
      "Claim a goal of the project and return its goal file: a read-only header (context, definitions, notes) above the line `-- mdc: edit below this line`, and the goal's theorem with a `sorry` proof below it. Edit only below that line, then call prove or split with the whole text.",
    inputSchema: {
      type: "object",
      properties: {
        goal: {
          type: "string",
          description:
            "A GoalKey, a unique prefix of at least 8 hex digits, or a node title or UUID",
        },
        branch: { type: "string" },
        with: {
          type: "array",
          items: { type: "string" },
          description: "More goals to offer as premises",
        },
        ttl: { type: "integer", minimum: 30, maximum: 3600 },
        allocation: { type: "integer", minimum: 0 },
        claim: {
          type: "boolean",
          description: "false writes the file without taking a lease",
        },
      },
      required: ["goal"],
    },
  },
  {
    name: "prove",
    description:
      "Submit the goal file text as a complete proof (no `sorry` premise may remain).",
    inputSchema: {
      type: "object",
      properties: {
        file,
        text: { type: "string", description: "The whole edited goal file" },
        files,
        root: { type: "string", description: "Module projects: another root theorem" },
        wait: { type: "boolean", description: "Wait for the check" },
      },
      required: ["file"],
    },
  },
  {
    name: "split",
    description:
      "Submit the goal file text as a decomposition: new `theorem … := sorry` premises become subgoals. It waits for the project owner's review before its subgoals can be claimed.",
    inputSchema: {
      type: "object",
      properties: {
        file,
        text: { type: "string", description: "The whole edited goal file" },
        files,
        root: { type: "string", description: "Module projects: another root theorem" },
        shared: {
          type: "array",
          items: { type: "string" },
          description:
            "Module projects: paths of files holding definitions the new goals' statements use; they are registered as shared modules",
        },
        description: {
          type: "string",
          description: "What the split does, for its review: Markdown, math in $…$ or $$…$$",
        },
        wait: { type: "boolean" },
      },
      required: ["file", "description"],
    },
  },
  {
    name: "check",
    description:
      "Have LeanGround check the goal file text now, on the project's base, and return Lean's diagnostics with line numbers. Nothing is submitted or recorded; use it before prove or split.",
    inputSchema: {
      type: "object",
      properties: {
        file,
        text: { type: "string", description: "The whole edited goal file" },
        files,
        root: { type: "string", description: "Module projects: another root theorem" },
      },
      required: ["file"],
    },
  },
  {
    name: "lemma",
    description:
      "Find library declarations (Mathlib and published results of the project's base) whose names contain one of the queries, with their types. Look a lemma up before citing it.",
    inputSchema: {
      type: "object",
      properties: {
        queries: {
          type: "array",
          items: { type: "string" },
          minItems: 1,
          maxItems: 10,
          description: "Parts of declaration names, e.g. `pow_of_mul_eq_pow`",
        },
        kind: { type: "string", enum: ["theorem", "definition", "all"] },
        limit: { type: "integer", minimum: 1, maximum: 100 },
      },
      required: ["queries"],
    },
  },
  {
    name: "done",
    description:
      "End the lease on a goal file, with what was spent and learned.",
    inputSchema: {
      type: "object",
      properties: {
        file,
        spent: { type: "integer", minimum: 0 },
        outcome: {
          type: "string",
          enum: ["submitted", "no_progress", "execution_failed", "released"],
        },
        note: {
          type: "string",
          maxLength: 500,
          description: "What was learned: Markdown, math in $…$",
        },
      },
      required: ["file"],
    },
  },
  {
    name: "note",
    description:
      "Keep a note on a goal (what was tried, where it got stuck) for whoever works on it next. Markdown, with math in $…$ (the web page typesets it; bare TeX such as p^{k} stays plain text).",
    inputSchema: {
      type: "object",
      properties: {
        file,
        goal: { type: "string", description: "A goal reference, if no file" },
        text: {
          type: "string",
          maxLength: 500,
          description: "Markdown, math in $…$",
        },
      },
      required: ["text"],
    },
  },
];

type Args = Record<string, unknown>;
const str = (a: Args, k: string) =>
  typeof a[k] === "string" ? (a[k] as string) : undefined;
/** Goal files and directories live directly in the server's directory, by the names pull gives them. */
function fileName(a: Args) {
  const name = str(a, "file");
  if (!name || !/^[A-Za-z0-9._'-]+$/.test(name) || name.startsWith("."))
    throw new MdcError(
      "invalid_argument",
      "file must be a name returned by pull",
    );
  return name;
}

export async function call(
  base: Omit<VerbContext, "flag" | "has" | "all">,
  name: string,
  a: Args,
): Promise<unknown> {
  const flags = new Map<string, string>();
  const many = new Map<string, string[]>();
  const ctx: VerbContext = {
    ...base,
    flag: (n) => flags.get(n),
    all: (n) => many.get(n) ?? [],
    has: (n) => flags.has(n),
  };
  /** Write the edited text of a pulled file, or the given files of a pulled directory. */
  const edit = async (f: string) => {
    const { record } = await verbs.pulled(ctx, f);
    if (record.format === "mdc-module-dir-record/1") {
      const given = a.files;
      if (given === undefined) return;
      if (!given || typeof given !== "object" || Array.isArray(given))
        throw new MdcError("invalid_argument", "files maps paths to Lean text");
      for (const [path, text] of Object.entries(given)) {
        if (!MODULE_PATH.test(path) || typeof text !== "string")
          throw new MdcError("invalid_argument", `${path}: a Lean file path such as Work/Main.lean, with its text`);
        if (Object.hasOwn(record.shared.files, path))
          throw new MdcError("shared_module_modified", `${path} is a shared module`);
        const target = resolve(ctx.cwd, f, ...path.split("/"));
        await mkdir(dirname(target), { recursive: true });
        await writeFile(target, text);
      }
      return;
    }
    const text = str(a, "text");
    if (text === undefined)
      throw new MdcError("invalid_argument", "text is required");
    await writeFile(resolve(ctx.cwd, f), text);
  };
  const set = (flag: string, value: unknown) => {
    if (value !== undefined && value !== null) flags.set(flag, String(value));
  };
  switch (name) {
    case "pull": {
      set("--branch", a.branch);
      if (Array.isArray(a.with)) set("--with", a.with.join(","));
      set("--ttl", a.ttl);
      set("--allocation", a.allocation);
      if (a.claim === false) set("--no-claim", true);
      const out = await verbs.pull(ctx, str(a, "goal"));
      if ("directory" in out) return { ...out, files: await readDirectory(resolve(ctx.cwd, out.directory)) };
      return {
        ...out,
        text: await readFile(resolve(ctx.cwd, out.file), "utf8"),
      };
    }
    case "prove":
    case "split": {
      const f = fileName(a);
      // Only a pulled file is written: the record must exist first.
      await edit(f);
      if (a.wait === true) set("--wait", true);
      set("--message", a.description);
      set("--root", a.root);
      if (Array.isArray(a.shared))
        many.set("--shared", a.shared.map((x) => resolve(ctx.cwd, f, String(x))));
      return verbs.submit(ctx, f, name);
    }
    case "check": {
      const f = fileName(a);
      await edit(f);
      set("--root", a.root);
      return verbs.check(ctx, f);
    }
    case "lemma": {
      const queries = Array.isArray(a.queries)
        ? a.queries.filter((q): q is string => typeof q === "string")
        : [];
      set("--kind", a.kind);
      set("--max", a.limit);
      return verbs.lemma(ctx, queries);
    }
    case "done":
      set("--spent", a.spent);
      set("--outcome", a.outcome);
      set("--note", a.note);
      return verbs.done(ctx, fileName(a));
    case "note":
      set("--message", a.text);
      return verbs.note(
        ctx,
        a.file !== undefined ? fileName(a) : str(a, "goal"),
      );
  }
  throw new MdcError("unknown_tool", name);
}

/** A pulled goal directory's text files (Lean files and GOAL.md) by path. */
async function readDirectory(dir: string) {
  const out: Record<string, string> = {};
  const walk = async (d: string) => {
    for (const e of await readdir(d, { withFileTypes: true })) {
      if (e.name.startsWith(".")) continue;
      const full = join(d, e.name);
      if (e.isDirectory()) await walk(full);
      else if (e.name.endsWith(".lean") || e.name === "GOAL.md")
        out[relative(dir, full).split(sep).join("/")] = await readFile(full, "utf8");
    }
  };
  await walk(dir);
  return out;
}

/** Serve until the input ends. Tool calls run one at a time, in order. */
export async function serve(
  base: Omit<VerbContext, "flag" | "has" | "all">,
  input: NodeJS.ReadableStream,
  output: NodeJS.WritableStream,
  version: string,
) {
  const reply = (id: unknown, body: Record<string, unknown>) =>
    output.write(`${JSON.stringify({ jsonrpc: "2.0", id, ...body })}\n`);
  let queue = Promise.resolve();
  const handle = async (line: string) => {
    let m: { id?: unknown; method?: unknown; params?: Args };
    try {
      m = JSON.parse(line);
    } catch {
      reply(null, { error: { code: -32700, message: "Parse error" } });
      return;
    }
    const notification = m.id === undefined || m.id === null;
    const p = m.params ?? {};
    switch (m.method) {
      case "initialize": {
        const asked =
          typeof p.protocolVersion === "string" ? p.protocolVersion : "";
        reply(m.id, {
          result: {
            protocolVersion: VERSIONS.includes(asked) ? asked : VERSIONS[0],
            capabilities: { tools: { listChanged: false } },
            serverInfo: { name: "mdc", version },
            instructions:
              "Prove goals of a MathDoc project: pull a goal, edit its file below the separator line, then prove (complete proof) or split (decomposition with new sorry premises); finish with done. Leave notes on what did not work.",
          },
        });
        return;
      }
      case "ping":
        if (!notification) reply(m.id, { result: {} });
        return;
      case "tools/list":
        reply(m.id, { result: { tools: TOOLS } });
        return;
      case "tools/call": {
        const name = String(p.name ?? "");
        const args =
          p.arguments && typeof p.arguments === "object"
            ? (p.arguments as Args)
            : {};
        try {
          const result = await call(base, name, args);
          reply(m.id, {
            result: {
              content: [
                { type: "text", text: JSON.stringify(result, null, 2) },
              ],
              structuredContent: result,
            },
          });
        } catch (e) {
          const error =
            e instanceof MdcError
              ? e.toJSON()
              : {
                  reason: "error",
                  details: e instanceof Error ? e.message : String(e),
                };
          reply(m.id, {
            result: {
              content: [{ type: "text", text: JSON.stringify(error) }],
              isError: true,
            },
          });
        }
        return;
      }
    }
    if (!notification)
      reply(m.id, {
        error: {
          code: -32601,
          message: `Method not found: ${String(m.method)}`,
        },
      });
  };
  const lines = createInterface({ input, crlfDelay: Infinity });
  for await (const line of lines) {
    if (!line.trim()) continue;
    queue = queue.then(() => handle(line));
  }
  await queue;
}
