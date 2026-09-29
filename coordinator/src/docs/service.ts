// Per-branch document API (src/service.rs and src/latex/api.rs at a4d61e3). Every
// handler runs under the branch lock with a snapshot matching the current database
// version, as the legacy service did; LaTeX rendering runs outside the lock.
import { DocError, digest, obj, serialize } from "./names.js";
import {
  BLOCK_TYPES,
  type LatexProject,
  leanProjectJson,
  type Node,
  moduleRoot,
  newNode,
  nodeRevision,
  nodeSource,
  parseLatexProject,
  parseLeanProject,
  parseNode,
  validateLatexProject,
  validateLeanProject,
  validateModules,
} from "./model.js";
import { type Database } from "./terminus.js";
import { Snapshot, weakComponentSizes } from "./graph.js";
import { LatexService } from "./latex.js";
import type { Action } from "./workspace.js";

export interface Reply {
  status?: number;
  body: unknown;
  headers?: Record<string, string>;
}
export interface Call {
  method: string;
  route: string; // path after /api, e.g. "/node/<id>/view"
  query: URLSearchParams;
  headers: Record<string, string | string[] | undefined>;
  body: () => Promise<unknown>;
  actor: string;
}

class Lock {
  private tail: Promise<unknown> = Promise.resolve();
  run<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.tail.then(fn);
    this.tail = next.catch(() => undefined);
    return next;
  }
}

/** Request bodies are serde(deny_unknown_fields) structs in the legacy backend. */
function fields(
  value: unknown,
  required: string[],
  optional: string[] = [],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new DocError(
      "Failed to deserialize the JSON body: expected an object",
    );
  for (const k of Object.keys(value))
    if (!required.includes(k) && !optional.includes(k))
      throw new DocError(
        `Failed to deserialize the JSON body: unknown field \`${k}\``,
      );
  for (const k of required)
    if (!(k in value))
      throw new DocError(
        `Failed to deserialize the JSON body: missing field \`${k}\``,
      );
  return value as Record<string, unknown>;
}
function str(value: unknown, name: string): string {
  if (typeof value !== "string")
    throw new DocError(
      `Failed to deserialize the JSON body: ${name} must be a string`,
    );
  return value;
}

function expected(headers: Call["headers"]): string {
  const h = headers["if-match"];
  const v = Array.isArray(h) ? h[0] : h;
  if (!v || v.length < 2 || !v.startsWith('"') || !v.endsWith('"'))
    throw new DocError("a quoted If-Match revision is required", 428);
  return v.slice(1, -1);
}
function checkRevision(headers: Call["headers"], node: Node) {
  if (expected(headers) !== nodeRevision(node))
    throw new DocError("node changed; reload and retry", 412);
}

/** The legacy wire form: `files` omitted when empty, `manifest` always present. */
const leanWire = (p: import("./model.js").LeanProject) =>
  JSON.parse(leanProjectJson(p));

const formal = (node: Node, language: string) =>
  nodeSource(node, language) !== undefined ? "unverified" : "no_code";

export class BranchService {
  private snapshot: Snapshot | null = null;
  private lock = new Lock();
  readonly latex = new LatexService();

  constructor(
    readonly db: Database,
    private author: (actor: string) => string,
  ) {}

  /** Cheap commit lookup; reload only when another writer moved the branch. */
  private async read(): Promise<Snapshot> {
    const version = await this.db.version();
    if (!this.snapshot || this.snapshot.version !== version)
      this.snapshot = await this.db.load();
    return this.snapshot;
  }
  /** Counts of the last loaded projection, for the project directory; never loads. */
  loadedReport() {
    return this.snapshot?.report() ?? null;
  }

  private async save(
    s: Snapshot,
    changes: Node[],
    message: string,
    actor: string,
  ) {
    s.validateChanges(changes);
    const version = await this.db.putBundle(
      changes,
      null,
      null,
      s.version,
      message,
      this.author(actor),
    );
    s.apply(changes, version);
  }

