import { createServer, IncomingMessage, ServerResponse } from "node:http";
import { createHash, timingSafeEqual } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve, extname } from "node:path";
import { z } from "zod";
import { Store } from "./store.js";
import { Config } from "./config.js";
import { LeanGround, RemoteError } from "./remote.js";
import { Documents, nodeSchema } from "./documents.js";
import {
  Fault,
  createSchema,
  commandSchema,
  documentCreateSchema,
  newBoard,
  newDocumentBoard,
  owner,
  requireThat,
  nodeRefSchema,
  ingest,
  type Board,
} from "./domain.js";
import { fetchBase, resolveProject } from "./worker.js";
import { DocsBackend } from "./docs/routes.js";
import { DocError } from "./docs/names.js";
import type { Action } from "./docs/workspace.js";
import {
  convert,
  submission,
  conclusionProposition,
  environmentInputSchema,
  Environments,
} from "./proof.js";
async function body(req: IncomingMessage, limit = 2200000) {
  const chunks: Buffer[] = [];
  let length = 0;
  for await (const chunk of req) {
    length += chunk.length;
    requireThat(length <= limit, "request_too_large", 413);
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString() || "{}");
  } catch {
    throw new Fault("invalid_json", 400);
  }
}
function send(res: ServerResponse, status: number, value: unknown) {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  res.end(JSON.stringify(value));
}
function authenticate(req: IncomingMessage, cfg: Config) {
  const token = req.headers.authorization?.replace(/^Bearer /, "") ?? "";
  for (const [actor, entry] of Object.entries(cfg.actors)) {
    const a = Buffer.from(token),
      b = Buffer.from(entry.token);
    if (a.length === b.length && timingSafeEqual(a, b)) return actor;
  }
  throw new Fault("unauthorized", 401);
}
function key(req: IncomingMessage) {
  return z.string().min(1).max(200).parse(req.headers["idempotency-key"]);
}
function revision(req: IncomingMessage) {
  const h = z
    .string()
    .regex(/^"\d+"$/)
    .parse(req.headers["if-match"]);
  return Number(h.slice(1, -1));
}
const bindingSchema = nodeRefSchema.pick({ database: true, branch: true });
export function server(
  cfg: Config,
  store: Store,
  lean: LeanGround,
  docs: Documents,
) {
  // Document API and the retained web/ editor (plan §6.1, §6.2). Collaboration lives
  // under /api/coordination/ (plan §6.3); /api/me is shared.
  const documents =
    cfg.terminusUrl && cfg.terminusPassword
      ? new DocsBackend({
          terminus: {
            url: cfg.terminusUrl,
            user: cfg.terminusUser,
            password: cfg.terminusPassword,
          },
          pool: store.pool,
          webDir: cfg.webDir ?? "web/dist",
          publicOrigin: cfg.publicOrigin,
          actors: cfg.actors,
          authenticate: (req) => authenticate(req, cfg),
          readBody: body,
          boards: (database, branch) => store.bound(database, branch),
        })
      : null;
  const environments = new Environments(store.pool);
  // Plan §6.3: collaboration never widens document access. Each project route re-checks
  // the actor's role on the bound branch; a missing role reads as not found.
  const documentRole = async (
    actor: string,
    database: string,
    branch: string,
    action: Action,
  ) => {
    requireThat(documents, "documents_not_configured", 503);
    await documents.workspaces.authorize(actor, database, branch, action);
  };
  const membersCanRead = async (
    members: string[],
    database: string,
    branch: string,
  ) => {
    for (const m of members)
      try {
        await documentRole(m, database, branch, "read");
      } catch (e) {
        if (e instanceof DocError)
          throw new Fault("member_lacks_document_access", 400);
        throw e;
      }
  };
  return createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://localhost");
      if (documents && !cfg.appDir && (await documents.handle(req, res, url)))
        return;
      if (
        documents &&
        cfg.appDir &&
        (url.pathname.startsWith("/p/") || url.pathname === "/api/status")
      )
        if (await documents.handle(req, res, url)) return;
      const coordination = url.pathname.startsWith("/api/coordination/");
      const path = coordination
        ? `/api${url.pathname.slice("/api/coordination".length)}`
        : url.pathname;
      const method = req.method;
      if (!path.startsWith("/api/")) {
        requireThat(cfg.appDir, "not_found", 404);
        requireThat(method === "GET", "method_not_allowed", 405);
        const asset =
          path === "/" ? "index.html" : decodeURIComponent(path).slice(1);
        const root = resolve(cfg.appDir!);
        const file = resolve(root, asset);
        requireThat(file.startsWith(`${root}/`), "not_found", 404);
        let data: Buffer;
        try {
          data = await readFile(file);
        } catch {
          throw new Fault("not_found", 404);
        }
        const types: Record<string, string> = {
          ".html": "text/html; charset=utf-8",
          ".js": "text/javascript",
          ".css": "text/css",
          ".woff2": "font/woff2",
          ".svg": "image/svg+xml",
        };
        res.writeHead(200, {
          "content-type": types[extname(file)] ?? "application/octet-stream",
          "x-content-type-options": "nosniff",
          "cache-control":
            asset === "index.html" ? "no-store" : "public,max-age=3600",
          "content-security-policy":
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'",
        });
        res.end(data);
        return;
      }
      const actor = authenticate(req, cfg);
      if (path === "/api/me" && method === "GET") {
        send(res, 200, { actor, admin: cfg.actors[actor].admin });
        return;
      }
      requireThat(coordination, "not_found", 404);
      if (path === "/api/documents" && method === "GET") {
        requireThat(cfg.actors[actor].admin, "admin_required", 403);
        const ref = bindingSchema.parse(Object.fromEntries(url.searchParams));
        send(res, 200, await docs.load(ref));
        return;
      }
      const envPath = /^\/api\/environments\/([\w-]+)\/([\w-]+)$/.exec(path);
      if (envPath) {
        const [, database, branch] = envPath;
        if (method === "GET") {
          await documentRole(actor, database, branch, "read");
          const env = await environments.get(database, branch);
          requireThat(env, "environment_not_configured", 404);
          send(res, 200, env);
          return;
        }
        requireThat(method === "PUT", "method_not_allowed", 405);
        // The environment decides what every new ProofRequest means: owners only.
        await documentRole(actor, database, branch, "member_manage");
        const input = environmentInputSchema.parse(await body(req));
        const base = await fetchBase(lean, input.base_key);
        send(
          res,
          200,
          await environments.put(database, branch, input, base, actor),
        );
        return;
      }
      if (path === "/api/projects" && method === "GET") {
        const q = url.searchParams;
        const database = q.get("database");
        const branch = q.get("branch");
        if (database && branch) {
          await documentRole(actor, database, branch, "read");
          send(
            res,
            200,
            await store.list(actor, {
              database,
              branch,
              node: q.get("node") ?? undefined,
            }),
          );
        } else send(res, 200, await store.list(actor));
        return;
      }
      if (path === "/api/projects" && method === "POST") {
        const raw = await body(req);
        if (raw && typeof raw === "object" && "document" in raw) {
          const input = documentCreateSchema.parse(raw);
          const { database, branch, node } = input.document;
          await documentRole(actor, database, branch, "proof_request_create");
          requireThat(
            input.members.every((m) => Object.hasOwn(cfg.actors, m)),
            "unknown_member",
            400,
          );
          await membersCanRead(input.members, database, branch);
          const env = await environments.get(database, branch);
          requireThat(env, "environment_not_configured");
          requireThat(documents, "documents_not_configured", 503);
          const snap = await documents.terminus
            .database(database, branch)
            .load();
          const root = snap.nodes.get(node);
          requireThat(root, "document_not_found", 404);
          requireThat(
            root.blocks.some((x) => x.srctype === "lean"),
            "root_has_no_lean_block",
          );
          const k = key(req);
          const digest = createHash("sha256")
            .update(JSON.stringify({ input, env: env.environment_id }))
            .digest("hex");
          const id =
            "mdc-" +
            createHash("sha256")
              .update(`${actor}\0${k}`)
              .digest("hex")
              .slice(0, 32);
          const b = {
            ...newDocumentBoard(id, actor, input, root.title, env),
            creation_hash: digest,
          };
          try {
            await store.create(b);
          } catch (e) {
            if ((e as { code?: string }).code !== "23505") throw e;
            const existing = await store.get(id, actor);
            requireThat(
              (existing as typeof b).creation_hash === digest,
              "idempotency_conflict",
            );
            send(res, 200, existing);
            return;
          }
          send(res, 201, b);
          return;
        }
        requireThat(cfg.actors[actor].admin, "admin_required", 403);
        const input = createSchema.parse(raw);
        requireThat(
          input.members.every((m) => Object.hasOwn(cfg.actors, m)),
          "unknown_member",
          400,
        );
        const k = key(req);
        const digest = createHash("sha256")
          .update(JSON.stringify(input))
          .digest("hex");
        const id =
          "mdc-" +
          createHash("sha256")
            .update(`${actor}\0${k}`)
            .digest("hex")
            .slice(0, 32);
        const prior = (
          await store.pool.query("SELECT body FROM mdc_project WHERE id=$1", [
            id,
          ])
        ).rows[0]?.body;
        if (prior) {
          requireThat(prior.creation_hash === digest, "idempotency_conflict");
          send(res, 200, prior);
          return;
        }
        if (input.node_ref) {
          const s = await docs.load(input.node_ref);
          requireThat(
            s.version === input.node_ref.revision &&
              s.nodes.some((n) => n.fnode === input.node_ref!.node),
            "document_revision_conflict",
          );
        }
        const resolved = await resolveProject(
          lean,
          input.base_key,
          input.proposition,
          input.context,
          input.options,
        );
        const b = {
          ...newBoard(id, actor, input, resolved.base, resolved.goal),
          creation_hash: digest,
        };
        try {
          await store.create(b);
        } catch (e) {
          if ((e as { code?: string }).code !== "23505") throw e;
          const existing = await store.get(id, actor);
          requireThat(
            (existing as typeof b).creation_hash === digest,
            "idempotency_conflict",
          );
          send(res, 200, existing);
          return;
        }
        send(res, 201, b);
        return;
      }
      const match =
        /^\/api\/projects\/([\w-]+)(?:\/(commands|jobs|history|documents|document-history|branches|facts|nodes)(?:\/([\w-]+))?(?:\/(draft))?)?$/.exec(
          path,
        );
      requireThat(match, "not_found", 404);
      const [, id, operation, child, draft] = match;
      const b: Board = await store.get(id, actor);
      if (b.request) {
        try {
          await documentRole(
            actor,
            b.request.database,
            b.request.branch,
            "read",
          );
        } catch (e) {
          if (e instanceof DocError) throw new Fault("not_found", 404);
          throw e;
        }
      }
      if (operation === "nodes" && child && draft && method === "GET") {
        // Agents start from the converted node: placeholders, definitions and identity.
        requireThat(b.request?.bound && documents, "no_document_binding");
        const snap = await documents!.terminus
          .database(b.request!.database, b.request!.branch)
          .load();
        const c = convert(snap.nodes, b.request!.node, b.context as never);
        const n = c.bindings[child];
        requireThat(n, "not_found", 404);
        requireThat(n.role === "theorem", n.reason ?? "not_a_theorem_node");
        const goalOf = (nid: string) => b.nodes?.[nid]?.goal;
        send(res, 200, {
          node: child,
          goal: goalOf(child) ?? null,
          proposition: conclusionProposition(c, child),
          premises: (n.premises ?? []).map((p) => ({
            node: p,
            name: c.bindings[p].conclusion,
            goal: goalOf(p) ?? null,
          })),
          definitions: (n.definition_nodes ?? []).flatMap(
            (d) => b.nodes?.[d]?.definition_ids ?? [],
          ),
          ...submission(c, child),
          state: n.state,
          reason: n.reason,
          details: n.details,
        });
        return;
      }
      if (!operation && method === "GET") {
        send(res, 200, b);
        return;
      }
      if (operation === "commands" && method === "POST") {
        const input = commandSchema.parse(await body(req));
        if (input.type === "members") {
          requireThat(
            input.members.every((m) => Object.hasOwn(cfg.actors, m)),
            "unknown_member",
            400,
          );
          if (b.request)
            await membersCanRead(
              input.members,
              b.request.database,
              b.request.branch,
            );
        }
        send(
          res,
          200,
          await store.apply(id, actor, revision(req), key(req), input),
        );
        return;
      }
      if (operation === "jobs" && method === "GET") {
        send(res, 200, await store.jobs(id, actor));
        return;
      }
      if (operation === "history" && method === "GET") {
        send(res, 200, await store.history(id, actor));
        return;
      }
      if (operation === "facts" && child && method === "GET") {
        requireThat(b.facts[child], "not_found", 404);
        const f = await lean.fact("read", {
          kind: "certificate",
          id: child,
          source: true,
        });
        ingest(structuredClone(b), f);
        send(res, 200, f);
        return;
      }
      requireThat(b.node_ref, "no_document_binding");
      if (operation === "documents" && method === "GET") {
        send(res, 200, await docs.load(b.node_ref));
        return;
      }
      if (operation === "documents" && method === "PUT") {
        owner(b, actor);
        const input = z
          .object({ version: z.string().min(1), node: nodeSchema })
          .strict()
          .parse(await body(req));
        send(res, 200, {
          version: await docs.put(b.node_ref, input.node, input.version, actor),
        });
        return;
      }
      if (operation === "document-history" && method === "GET") {
        send(res, 200, await docs.history(b.node_ref));
        return;
      }
      if (operation === "branches" && method === "POST") {
        owner(b, actor);
        const input = z
          .object({ name: z.string().regex(/^[\w-]+$/) })
          .strict()
          .parse(await body(req));
        send(res, 201, await docs.branch(b.node_ref, input.name));
        return;
      }
      throw new Fault("not_found", 404);
    } catch (e) {
      if (e instanceof DocError)
        send(res, e.status, {
          reason:
            e.status === 404
              ? "not_found"
              : e.status === 403
                ? "document_permission_denied"
                : "document_error",
          error: e.message,
        });
      else if (e instanceof z.ZodError)
        send(res, 400, {
          reason: "invalid_request",
          issues: e.issues.map((i) => ({ path: i.path, message: i.message })),
        });
      else if (e instanceof Fault)
        send(res, e.status, {
          reason: e.code,
          ...(e instanceof RemoteError ? { retryable: e.retryable } : {}),
        });
      else {
        console.error(
          "request failed",
          e instanceof Error ? e.name : "unknown",
        );
        send(res, 500, { reason: "internal_error" });
      }
    }
  });
}
