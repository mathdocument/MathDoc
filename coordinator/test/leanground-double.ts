// A deterministic contract double of LeanGround's fact API (docs/12 of LeanGround) for
// tests without a Lean kernel. It checks shapes, not proofs: a GoalKey is a hash of the
// statement text and the environment, premises are the source's `sorry` theorems, and an
// assembly always certifies. A source with a `-- wrong:` comment stands for a proof Lean refuses:
// it is rejected with a Lean-like message, and a draft check reports it on that line. Retrieval
// searches a small fixed library by name. Results are only as faithful as these rules.
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import { once } from "node:events";
import { proposition, scan } from "../src/lean-source.js";

const digest = (value: unknown) =>
  createHash("sha256").update(JSON.stringify(value)).digest("hex");

export const BASE = {
  base_id: "double-base",
  base_key: 1,
  root_module: "Init",
  lean_version: "4.0.0",
  lean_githash: "0",
  package_revision: "0",
  package_manifest_hash: "0",
  module_list_hash: "0",
  build_flags_hash: "0",
  platform: "test",
  promotion_generation: 0,
  schema_version: 1,
};

type Body = Record<string, any>;

/**
 * Module projects (/v1/modules/*): the theorems of a file set, by full name (`namespace` blocks
 * at column 0 prefix their names, `_root_.` drops them), with the signature text and whether the
 * proof is `sorry`.
 * A goal's identity is its signature text; a proof "connects" when its root has the same one.
 */