  private summary(s: Snapshot, n: Node) {
    return {
      fnode: n.fnode,
      title: n.title,
      broken: false,
      depth: s.depths.get(n.fnode) ?? 0,
    };
  }
  private preview(s: Snapshot, n: Node) {
    return {
      ...this.summary(s, n),
      formalization: { lean: formal(n, "lean"), rocq: formal(n, "rocq") },
    };
  }
  private detail(s: Snapshot, n: Node) {
    return {
      ...this.preview(s, n),
      revision: nodeRevision(n),
      depens: n.depens,
      blocks: n.blocks,
      module: n.module,
    };
  }
  private revisionReply(s: Snapshot, n: Node): Reply {
    return {
      body: this.detail(s, n),
      headers: { etag: `"${nodeRevision(n)}"` },
    };
  }

  /** The workspace action a route needs; null for unknown routes. */
  static action(method: string, route: string): Action | null {
    if (route === "/export" && method === "GET") return "export";
    if (route === "/import" && method === "POST") return "import";
    if (route === "/history" && method === "GET") return "history";
    if (route === "/branches" && method === "POST") return "branch_create";
    // A preview renders a draft without writing it.
    if (method === "GET" || /^\/node\/[^/]+\/latex\/preview$/.test(route))
      return "read";
    return "write";
  }

  handle(call: Call): Promise<Reply> {
    const m = (re: RegExp) => re.exec(call.route);
    const { method, route } = call;
    const latex = m(/^\/node\/([^/]+)\/latex\/(context|preview)$/);
    if (route === "/project/latex/catalog" && method === "GET")
      return this.catalog(call);
    if (latex && latex[2] === "context" && method === "GET")
      return this.latexContext(call, decodeURIComponent(latex[1]));
    if (latex && latex[2] === "preview" && method === "POST")
      return this.latexPreview(call, decodeURIComponent(latex[1]));
    return this.lock.run(() => this.locked(call));
  }

