// HTTP front of the document backend (src/server.rs, src/server/projects.rs,
// src/web/assets.rs at a4d61e3). Branch services load on demand: there is no per-branch
// start/stop. Every API request is authenticated and checked against its workspace.
import type { IncomingMessage, ServerResponse } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import type pg from "pg";
import { DocError, projectParts, validateName } from "./names.js";
import { Terminus, type TerminusConfig } from "./terminus.js";
import { BranchService, type Reply } from "./service.js";
import { Workspaces, type Role } from "./workspace.js";

export interface DocsOptions {
  terminus: TerminusConfig;
  pool: pg.Pool;
  webDir: string;
  publicOrigin?: string;
  actors: Record<string, { admin: boolean }>;
  authenticate: (req: IncomingMessage) => string;
  readBody: (req: IncomingMessage) => Promise<unknown>;
}

function send(
  res: ServerResponse,
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
) {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    ...headers,
  });
  res.end(JSON.stringify(body));
}

/** service::allowed_origin: loopback Host (or the public origin), matching Origin if sent. */
export function allowedOrigin(
  req: IncomingMessage,
  publicOrigin?: string,
): boolean {
  const host = req.headers.host ?? "";
  let local = false;
  try {
    const hostname = new URL(`http://${host}`).hostname.replace(/^\[|\]$/g, "");
    local =
      hostname === "localhost" ||
      /^127(\.\d{1,3}){3}$/.test(hostname) ||
      hostname === "::1";
  } catch {
    local = false;
  }
  const publicMatch =
    publicOrigin !== undefined && publicOrigin.split("://")[1] === host
      ? publicOrigin
      : undefined;
  const expected = publicMatch ?? (local ? `http://${host}` : undefined);
  if (!expected) return false;
  const origin = req.headers.origin;
  if (origin === undefined) return true;
  try {
    return new URL(origin).origin === expected;
  } catch {
    return false;
  }
}

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
  ".map": "application/json",
  ".gz": "application/gzip",
  ".txt": "text/plain; charset=utf-8",
};

/** web::assets::serve_asset: direct file, 404 for missing file-like paths, else the SPA shell. */
async function serveWeb(res: ServerResponse, dir: string, path: string) {
  const root = resolve(dir);
  const relative = decodeURIComponent(path).replace(/^\/+/, "");
  const file = resolve(root, relative);
  const inside = file === root || file.startsWith(`${root}/`);
  const reply = (name: string, data: Buffer) => {
    res.writeHead(200, {
      "content-type": MIME[extname(name)] ?? "application/octet-stream",
      "x-content-type-options": "nosniff",
      "cache-control":
        name === "index.html"
          ? "no-store"
          : name.startsWith("assets/")
            ? "public, max-age=31536000, immutable"
            : "public, max-age=0, must-revalidate",
    });
    res.end(data);
  };
  if (inside && relative !== "")
    try {
      return reply(relative, await readFile(file));
    } catch {
      /* fall through */
    }
  const last = relative.split("/").pop() ?? "";
  if (
    relative === "assets" ||
    relative.startsWith("assets/") ||
    last.includes(".")
  ) {
    res.writeHead(404, { "content-type": "text/plain" });
    return res.end("asset not found");
  }
  try {
    return reply("index.html", await readFile(resolve(root, "index.html")));
  } catch {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("frontend not built");
  }
}

export class DocsBackend {
  readonly terminus: Terminus;
  readonly workspaces: Workspaces;
  private branches = new Map<string, BranchService>();

  constructor(private o: DocsOptions) {
    this.terminus = new Terminus(o.terminus);
    this.workspaces = new Workspaces(
      o.pool,
      (actor) => o.actors[actor]?.admin === true,
    );
  }

  private branch(database: string, branch: string) {
    const key = `${database}/${branch}`;
    let service = this.branches.get(key);
    if (!service) {
      service = new BranchService(
        this.terminus.database(database, branch),
        (actor) => actor,
      );
      this.branches.set(key, service);
    }
    return service;
  }
  private forget(prefix: string) {
    for (const [key, service] of this.branches)
      if (key === prefix || key.startsWith(`${prefix}/`)) {
        service.shutdown();
        this.branches.delete(key);
      }
  }