function theorems(files: Record<string, string>) {
  const out: {
    name: string;
    signature: string;
    sorry: boolean;
    proof: string;
    file: string;
  }[] = [];
  for (const [file, text] of Object.entries(files)) {
    const ns: string[] = [];
    const lines = text.split("\n");
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const open = /^namespace\s+(\S+)/.exec(line);
      if (open) ns.push(open[1]);
      if (/^end\b/.test(line)) ns.pop();
      const m = /^(?:theorem|lemma)\s+([\w.']+)(.*)$/.exec(line);
      if (!m) continue;
      let rest = m[2];
      for (let j = i + 1; j < lines.length && !/^\S/.test(lines[j]); j++)
        rest += `\n${lines[j]}`;
      const at = rest.indexOf(":=");
      if (at < 0) continue;
      out.push({
        name: m[1].startsWith("_root_.")
          ? m[1].slice("_root_.".length)
          : [...ns, m[1]].join("."),
        signature: rest.slice(0, at).replace(/\s+/g, " ").trim(),
        sorry: /^\s*(by\s+)?sorry\s*$/.test(rest.slice(at + 2)),
        proof: rest.slice(at + 2),
        file,
      });
    }
  }
  return out;
}
const canonical = (files: Record<string, string>) =>
  JSON.stringify(
    Object.fromEntries(
      Object.entries(files).sort(([a], [b]) => (a < b ? -1 : 1)),
    ),
  );

/** The declarations retrieval knows. */
const LIBRARY = [
  { name: "Nat.add_comm", type: "∀ (n m : ℕ), n + m = m + n" },
  { name: "Nat.mul_comm", type: "∀ (n m : ℕ), n * m = m * n" },
  { name: "Nat.two_mul", type: "∀ (n : ℕ), 2 * n = n + n" },
];

export async function leangroundDouble(port = 0) {
  const certificates = new Map<string, Body>();
  const definitions = new Map<string, Body>();
  const modules = new Map<string, Body>();
  /** GoalKey → its statement signature (the first source registering it). */
  const statements = new Map<string, string>();
  const events: { sequence: number; kind: string; fact_id: string }[] = [];
  const calls: string[] = [];
  const identity = (statement: string, env: Body) => {
    const ctx = digest(env.context ?? {});
    const opts = digest(env.options ?? {});
    return {
      key: digest([statement.replace(/\s+/g, " ").trim(), ctx, opts]),
      base_fingerprint: "base",
      local_context_hash: ctx,
      relevant_options_hash: opts,
    };
  };
  const fail = (reason: string, status = 400) => ({ status, body: { reason } });
  const handle = (
    path: string,
    body: Body,
  ): { status: number; body: unknown } => {
    calls.push(path);
    if (path === "/v1/bases/1")
      return { status: 200, body: { base: { ...BASE, fingerprint: "base" } } };
    if (path === "/v1/check") {
      const wrong = String(body.source)
        .split("\n")
        .findIndex((l) => l.includes("-- wrong:"));
      return {
        status: 200,
        body: {
          ok: wrong < 0,
          verdict: true,
          stage: wrong < 0 ? null : "decl",
          base: body.bases?.[0] ?? "Init",
          diagnostics:
            wrong < 0
              ? []
              : [
                  {
                    severity: "error",
                    line: wrong + 1,
                    col: 2,
                    text: "Tactic `rfl` failed: the left-hand side is not definitionally equal to the right-hand side",
                  },
                ],
          axioms: {},
          trust: null,
          missing_imports: [],
          ms: 1,
        },
      };
    }
    if (path === "/v1/retrieval") {
      // LeanGround's BaseRef parser is strict: a derived fingerprint is refused.
      if (!body.base?.base_key || "fingerprint" in body.base)
        return fail("wire_error");
      const hits = (body.queries as string[]).flatMap((q) =>
        LIBRARY.filter((d) => d.name.includes(q)).map((d) => ({
          declaration_id: d.name,
          base_id: BASE.base_id,
          stored_base_key: 1,
          node_id: LIBRARY.indexOf(d),
          name: d.name,
          kind: "theorem",
          type: d.type,
          trust: 3,
          axioms: [],
          evidence: { request_id: body.request_id, matched_query: q },
        })),
      );
      return { status: 200, body: { hits: hits.slice(0, body.limit) } };
    }
    if (path.startsWith("/v1/modules/"))
      return moduleCall(path.slice("/v1/modules/".length), body);
    const op = path.replace("/v1/facts/", "");
    switch (op) {
      case "project":
        return { status: 200, body: { id: body.id } };
      case "resolve":
        // LeanGround reports the registered definitions the statement references; the
        // double approximates that by name.
        return {
          status: 200,
          body: {
            goal_key: identity(body.proposition, body),
            definitions: (body.definitions ?? []).filter((id: string) => {
              const d = definitions.get(id);
              return d && new RegExp(`\\b${d.name}\\b`).test(body.proposition);
            }),
          },
        };
      case "define": {
        const out = scan(body.request.source).decls.map((d) => {
          const text = body.request.source.slice(d.start, d.end);
          const id = digest(["definition", d.name, text]);
          definitions.set(id, {
            id,
            name: d.name,
            source: text,
            deps: body.request.definitions ?? [],
          });
          return { id, name: d.name };
        });
        return { status: 200, body: { definitions: out } };
      }
      case "submit": {
        const r = body.request;
        const s = scan(r.source);
        const root = s.decls.find((d) => d.name === r.expected_root);
        if (!root?.type) return fail("unsupported_source");
        if (root.sorry) return fail("invalid_bindings");
        const placeholders = s.decls.filter((d) => d.sorry && d !== root);
        if (r.mode === "sketch" && !placeholders.length)
          return fail("no_premises");
        if (r.mode !== "sketch" && placeholders.length)
          return fail("sorry_outside_subgoals");
        const goal = identity(proposition(root), r);
        if (r.goal && r.goal.key !== goal.key)
          return fail("proposition_differs");
        if (r.source.includes("-- wrong:"))
          return {
            status: 400,
            body: {
              kind: "fact_error",
              reason: "verification_failed",
              error:
                "<input>:3:2: error: Tactic `rfl` failed: the left-hand side is not definitionally equal to the right-hand side",
            },
          };
        const premises = placeholders.map((d) => ({
          name: d.name,
          goal: identity(proposition(d), r),
        }));
        const id = digest([
          "certificate",
          body.component,
          r.source,
          goal.key,
          r.definitions ?? [],
        ]);
        const record = {
          id,
          goal: goal.key,
          premises: [...new Set(premises.map((p) => p.goal.key))],
          trust: "trusted",
          request: { goal },
          access: body.scope,
          caller_ref: body.caller_ref ?? null,
          source: r.source,
          bindings: [
            { name: root.name, role: "conclusion", goal: goal.key },
            ...premises.map((p) => ({
              name: p.name,
              role: "premise",
              goal: p.goal.key,
            })),
          ],
          definitions: r.definitions ?? [],
        };
        if (!certificates.has(id)) {
          certificates.set(id, record);
          events.push({
            sequence: events.length + 1,
            kind: "certificate",
            fact_id: id,
          });
        }
        return { status: 200, body: record };
      }
      case "read": {
        const store =
          body.kind === "definition"
            ? definitions
            : body.kind === "module"
              ? modules
              : certificates;
        const value = store.get(body.id);
        return value ? { status: 200, body: value } : fail("not_found", 404);
      }
      case "events": {
        const page = events
          .filter((e) => e.sequence > body.after)
          .slice(0, body.limit ?? 100);
        return {
          status: 200,
          body: {
            cursor: page.at(-1)?.sequence ?? body.after,
            has_more: false,
            events: page,
          },
        };
      }
      case "plan":
        return {
          status: 200,
          body: { id: `plan-${digest(body.route).slice(0, 16)}` },
        };
      case "widen":
        return { status: 200, body: { id: body.id } };
    }
    return fail("unknown", 404);
  };
  /** The module operations: compile = "no `-- wrong:` line"; connect = same signature text. */
  function moduleCall(
    op: string,
    body: Body,
  ): { status: number; body: unknown } {
    const files = (body.files ?? {}) as Record<string, string>;
    const bad = Object.entries(files).find(([, t]) => t.includes("-- wrong:"));
    const diagnostics = bad
      ? [
          {
            file: bad[0],
            line:
              bad[1].split("\n").findIndex((l) => l.includes("-- wrong:")) + 1,
            column: 2,
            severity: "error",
            message: "type mismatch",
          },
        ]
      : [];
    const compileError = {
      status: 400,
      body: {
        kind: "fact_error",
        reason: "compile_failed",
        error: JSON.stringify({ file: bad?.[0], diagnostics }),
      },
    };
    const goalOf = (signature: string) => {
      const goal = identity(signature, {});
      if (!statements.has(goal.key)) statements.set(goal.key, signature);
      return goal;
    };
    // Contribution checks: the root connects to the goal and the placeholders it uses.
    const certify = () => {
      const all = theorems(files);
      const root = all.find((t) => t.name === body.root);
      const violations: string[] = [];
      if (!root) violations.push(`root_not_found ${body.root}`);
      else if (root.sorry) violations.push("root_is_placeholder");
      else if (statements.get(body.goal?.key) !== root.signature)
        violations.push("kernel: (kernel) declaration type mismatch");
      // Like the kernel's reachability: placeholders the root's proof mentions.
      const premises = all.filter(
        (t) =>
          t.sorry &&
          t.name !== body.root &&
          root !== undefined &&
          new RegExp(`(^|[^\\w.'])${t.name.split(".").at(-1)}(?![\\w'])`).test(
            root.proof,
          ),
      );
      return { violations, premises, root };
    };
    switch (op) {
      case "shared": {
        if (bad) return compileError;
        const id = digest(["module", canonical(files), body.shared ?? []]);
        modules.set(id, {
          id,
          modules: Object.keys(files)
            .sort()
            .map((p) => ({ module: p.slice(0, -5).split("/").join(".") })),
          shared: body.shared ?? [],
          files_sha256: digest(canonical(files)),
          source: canonical(files),
          trust: "trusted",
          access: body.scope,
        });
        return { status: 200, body: modules.get(id) };
      }
      case "resolve": {
        if (bad) return compileError;
        const found = theorems(files).filter((t) => t.sorry);
        const named = body.statement
          ? found.filter((t) => t.name === body.statement)
          : found;
        if (named.length !== 1)
          return fail(
            named.length ? "statement_ambiguous" : "statement_not_found",
          );
        const goal = goalOf(named[0].signature);
        return {
          status: 200,
          body: {
            goal_key: goal,
            statement: {
              bundle: digest(canonical(files)),
              const: named[0].name,
              shared: body.shared ?? [],
            },
          },
        };
      }
      case "check": {
        if (bad)
          return {
            status: 200,
            body: {
              ok: false,
              compiled: false,
              reason: "compile_failed",
              diagnostics,
            },
          };
        if (!body.goal)
          return {
            status: 200,
            body: { ok: true, compiled: true, diagnostics: [] },
          };
        const c = certify();
        return {
          status: 200,
          body: {
            ok: !c.violations.length,
            compiled: true,
            diagnostics: [],
            violations: c.violations,
            premises: c.premises.map((p) => ({ name: p.name })),
            axioms: [],
          },
        };
      }
      case "submit": {
        if (bad) return compileError;
        const c = certify();
        if (c.violations.length)
          return {
            status: 400,
            body: {
              kind: "fact_error",
              reason: "verification_failed",
              error: JSON.stringify(c.violations),
            },
          };
        if (body.mode === "leaf" && c.premises.length)
          return fail("has_premises");
        if (body.mode === "sketch" && !c.premises.length)
          return fail("no_premises");
        const premises = c.premises.map((t) => ({
          name: t.name,
          goal: goalOf(t.signature),
        }));
        const id = digest([
          "module-certificate",
          canonical(files),
          body.root,
          body.goal.key,
          body.shared ?? [],
        ]);
        const record = {
          id,
          goal: body.goal.key,
          premises: [...new Set(premises.map((x) => x.goal.key))].sort(),
          trust: "trusted",
          request: { goal: body.goal, context: {}, options: {} },
          access: body.scope,
          caller_ref: body.caller_ref ?? null,
          source: canonical(files),
          source_format: "lean-modules-v1",
          shared: body.shared ?? [],
          bindings: [
            { name: body.root, role: "conclusion", goal: body.goal.key },
            ...premises.map((x) => ({
              name: x.name,
              role: "premise",
              goal: x.goal.key,
            })),
          ],
        };
        if (!certificates.has(id)) {
          certificates.set(id, record);
          events.push({
            sequence: events.length + 1,
            kind: "certificate",
            fact_id: id,
          });
        }
        return {
          status: 200,
          body: {
            ...record,
            premise_goals: Object.fromEntries(
              premises.map((x) => [x.goal.key, x.goal]),
            ),
          },
        };
      }
    }
    return fail("unknown", 404);
  }
  // Plans remember their goal so that assemble can certify it.
  const plans = new Map<string, string>();
  const server: Server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c);
    const body = JSON.parse(Buffer.concat(chunks).toString() || "{}");
    const path = req.url!.split("?")[0];
    let out: { status: number; body: unknown };
    if (path === "/v1/facts/plan") {
      out = handle(path, body);
      plans.set((out.body as Body).id, body.goal);
    } else if (path === "/v1/facts/assemble") {
      calls.push(path);
      const goal = plans.get(body.plan_id);
      const root = [...certificates.values()].find((c) => c.goal === goal);
      if (!goal || !root) out = fail("unknown_plan", 404);
      else {
        const id = `final-${body.plan_id}`;
        certificates.set(id, {
          ...root,
          id,
          premises: [],
          bindings: root.bindings.filter((b: Body) => b.role === "conclusion"),
        });
        out = {
          status: 200,
          body: {
            plan_id: body.plan_id,
            goal,
            status: "certified",
            certification_id: id,
          },
        };
      }
    } else out = handle(path, body);
    res.writeHead(out.status, { "content-type": "application/json" });
    res.end(JSON.stringify(out.body));
  });
  server.listen(port, "127.0.0.1");
  await once(server, "listening");
  return {
    url: `http://127.0.0.1:${(server.address() as { port: number }).port}`,
    certificates,
    modules,
    calls,
    async close() {
      server.closeAllConnections();
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}