  private async locked(call: Call): Promise<Reply> {
    const { method, route, query, headers, actor } = call;
    const s = await this.read();
    const node = /^\/node\/([^/]+)(\/.*)?$/.exec(route);
    const id = node ? decodeURIComponent(node[1]) : "";
    const tail = node?.[2] ?? "";
    const q = (query.get("q") ?? "").toLowerCase();
    const limit = (fallback: number) => {
      const n = query.get("n");
      if (n === null) return fallback;
      if (!/^\d+$/.test(n))
        throw new DocError(
          "Failed to deserialize query string: invalid n",
          400,
        );
      return Math.min(Number(n), 200);
    };
    const matches = (n: Node) =>
      n.title.toLowerCase().includes(q) || n.fnode.includes(q);

    if (route === "/graph/check" && method === "GET")
      return { body: s.report() };
    if (route === "/graph/roots" && method === "GET") {
      const referenced = new Set(s.values().flatMap((n) => n.depens));
      const sizes = weakComponentSizes(s.graph(), new Set(s.ids()));
      const roots = s
        .values()
        .filter((n) => !referenced.has(n.fnode))
        .map((n) => {
          const v = this.summary(s, n);
          return {
            ...v,
            component_size: sizes.get(n.fnode) ?? 1,
            topo_depth: v.depth,
          };
        });
      // Rust sort_by_key is stable, so equal depths keep fnode order.
      roots.sort((a, b) => b.topo_depth - a.topo_depth);
      return { body: roots };
    }
    if (route === "/graph/full" && method === "GET") {
      const ids = s.ids();
      const index = new Map(ids.map((id, i) => [id, i]));
      const edges: [number, number][] = [];
      for (const n of s.values())
        for (const d of n.depens)
          if (index.has(d)) edges.push([index.get(n.fnode)!, index.get(d)!]);
      return {
        body: {
          nodes: s
            .values()
            .map((n) => ({ ...this.summary(s, n), lean: formal(n, "lean") })),
          edges,
        },
      };
    }
    if (route === "/search" && method === "GET")
      return {
        body: s
          .values()
          .filter(matches)
          .slice(0, limit(200))
          .map((n) => this.summary(s, n)),
      };
    if (route === "/resolve" && method === "GET") {
      const ref = query.get("ref");
      if (ref === null)
        throw new DocError(
          "Failed to deserialize query string: missing field `ref`",
          400,
        );
      const n = s.resolve(ref);
      return { body: { fnode: n.fnode, title: n.title } };
    }
    if (route === "/node/new" && method === "POST") {
      const body = fields(await call.body(), ["title"], ["parent_fnode"]);
      const created = newNode(str(body.title, "title"), moduleRoot(s.project));
      const changes = [created];
      if (body.parent_fnode !== undefined && body.parent_fnode !== null) {
        const parent = s.resolve(str(body.parent_fnode, "parent_fnode"));
        checkRevision(headers, parent);
        changes.push({ ...parent, depens: [...parent.depens, created.fnode] });
      }
      const last = changes[changes.length - 1];
      await this.save(s, changes, "Create node", actor);
      return this.revisionReply(s, last);
    }
    if (node && tail === "" && method === "DELETE") {
      const n = s.resolve(id);
      checkRevision(headers, n);
      const referrers = s.referrers.get(n.fnode) ?? [];
      const removed = n.depens.length + referrers.length;
      const version = await this.db.deleteNode(
        n.fnode,
        referrers,
        s.version,
        this.author(actor),
      );
      s.remove(n.fnode, version);
      return {
        body: { fnode: n.fnode, deleted: true, removed_edges: removed },
      };
    }
    if (node && tail === "/view" && method === "GET") {
      const n = s.resolve(id);
      return {
        body: {
          node: this.detail(s, n),
          referrers: (s.referrers.get(n.fnode) ?? []).map((r) =>
            this.preview(s, s.nodes.get(r)!),
          ),
          children: n.depens
            .filter((d) => s.nodes.has(d))
            .map((d) => this.preview(s, s.nodes.get(d)!)),
        },
      };
    }
    if (node && tail === "/title" && method === "PUT") {
      const body = fields(await call.body(), ["title"]);
      const n = s.resolve(id);
      checkRevision(headers, n);
      const changed = { ...n, title: str(body.title, "title") };
      await this.save(s, [changed], "Rename node", actor);
      return this.revisionReply(s, changed);
    }
    const block = /^\/block\/([^/]+)$/.exec(tail);
    if (node && block && (method === "PUT" || method === "DELETE")) {
      const language = decodeURIComponent(block[1]);
      const body =
        method === "PUT" ? fields(await call.body(), ["content"]) : null;
      const n = s.resolve(id);
      checkRevision(headers, n);
      if (!(BLOCK_TYPES as readonly string[]).includes(language))
        throw new DocError("supported types: text, lean, rocq, latex");
      let blocks: Node["blocks"];
      if (body) {
        const content = str(body.content, "content");
        blocks = n.blocks.some((b) => b.srctype === language)
          ? n.blocks.map((b) =>
              b.srctype === language ? { ...b, content } : b,
            )
          : [...n.blocks, { srctype: language, content, metadata: {} }];
      } else blocks = n.blocks.filter((b) => b.srctype !== language);
      const changed = { ...n, blocks };
      await this.save(
        s,
        [changed],
        body ? "Update source block" : "Delete source block",
        actor,
      );
      return this.revisionReply(s, changed);
    }
    if (node && tail === "/dep/add" && method === "POST") {
      const body = fields(await call.body(), ["dep_fnode"]);
      const n = s.resolve(id);
      checkRevision(headers, n);
      const dep = s.resolve(str(body.dep_fnode, "dep_fnode")).fnode;
      let changed = n;
      if (!n.depens.includes(dep)) {
        changed = { ...n, depens: [...n.depens, dep] };
        await this.save(s, [changed], "Add dependency", actor);
      }
      return this.revisionReply(s, changed);
    }
    if (node && tail === "/dep/rm" && method === "POST") {
      const body = fields(await call.body(), ["dep_fnodes"]);
      if (!Array.isArray(body.dep_fnodes))
        throw new DocError(
          "Failed to deserialize the JSON body: dep_fnodes must be an array",
        );
      const n = s.resolve(id);
      checkRevision(headers, n);
      const removed = new Set(
        body.dep_fnodes.map((r) => s.resolve(str(r, "dep_fnodes")).fnode),
      );
      const changed = { ...n, depens: n.depens.filter((d) => !removed.has(d)) };
      await this.save(s, [changed], "Remove dependencies", actor);
      return this.revisionReply(s, changed);
    }
    if (node && tail === "/dep/candidates" && method === "GET") {
      const n = s.resolve(id);
      const found = s.values().filter(matches);
      const deps = new Set(n.depens);
      const source = found.filter((x) => x.fnode === n.fnode).length;
      const existing = found.filter((x) => deps.has(x.fnode)).length;
      const available = found.length - source - existing;
      const nodes = found
        .filter((x) => x.fnode !== n.fnode && !deps.has(x.fnode))
        .slice(0, limit(50))
        .map((x) => this.summary(s, x));
      const empty = nodes.length
        ? null
        : available > 0
          ? { kind: "result_limit", available }
          : source + existing > 0
            ? {
                kind: "excluded",
                source,
                existing_dependencies: existing,
                invalid_or_duplicate: 0,
              }
            : { kind: "no_match" };
      return { body: { nodes, empty } };
    }
    if (node && tail === "/dep" && method === "GET") {
      for (const k of query.keys())
        if (k !== "mode" && k !== "depth")
          throw new DocError(
            `Failed to deserialize query string: unknown field \`${k}\``,
            400,
          );
      const mode = query.get("mode");
      const depthText = query.get("depth");
      if (
        !["show", "refs", "leaf"].includes(mode ?? "") ||
        depthText === null ||
        !/^-?\d+$/.test(depthText)
      )
        throw new DocError(
          "Failed to deserialize query string: mode and depth are required",
          400,
        );
      const maxDepth = Number(depthText);
      if (maxDepth < -1)
        throw new DocError("depth must be -1 (unlimited) or nonnegative");
      const root = s.resolve(id);
      const seen = new Set([root.fnode]);
      const queue: [string, number][] = [[root.fnode, 0]];
      const out: unknown[] = [];
      for (let i = 0; i < queue.length; i++) {
        const [cur, depth] = queue[i];
        const n = s.nodes.get(cur)!;
        if (depth > 0 && (mode !== "leaf" || n.depens.length === 0))
          out.push({ ...this.summary(s, n), depth });
        if (maxDepth >= 0 && depth >= maxDepth) continue;
        for (const next of mode === "refs"
          ? (s.referrers.get(cur) ?? [])
          : n.depens)
          if (!seen.has(next)) {
            seen.add(next);
            queue.push([next, depth + 1]);
          }
      }
      return { body: out };
    }
    if (node && tail === "/metric/ior" && method === "GET") {
      const n = s.resolve(id);
      const incoming = (s.referrers.get(n.fnode) ?? []).length;
      const outgoing = n.depens.length;
      return {
        body: {
          fnode: n.fnode,
          in_degree: incoming,
          out_degree: outgoing,
          ior: Math.log((incoming + 1) / (outgoing + 1)),
        },
      };
    }
    if (route === "/project/lean" && method === "GET")
      return { body: { revision: s.version, project: leanWire(s.project) } };
    if (route === "/project/lean" && method === "PUT") {
      const project = parseLeanProject(await call.body());
      if (expected(headers) !== s.version)
        throw new DocError("project changed; reload and retry", 412);
      validateModules(project, s.nodes.values());
      s.version = await this.db.putProject(
        project,
        s.version,
        this.author(actor),
      );
      s.project = project;
      return { body: { revision: s.version, project: leanWire(s.project) } };
    }
    if (route === "/project/latex" && method === "GET")
      return { body: { revision: s.version, project: s.latexProject } };
    if (route === "/project/latex" && method === "PUT") {
      const project = parseLatexProject(await call.body());
      const expectedRevision = expected(headers);
      validateLatexProject(project);
      if (expectedRevision !== s.version)
        throw new DocError("project changed; reload and retry", 412);
      const version = await this.db.putBundle(
        [],
        null,
        project,
        s.version,
        "Update LaTeX project",
        this.author(actor),
      );
      s.setLatexProject(project, version);
      return { body: { revision: s.version, project: s.latexProject } };
    }
    if (route === "/export" && method === "GET")
      return {
        body: {
          nodes: s.values(),
          project: leanWire(s.project),
          latex_project: s.latexProject,
        },
      };
    if (route === "/import" && method === "POST") {
      const body = fields(
        await call.body(),
        ["nodes", "project"],
        ["latex_project"],
      );
      if (s.nodes.size)
        throw new DocError("graph import requires an empty branch", 409);
      if (!Array.isArray(body.nodes))
        throw new DocError(
          "Failed to deserialize the JSON body: nodes must be an array",
        );
      const project = parseLeanProject(body.project);
      const latexProject = parseLatexProject(body.latex_project ?? {});
      const nodes = body.nodes.map(parseNode);
      // Project configuration is validated before any data is imported.
      validateLeanProject(project);
      validateLatexProject(latexProject);
      validateModules(project, nodes);
      const probe = new Snapshot(s.version, [], project, latexProject);
      probe.validateChanges(nodes);
      const version = await this.db.putBundle(
        nodes,
        project,
        latexProject,
        s.version,
        "Import graph",
        this.author(actor),
      );
      this.snapshot = new Snapshot(version, nodes, project, latexProject);
      return { body: this.snapshot.report() };
    }
    if (route === "/history" && method === "GET")
      return { body: await this.db.history() };
    throw new DocError("API endpoint not found", 404);
  }

