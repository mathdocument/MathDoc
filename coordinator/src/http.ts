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
  newBoard,
  owner,
  requireThat,
  nodeRefSchema,
  ingest,
} from "./domain.js";
import { resolveProject } from "./worker.js";
async function body(req: IncomingMessage) {
  const chunks: Buffer[] = [];
  let length = 0;
  for await (const chunk of req) {
    length += chunk.length;
    requireThat(length <= 2200000, "request_too_large", 413);
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
  return createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://localhost");
      const path = url.pathname;
      const method = req.method;
      if (!path.startsWith("/api/")) {
        requireThat(method === "GET", "method_not_allowed", 405);
        const asset =
          path === "/" ? "index.html" : decodeURIComponent(path).slice(1);
        const root = resolve(cfg.appDir);
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
      if (path === "/api/documents" && method === "GET") {
        requireThat(cfg.actors[actor].admin, "admin_required", 403);
        const ref = bindingSchema.parse(Object.fromEntries(url.searchParams));
        send(res, 200, await docs.load(ref));
        return;
      }
      if (path === "/api/projects" && method === "GET") {
        send(res, 200, await store.list(actor));
        return;
      }
      if (path === "/api/projects" && method === "POST") {
        requireThat(cfg.actors[actor].admin, "admin_required", 403);
        const input = createSchema.parse(await body(req));
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
        /^\/api\/projects\/([\w-]+)(?:\/(commands|jobs|history|documents|document-history|branches|facts)(?:\/([\w-]+))?)?$/.exec(
          path,
        );
      requireThat(match, "not_found", 404);
      const [, id, operation, child] = match;
      const b = await store.get(id, actor);
      if (!operation && method === "GET") {
        send(res, 200, b);
        return;
      }
      if (operation === "commands" && method === "POST") {
        const input = commandSchema.parse(await body(req));
        if (input.type === "members")
          requireThat(
            input.members.every((m) => Object.hasOwn(cfg.actors, m)),
            "unknown_member",
            400,
          );
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
      if (e instanceof z.ZodError)
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