  /** Returns false when the request is not a document request. */
  async handle(
    req: IncomingMessage,
    res: ServerResponse,
    url: URL,
  ): Promise<boolean> {
    const path = url.pathname;
    const isProject = path.startsWith("/p/");
    const isDocApi = path === "/api/status" || path === "/api/projects";
    if (!isProject && !isDocApi) {
      if (path.startsWith("/api/")) return false;
      if (req.method !== "GET") send(res, 405, { error: "GET required" });
      else await serveWeb(res, this.o.webDir, path);
      return true;
    }
    try {
      await this.dispatch(req, res, url, isProject);
    } catch (e) {
      if (e instanceof DocError) send(res, e.status, { error: e.message });
      else {
        console.error(
          "document request failed",
          e instanceof Error ? e.message : "unknown",
        );
        send(res, 500, { error: "internal error" });
      }
    }
    return true;
  }

  private async dispatch(
    req: IncomingMessage,
    res: ServerResponse,
    url: URL,
    isProject: boolean,
  ) {
    const path = url.pathname;
    let rest = "";
    let database = "";
    let branchName = "";
    if (isProject) {
      const segments = path.slice(3).split("/");
      [database, branchName] = projectParts(
        `${segments[0] ?? ""}/${segments[1] ?? ""}`,
      );
      if (segments.length === 2) {
        res.writeHead(307, {
          location: `/p/${database}/${branchName}/${url.search}`,
        });
        return res.end();
      }
      rest = `/${segments.slice(2).join("/")}`;
      if (!rest.startsWith("/api/")) {
        if (req.method !== "GET")
          return send(res, 405, { error: "GET required" });
        return serveWeb(res, this.o.webDir, rest);
      }
    }
    if (!allowedOrigin(req, this.o.publicOrigin))
      return send(res, 403, {
        error: "same-origin requests from the configured host required",
      });
    let actor: string;
    try {
      actor = this.o.authenticate(req);
    } catch {
      return send(res, 401, { error: "sign in with an access token" });
    }
    if (!isProject)
      return this.directory(req, res, actor, path === "/api/projects");

    const route = rest.slice("/api".length);
    const method = req.method ?? "GET";
    if (route === "/branches" && method === "POST") {
      await this.workspaces.authorize(
        actor,
        database,
        branchName,
        "branch_create",
      );
      return send(
        res,
        200,
        await this.newBranch(
          database,
          branchName,
          await this.o.readBody(req),
          actor,
        ),
      );
    }
    const action = BranchService.action(method, route);
    if (!action) throw new DocError("API endpoint not found", 404);
    await this.workspaces.authorize(actor, database, branchName, action);
    const reply: Reply = await this.branch(database, branchName).handle({
      method,
      route,
      query: url.searchParams,
      headers: req.headers,
      body: () => this.o.readBody(req),
      actor,
    });
    send(res, reply.status ?? 200, reply.body, reply.headers);
  }

  private async newBranch(
    database: string,
    from: string,
    body: unknown,
    actor: string,
  ) {
    const b = body as Record<string, unknown>;
    if (
      typeof b !== "object" ||
      b === null ||
      Object.keys(b).some((k) => k !== "name") ||
      typeof b.name !== "string"
    )
      throw new DocError(
        'Failed to deserialize the JSON body: expected {"name": string}',
      );
    const out = await this.terminus
      .database(database, from)
      .createBranch(b.name);
    await this.workspaces.copyRule(database, from, b.name);
    void actor;
    return out;
  }

  /** GET /api/status and /api/projects; POST /api/projects actions. */
  private async directory(
    req: IncomingMessage,
    res: ServerResponse,
    actor: string,
    withCounts: boolean,
  ) {
    if (req.method === "GET") {
      const origin = this.o.publicOrigin ?? `http://${req.headers.host}`;
      const projects: Record<string, unknown> = {};
      const workspaces = new Map<
        string,
        Awaited<ReturnType<Workspaces["get"]>>
      >();
      for (const { database, branch } of await this.terminus.projects()) {
        if (!workspaces.has(database))
          workspaces.set(database, await this.workspaces.get(database));
        const role: Role | null = this.workspaces.role(
          actor,
          workspaces.get(database) ?? null,
          branch,
        );
        if (!role) continue;
        const project = `${database}/${branch}`;
        const entry: Record<string, unknown> = {
          running: true,
          url: `${origin}/p/${project}/`,
          role,
        };
        const counts = withCounts
          ? this.branches.get(project)?.loadedReport()
          : null;
        if (counts)
          Object.assign(entry, { nodes: counts.nodes, edges: counts.edges });
        projects[project] = entry;
      }
      const port = Number(req.socket.localPort) || null;
      return send(res, 200, {
        server: { running: true, port, url: origin },
        projects,
      });
    }
    if (req.method !== "POST" || !withCounts)
      throw new DocError("API endpoint not found", 404);
    send(res, 200, await this.manage(actor, await this.o.readBody(req)));
  }