  // ---- LaTeX (the renderer runs outside the branch lock) --------------------------

  private capture(
    s: Snapshot,
    id: string,
    source: string | null,
    kind: string,
  ) {
    const node = s.resolve(id);
    const target = {
      fnode: node.fnode,
      title: node.title,
      source: source ?? nodeSource(node, "latex") ?? "",
    };
    const dependencies = node.depens.map((d) => {
      const n = s.nodes.get(d)!;
      return {
        fnode: n.fnode,
        title: n.title,
        source: nodeSource(n, "latex") ?? "",
      };
    });
    const canon = (x: { fnode: string; title: string; source: string }) =>
      obj([
        ["fnode", x.fnode],
        ["title", x.title],
        ["source", x.source],
      ]);
    const key = digest(
      serialize([s.latexProjectKey, canon(target), dependencies.map(canon)]),
    );
    return {
      kind,
      target,
      dependencies,
      context_key: key,
      project_key: s.latexProjectKey,
    };
  }

  private async catalog(call: Call): Promise<Reply> {
    const known = call.query.get("known");
    const { project, key } = await this.lock.run(async () => {
      const s = await this.read();
      return { project: s.latexProject, key: s.latexProjectKey };
    });
    if (known === key) return { body: { unchanged: true, project_key: key } };
    const result = (await this.latex.request(project, {
      kind: "catalog",
      project_key: key,
    })) as Record<string, unknown>;
    return { body: { ...result, project_key: key } };
  }

  private async latexContext(call: Call, id: string): Promise<Reply> {
    for (const k of call.query.keys())
      if (k !== "known")
        throw new DocError(
          `Failed to deserialize query string: unknown field \`${k}\``,
          400,
        );
    const { project, input } = await this.lock.run(async () => {
      const s = await this.read();
      return {
        project: s.latexProject as LatexProject,
        input: this.capture(s, id, null, "context"),
      };
    });
    if (call.query.get("known") === input.context_key)
      return {
        body: {
          unchanged: true,
          context_key: input.context_key,
          project_key: input.project_key,
        },
      };
    const result = (await this.latex.request(project, input)) as Record<
      string,
      unknown
    >;
    return {
      body: {
        ...result,
        context_key: input.context_key,
        project_key: input.project_key,
      },
    };
  }

  private async latexPreview(call: Call, id: string): Promise<Reply> {
    const draft = fields(await call.body(), ["source"]);
    const source = str(draft.source, "source");
    if (Buffer.byteLength(source) > 2 * 1024 * 1024)
      throw new DocError("LaTeX block exceeds 2 MiB", 413);
    const { project, input } = await this.lock.run(async () => {
      const s = await this.read();
      return {
        project: s.latexProject as LatexProject,
        input: this.capture(s, id, source, "preview"),
      };
    });
    const result = (await this.latex.request(project, input)) as Record<
      string,
      unknown
    >;
    const current = await this.lock.run(async () =>
      this.capture(await this.read(), id, source, "preview"),
    );
    if (current.context_key !== input.context_key)
      throw new DocError(
        "LaTeX dependencies or project changed; retry the preview",
        409,
      );
    return {
      body: {
        ...result,
        project_key: input.project_key,
        context_key: input.context_key,
      },
    };
  }

  shutdown() {
    this.latex.shutdown();
  }
}