  private async manage(actor: string, input: unknown) {
    const a = input as Record<string, unknown>;
    if (typeof a !== "object" || a === null || typeof a.action !== "string")
      throw new DocError(
        "Failed to deserialize the JSON body: missing field `action`",
      );
    const need = (...keys: string[]) => {
      for (const k of Object.keys(a))
        if (k !== "action" && !keys.includes(k))
          throw new DocError(
            `Failed to deserialize the JSON body: unknown field \`${k}\``,
          );
      for (const k of keys)
        if (typeof a[k] !== "string")
          throw new DocError(
            `Failed to deserialize the JSON body: missing field \`${k}\``,
          );
      return (k: string) => a[k] as string;
    };
    const admin = this.o.actors[actor]?.admin === true;
    switch (a.action) {
      case "init": {
        const name = need("name")("name");
        validateName(name);
        if (!admin)
          throw new DocError("database create requires one of: admin", 403);
        await this.terminus.database(name, "main").initialize(actor);
        await this.workspaces.create(name, actor);
        return { project: `${name}/main`, initialized: true };
      }
      case "remove": {
        const database = need("database")("database");
        validateName(database);
        await this.workspaces.authorize(
          actor,
          database,
          "main",
          "database_delete",
        );
        const db = this.terminus.database(database, "main");
        await db.version();
        this.forget(database);
        await db.deleteDatabase();
        await this.workspaces.drop(database);
        return { database, deleted: true };
      }
      case "new_branch": {
        const get = need("project", "name");
        const [database, branch] = projectParts(get("project"));
        await this.workspaces.authorize(
          actor,
          database,
          branch,
          "branch_create",
        );
        await this.newBranch(database, branch, { name: get("name") }, actor);
        return { project: `${database}/${get("name")}`, created: true };
      }
      case "delete_branch": {
        const project = need("project")("project");
        const [database, branch] = projectParts(project);
        if (branch === "main")
          throw new DocError(
            `cannot delete the main branch; remove ${database} to delete the entire project`,
          );
        await this.workspaces.authorize(
          actor,
          database,
          branch,
          "branch_delete",
        );
        const db = this.terminus.database(database, branch);
        await db.version();
        this.forget(project);
        await db.deleteBranch();
        await this.workspaces.dropRule(database, branch);
        return { project, deleted: true };
      }
      case "grant": {
        const get = need("project", "actor", "role");
        const [database, branch] = projectParts(get("project"));
        await this.workspaces.authorize(
          actor,
          database,
          branch,
          "member_manage",
        );
        const target = get("actor");
        const role = get("role");
        if (!Object.hasOwn(this.o.actors, target))
          throw new DocError("unknown actor");
        if (!["owner", "editor", "viewer", "none"].includes(role))
          throw new DocError("role must be owner, editor, viewer or none");
        await this.workspaces.grant(
          database,
          branch,
          target,
          role === "none" ? null : (role as "owner" | "editor" | "viewer"),
        );
        return { project: get("project"), actor: target, role };
      }
      case "set_owner": {
        const get = need("database", "owner");
        validateName(get("database"));
        if (!admin) throw new DocError("set owner requires one of: admin", 403);
        if (!Object.hasOwn(this.o.actors, get("owner")))
          throw new DocError("unknown actor");
        await this.terminus.database(get("database"), "main").version();
        await this.workspaces.setOwner(get("database"), get("owner"));
        return { database: get("database"), owner: get("owner") };
      }
      case "start":
      case "stop":
        throw new DocError(
          "branches load on demand; start and stop are not needed",
        );
      default:
        throw new DocError(`unknown action ${String(a.action)}`);
    }
  }

  shutdown() {
    for (const s of this.branches.values()) s.shutdown();
  }
}
