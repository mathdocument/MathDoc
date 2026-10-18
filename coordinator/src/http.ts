import { createServer, IncomingMessage, ServerResponse } from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { z } from "zod";
import { Store } from "./store.js";
import { Config } from "./config.js";
import { LeanGround, RemoteError } from "./remote.js";
import { Documents, nodeSchema } from "./documents.js";
import {
  Fault,
  createSchema,
  commandSchema,
  moduleFiles,
  documentCreateSchema,
  newBoard,
  newProjectBoard,
  owner,
  requireThat,
  nodeRefSchema,
  checkFact,
  attached,
  rankGoals,
  type Board,
  type Grants,
} from "./domain.js";
import { decompositions, goalDetail, goalSummaries } from "./goal-view.js";
import { fetchBase, resolveProject } from "./worker.js";
import { goalDraft } from "./goal-draft.js";
import { moduleDraft } from "./modules.js";
import {
  actorName,
  allows,
  commandScopes,
  identify,
  requireScope,
  Tokens,
  type Identity,
} from "./identity.js";
import { DocsBackend } from "./docs/routes.js";
import { buildImport, Imports, type ImportRecord } from "./imports.js";
import { DocError } from "./docs/names.js";
import { Terminus } from "./docs/terminus.js";
import { previewWriteback } from "./writeback-plan.js";
import type { Action } from "./docs/workspace.js";
import {
  convert,
  submission,
  conclusionProposition,
  environmentInputSchema,
  Environments,
} from "./proof.js";
/** A module project's contribution may be many files: LeanGround takes request bodies up to 32 MB. */
const CONTRIBUTION_LIMIT = 32 * 1024 * 1024;
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
function key(req: IncomingMessage) {
  return z.string().min(1).max(200).parse(req.headers["idempotency-key"]);
}
/** The revision the client read; `*` (null) for commands checked against the current state. */
function revision(req: IncomingMessage) {
  const h = z
    .string()
    .regex(/^("\d+"|\*)$/)
    .parse(req.headers["if-match"]);
  return h === "*" ? null : Number(h.slice(1, -1));
}
const bindingSchema = nodeRefSchema.pick({ database: true, branch: true });
/** LeanGround's answers to a draft check and a retrieval, as far as MathDoc reads them. */
const checkResultSchema = z
  .object({
    ok: z.boolean(),
    stage: z.string().nullish(),
    diagnostics: z.array(
      z
        .object({
          severity: z.string(),
          line: z.number().nullish(),
          col: z.number().nullish(),
          text: z.string(),
        })
        .passthrough(),
    ),
    axioms: z.record(z.array(z.string())).default({}),
    trust: z.union([z.number(), z.string()]).nullish(),
    missing_imports: z.array(z.string()).default([]),
    ms: z.number().nullish(),
  })
  .passthrough();
const searchResultSchema = z.object({
  hits: z.array(
    z
      .object({
        name: z.string(),
        kind: z.string(),
        type: z.string().nullish(),
        trust: z.number().nullish(),
        evidence: z.record(z.unknown()).default({}),
      })
      .passthrough(),
  ),
});
export function server(
  cfg: Config,
  store: Store,
  lean: LeanGround,
  docs: Documents,
) {
  // `/` separates a person from a delegated token's name (docs/mathdoc.md §2.7.4).
  for (const name of Object.keys(cfg.actors))
    if (!actorName.test(name))
      throw new Error(
        `MDC_ACTORS: ${JSON.stringify(name)} is not a valid actor name`,
      );
  const tokens = new Tokens(store.pool);
  const environments = new Environments(store.pool);
  const imports = new Imports(store.pool, lean, cfg.importSources ?? {});
  const authenticate = (req: IncomingMessage): Promise<Identity> =>
    identify(req, cfg.actors, tokens);
  // Document API and the retained web/ editor (docs/mathdoc.md §4.4). Collaboration lives
  // under /api/coordination/ (§4.3); /api/me and /api/tokens are shared.
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
          authenticate,
          readBody: body,
          boards: (database, branch) => store.bound(database, branch),
          removed: async (database, actor) => {
            await store.transaction(async (c) => {
              await store.retireDatabase(c, database, actor);
              await environments.drop(database, c);
            });
            await imports.drop(database);
          },
        })
      : null;
  // Writeback previews read the branch the way the writeback job does.
  const terminus =
    cfg.terminusUrl && cfg.terminusPassword
      ? new Terminus({
          url: cfg.terminusUrl,
          user: cfg.terminusUser,
          password: cfg.terminusPassword,
        })
      : null;
  // docs/mathdoc.md §4.3: collaboration never widens document access. Each project route re-checks
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
  const canRead = async (actor: string, database: string, branch: string) => {
    try {
      await documentRole(actor, database, branch, "read");
      return true;
    } catch (e) {
      if (e instanceof DocError) return false;
      throw e;
    }
  };
  // The branch whose read access guards a board: `main` for a project board (the project
  // belongs to the whole database), the bound branch for a stage 4 proof request.
  const home = (b: Board) =>
    b.project
      ? { database: b.project.database, branch: "main" }
      : b.request
        ? { database: b.request.database, branch: b.request.branch }
        : undefined;
  // Branches the actor cannot read are left out of a project board, with their runs.
  // Boards are loaded per request, so only the maps that lose entries are copied (§2.7.5).
  const visible = async (actor: string, b: Board): Promise<Board> => {
    if (!b.project || !b.branches) return b;
    const hidden: string[] = [];
    for (const name of Object.keys(b.branches))
      if (!(await canRead(actor, b.project.database, name))) hidden.push(name);
    if (!hidden.length) return b;
    const branches = { ...b.branches };
    for (const name of hidden) delete branches[name];
    const runs = Object.fromEntries(
      Object.entries(b.runs).filter(
        ([, r]) => !r.branch || Object.hasOwn(branches, r.branch),
      ),
    );
    return { ...b, branches, runs };
  };
  const http = createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://localhost");
      if (documents && (await documents.handle(req, res, url))) return;
      const coordination = url.pathname.startsWith("/api/coordination/");
      const path = coordination
        ? `/api${url.pathname.slice("/api/coordination".length)}`
        : url.pathname;
      const method = req.method;
      requireThat(path.startsWith("/api/"), "not_found", 404);
      const who = await authenticate(req);
      const actor = who.actor;
      // Administrator rights are the person's; a delegated token also needs `maintain`.
      const admin = cfg.actors[who.principal].admin && allows(who, "maintain");
      if (path === "/api/me" && method === "GET") {
        send(res, 200, {
          actor,
          principal: who.principal,
          admin: cfg.actors[who.principal].admin,
          scopes: who.scopes,
        });
        return;
      }
      const tokenPath = /^\/api\/tokens(?:\/([A-Za-z0-9_-]{1,64}))?$/.exec(
        path,
      );
      if (tokenPath) {
        // Delegated tokens are managed with one's own token: one cannot mint another.
        requireThat(who.scopes === null, "own_token_required", 403);
        const [, name] = tokenPath;
        if (!name && method === "GET")
          send(res, 200, await tokens.list(who.principal));
        else if (!name && method === "POST")
          send(res, 201, await tokens.create(who.principal, await body(req)));
        else if (name && method === "DELETE")
          send(res, 200, await tokens.revoke(who.principal, name));
        else throw new Fault("method_not_allowed", 405);
        return;
      }
      // External proof imports (docs/external-proofs.md §3.3): read like the database's main
      // branch; created by admins, checked by the database's owners.
      if (url.pathname === "/api/imports" && method === "GET") {
        const out = [];
        for (const r of await imports.list())
          if (await canRead(actor, r.database, "main")) out.push(r);
        send(res, 200, out);
        return;
      }
      if (url.pathname === "/api/imports" && method === "POST") {
        requireThat(documents, "documents_not_configured", 503);
        requireScope(who, "maintain");
        requireThat(admin, "admin_required", 403);
        const input = z
          .object({
            database: z.string().regex(/^[A-Za-z0-9_-]{1,64}$/),
            source: z.string(),
            config: z.string(),
            commit: z.string().regex(/^[0-9a-f]{40}$/).optional(),
          })
          .strict()
          .parse(await body(req));
        const source = imports.sources[input.source];
        requireThat(source, "import_source_not_configured", 404);
        requireThat(!(await imports.get(input.database)), "import_exists", 409);
        const { record, graph } = await buildImport(
          input.database,
          input.source,
          source!,
          input.config,
          actor,
          input.commit,
        );
        try {
          await documents.createWithGraph(actor, input.database, graph);
        } catch (e) {
          if (e instanceof DocError) throw new Fault(e.message, e.status);
          throw e;
        }
        await imports.put(record);
        send(res, 200, { import: record, checks: [] });
        return;
      }
      const importPath = /^\/api\/imports\/([A-Za-z0-9_-]+)(\/checks|\/rebuild|\/source|\/source\/file)?$/.exec(url.pathname);
      if (importPath) {
        const [, database, sub] = importPath;
        await documentRole(actor, database, "main", "read");
        const record = await imports.get(database);
        requireThat(record, "not_found", 404);
        if (!sub && method === "GET") {
          send(res, 200, { import: record, checks: await imports.checks(database) });
          return;
        }
        if (sub === "/checks" && method === "POST") {
          requireScope(who, "maintain");
          await documentRole(actor, database, "main", "member_manage");
          send(res, 200, await imports.submit(database, actor));
          return;
        }
        // Rebuilds the nodes at the import's commit: ids are kept, nodes added since are written
        // in, and the checks stay (they are of the configuration, not of the nodes).
        if (sub === "/rebuild" && method === "POST") {
          requireThat(documents, "documents_not_configured", 503);
          requireScope(who, "maintain");
          requireThat(admin, "admin_required", 403);
          const source = imports.sources[record!.source];
          requireThat(source, "import_source_not_configured", 503);
          const { record: next, graph } = await buildImport(
            database,
            record!.source,
            source!,
            record!.config_name,
            actor,
            record!.commit,
          );
          let report: unknown;
          try {
            report = await documents.upsertGraph(actor, database, graph.nodes);
          } catch (e) {
            if (e instanceof DocError) throw new Fault(e.message, e.status);
            throw e;
          }
          const rebuilt: ImportRecord = {
            ...next,
            created_by: record!.created_by,
            created_at: record!.created_at,
            rebuilt_by: actor,
            rebuilt_at: new Date().toISOString(),
          };
          await imports.update(rebuilt);
          send(res, 200, { import: rebuilt, graph: report });
          return;
        }
        if (sub === "/source" && method === "GET") {
          send(res, 200, await imports.source(record!));
          return;
        }
        if (sub === "/source/file" && method === "GET") {
          send(res, 200, await imports.file(record!, url.searchParams.get("module") ?? ""));
          return;
        }
        requireThat(false, "method_not_allowed", 405);
      }
      requireThat(coordination, "not_found", 404);
      if (path === "/api/documents" && method === "GET") {
        requireThat(admin, "admin_required", 403);
        const ref = bindingSchema.parse(Object.fromEntries(url.searchParams));
        send(res, 200, await docs.load(ref));
        return;
      }
      const envPath = /^\/api\/environments\/([\w-]+)$/.exec(path);
      if (envPath) {
        const [, database] = envPath;
        if (method === "GET") {
          await documentRole(actor, database, "main", "read");
          const env = await environments.get(database);
          requireThat(env, "environment_not_configured", 404);
          send(res, 200, env);
          return;
        }
        requireThat(method === "PUT", "method_not_allowed", 405);
        // The environment decides what the project's goals mean: owners only. A change
        // starts a new project board; the old one stays readable.
        requireScope(who, "maintain");
        await documentRole(actor, database, "main", "member_manage");
        const input = environmentInputSchema.parse(await body(req));
        const base = await fetchBase(lean, input.base_key);
        send(
          res,
          200,
          await store.transaction(async (c) => {
            const env = await environments.put(database, input, base, actor, c);
            await store.supersede(c, database, env.environment_id, actor);
            return env;
          }),
        );
        return;
      }
      if (path === "/api/projects" && method === "GET") {
        const q = url.searchParams;
        const database = q.get("database");
        const branch = q.get("branch");
        if (database && branch)
          await documentRole(actor, database, branch, "read");
        const boards = await store.list(
          actor,
          database && branch
            ? { database, branch, node: q.get("node") ?? undefined }
            : undefined,
        );
        // Only boards whose home branch the actor can read, without unreadable branches.
        const out: Board[] = [];
        for (const b of boards) {
          const h = home(b);
          if (!h || (await canRead(actor, h.database, h.branch)))
            out.push(await visible(actor, b));
        }
        send(res, 200, out);
        return;
      }
      if (path === "/api/projects" && method === "POST") {
        const raw = await body(req);
        if (raw && typeof raw === "object" && "document" in raw) {
          // Submit a branch: the first submission creates the database's project board
          // rooted at `node`; later ones attach their branch to it (converted from the root).
          const input = documentCreateSchema.parse(raw);
          const { database, branch, node } = input.document;
          requireScope(who, "edit");
          await documentRole(actor, database, branch, "proof_request_create");
          const env = await environments.get(database);
          requireThat(env, "environment_not_configured");
          requireThat(documents, "documents_not_configured", 503);
          const snap = await documents.terminus
            .database(database, branch)
            .load();
          const existing = await store.project(database, env.environment_id);
          if (!existing) {
            // Creating the project fixes the database's root theorem: owners only.
            requireScope(who, "maintain");
            await documentRole(actor, database, "main", "member_manage");
            requireThat(
              input.members.every((m) => Object.hasOwn(cfg.actors, m)),
              "unknown_member",
              400,
            );
            await membersCanRead(input.members, database, "main");
            const root = snap.nodes.get(node);
            requireThat(root, "document_not_found", 404);
            requireThat(
              root.blocks.some((x) => x.srctype === "lean"),
              "root_has_no_lean_block",
            );
          } else
            requireThat(
              snap.nodes.has(existing.project!.root_node),
              "root_not_on_branch",
            );
          const out = await store.submitBranch({
            actor,
            key: key(req),
            hash: createHash("sha256")
              .update(JSON.stringify({ input, env: env.environment_id }))
              .digest("hex"),
            database,
            environmentId: env.environment_id,
            branch,
            create: () =>
              newProjectBoard(
                `mdc-${randomUUID().replaceAll("-", "")}`,
                who.principal,
                input,
                snap.nodes.get(node)!.title,
                env,
              ),
          });
          send(res, out.created ? 201 : 200, await visible(actor, out.board));
          return;
        }
        requireThat(admin, "admin_required", 403);
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
          send(res, 200, await store.get(id, actor));
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
          ...newBoard(id, who.principal, input, resolved.base, resolved.goal),
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
        /^\/api\/projects\/([\w-]+)(?:\/(commands|jobs|history|documents|document-history|branches|facts|nodes|goals|next|decompositions|check|search|runs)(?:\/([\w-]+))?(?:\/(draft|preview))?)?$/.exec(
          path,
        );
      requireThat(match, "not_found", 404);
      const [, id, operation, child, draft] = match;
      // Commands, jobs and history need only the board's meta (§2.7.5); other routes read it.
      const m = await store.meta(id, actor);
      const h = m.home;
      if (h && !(await canRead(actor, h.database, h.branch)))
        throw new Fault("not_found", 404);
      if (operation === "commands" && method === "POST") {
        const input = commandSchema.parse(await body(req, CONTRIBUTION_LIMIT));
        const grants: Grants = {};
        // §2.7.7: a branch's route is part of its content, so its editors pick too; the
        // owner picks with `maintain`, an editor (or an owner's token without it) with `edit`.
        if (
          input.type === "pick" &&
          m.database &&
          !(m.owner === who.principal && allows(who, "maintain"))
        ) {
          requireScope(who, "edit");
          await documentRole(actor, m.database, input.branch, "write");
          grants.editsBranch = true;
        } else requireScope(who, ...commandScopes[input.type]);
        if (input.type === "delete_note" || input.type === "finish")
          grants.maintains = allows(who, "maintain");
        if (input.type === "members") {
          requireThat(
            input.members.every((m) => Object.hasOwn(cfg.actors, m)),
            "unknown_member",
            400,
          );
          if (m.home)
            await membersCanRead(input.members, m.home.database, m.home.branch);
        }
        // A command naming a branch needs to read it; attaching it needs to edit it.
        if (m.database && "branch" in input && input.branch !== undefined)
          await documentRole(
            actor,
            m.database,
            input.branch,
            input.type === "attach" ? "proof_request_create" : "read",
          );
        // Accepting writes the run's route into its branch.
        if (m.database && input.type === "accept") {
          const runs = (await store.get(id, actor)).runs;
          const run = Object.hasOwn(runs, input.run)
            ? runs[input.run]
            : undefined;
          if (run?.branch)
            await documentRole(actor, m.database, run.branch, "write");
        }
        send(
          res,
          200,
          await store.apply(id, actor, revision(req), key(req), input, grants),
        );
        return;
      }
      if (operation === "jobs" && child && method === "GET") {
        send(res, 200, await store.job(id, actor, child));
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
      const b: Board = await store.get(id, actor);
      if (
        operation === "runs" &&
        child &&
        draft === "preview" &&
        method === "GET"
      ) {
        // §4.9: what accepting this assembly writes, computed as the writeback job computes it
        // and not committed; once written, the batch that was.
        const run = Object.hasOwn(b.runs, child) ? b.runs[child] : undefined;
        requireThat(run, "not_found", 404);
        const written = run.branch
          ? Object.values(b.branches?.[run.branch]?.writebacks ?? {}).find(
              (w) => w.run === child && w.status === "committed",
            )
          : undefined;
        if (written) {
          send(res, 200, { status: "written", writeback: written });
          return;
        }
        requireThat(terminus, "documents_not_configured", 503);
        const load = async (database: string, name: string) => {
          try {
            return await terminus.database(database, name).load();
          } catch (e) {
            if (e instanceof DocError)
              throw new Fault("documents_unavailable", 502);
            throw e;
          }
        };
        send(res, 200, await previewWriteback(lean, load, b, child));
        return;
      }
      // A draft check: `{source}`, one Lean file, goes to LeanGround's resident checker on the
      // project's base (§2.7.8); in a module project, `{files, …}` is compiled in its sandbox.
      const checkBody =
        operation === "check" && !child && method === "POST"
          ? await body(req, CONTRIBUTION_LIMIT)
          : undefined;
      if (
        checkBody !== undefined &&
        b.mode === "modules" &&
        !(checkBody && typeof checkBody === "object" && "source" in checkBody)
      ) {
        // docs/mathdoc.md §4.13: LeanGround compiles the files in its sandbox and,
        // given a goal, checks them as a contribution to it. Nothing is recorded. A scratch
        // file (`{source}`, e.g. `#check` lines) takes the resident checker below instead: a
        // sandboxed build imports Mathlib afresh every time (60 s against 8.5 s, 3a pilot).
        requireScope(who, "work");
        const input = z
          .object({
            files: moduleFiles,
            shared: z.array(z.string().min(1).max(200)).max(100).default([]),
            goal: z.string().min(1).max(200).optional(),
            root: z.string().min(1).max(200).optional(),
          })
          .strict()
          .parse(checkBody);
        const goal =
          input.goal === undefined
            ? undefined
            : Object.hasOwn(b.goals, input.goal)
              ? b.goals[input.goal].identity
              : undefined;
        requireThat(input.goal === undefined || goal, "unknown_goal", 404);
        requireThat(input.goal === undefined || input.root, "root_required", 400);
        send(
          res,
          200,
          await lean.modules("check", {
            base: b.base,
            files: input.files,
            shared: input.shared,
            ...(goal ? { goal, root: input.root } : {}),
          }),
        );
        return;
      }
      if (checkBody !== undefined) {
        // §2.7.8: LeanGround checks a draft on the project's base. Nothing is recorded and no
        // certificate is made, so it does not count as a submission.
        requireScope(who, "work");
        const { source } = z
          .object({ source: z.string().min(1).max(2000000) })
          .strict()
          .parse(checkBody);
        requireThat(b.base?.root_module, "environment_not_configured", 409);
        const out = checkResultSchema.parse(
          await lean.check(source, [String(b.base.root_module)]),
        );
        send(res, 200, {
          ok: out.ok,
          ...(out.stage ? { stage: out.stage } : {}),
          diagnostics: out.diagnostics,
          axioms: out.axioms,
          trust: out.trust,
          missing_imports: out.missing_imports,
          ms: out.ms,
        });
        return;
      }
      if (operation === "search" && !child && method === "GET") {
        // §2.7.8: declarations of the project's base by name, through LeanGround.
        const queries = url.searchParams.getAll("q").filter((q) => q.trim());
        requireThat(
          queries.length >= 1 &&
            queries.length <= 10 &&
            queries.every((q) => q.length <= 200),
          "invalid_query",
          400,
        );
        const kind = z
          .enum(["theorem", "definition", "all"])
          .default("theorem")
          .parse(url.searchParams.get("kind") ?? undefined);
        const limit = z.coerce
          .number()
          .int()
          .min(1)
          .max(100)
          .default(20)
          .parse(url.searchParams.get("limit") ?? undefined);
        requireThat(
          b.base?.base_key !== undefined,
          "environment_not_configured",
          409,
        );
        const out = searchResultSchema.parse(
          await lean.search(b.base, queries, kind, limit),
        );
        send(res, 200, {
          hits: out.hits.map((h) => ({
            name: h.name,
            kind: h.kind,
            type: h.type,
            trust: h.trust,
            matched_query: h.evidence.matched_query ?? null,
          })),
        });
        return;
      }
      if (operation === "nodes" && child && draft && method === "GET") {
        // Agents start from the converted node: placeholders, definitions and identity.
        requireThat(b.mode !== "modules", "not_in_module_mode");
        requireThat(documents, "documents_not_configured", 503);
        let request = b.request;
        let nodes = b.nodes ?? {};
        if (b.project) {
          const name = url.searchParams.get("branch");
          requireThat(name, "branch_required", 400);
          if (!(await canRead(actor, b.project.database, name)))
            throw new Fault("not_found", 404);
          ({ request, nodes } = attached(b, name));
        }
        requireThat(request?.bound, "no_document_binding");
        const snap = await documents.terminus
          .database(request.database, request.branch)
          .load();
        const c = convert(snap.nodes, request.node, b.context as never);
        const n = c.bindings[child];
        requireThat(n, "not_found", 404);
        requireThat(n.role === "theorem", n.reason ?? "not_a_theorem_node");
        const goalOf = (nid: string) =>
          Object.hasOwn(nodes, nid) ? nodes[nid].goal : undefined;
        send(res, 200, {
          node: child,
          goal: goalOf(child) ?? null,
          proposition: conclusionProposition(c, child),
          premises: (n.premises ?? []).map((p) => ({
            node: p,
            name: c.bindings[p].conclusion,
            goal: goalOf(p) ?? null,
          })),
          definitions: (n.definition_nodes ?? []).flatMap((d) =>
            Object.hasOwn(nodes, d) ? (nodes[d].definition_ids ?? []) : [],
          ),
          ...submission(c, child),
          state: n.state,
          reason: n.reason,
          details: n.details,
        });
        return;
      }
      if (operation === "goals" && child && draft && method === "GET") {
        // docs/mathdoc.md §2.7.2: the material of a goal file (`mdc pull`), for any goal.
        requireThat(b.project, "not_a_project_board", 400);
        const name = url.searchParams.get("branch") ?? undefined;
        if (name !== undefined) {
          bindingSchema.shape.branch.parse(name);
          if (!(await canRead(actor, b.project.database, name)))
            throw new Fault("not_found", 404);
        }
        const extra = (url.searchParams.get("with") ?? "")
          .split(",")
          .filter(Boolean);
        requireThat(extra.length <= 100, "too_many_premises", 400);
        const database = b.project.database;
        if (b.mode === "modules") {
          // docs/mathdoc.md §4.13: the goal's statement files and shared modules.
          send(
            res,
            200,
            await moduleDraft({
              board: b,
              goal: child,
              branch: name,
              snapshot: async (branch) => {
                requireThat(documents, "documents_not_configured", 503);
                return documents.terminus.database(database, branch).load();
              },
              certificate: async (cid) => {
                const f = await lean.fact("read", {
                  kind: "certificate",
                  id: cid,
                  source: true,
                });
                checkFact(b, f);
                return f;
              },
              shared: (sid) =>
                lean.fact("read", { kind: "module", id: sid, source: true }),
            }),
          );
          return;
        }
        send(
          res,
          200,
          await goalDraft({
            board: b,
            goal: child,
            branch: name,
            with: extra,
            snapshot: async (branch) => {
              requireThat(documents, "documents_not_configured", 503);
              return documents.terminus.database(database, branch).load();
            },
            definition: (id) =>
              lean.fact("read", { kind: "definition", id, source: true }),
          }),
        );
        return;
      }
      if (
        (operation === "goals" || operation === "decompositions") &&
        !draft &&
        method === "GET"
      ) {
        // §2.7.7: what `mdc goals`, `mdc goal` and `mdc review` show, from the visible board.
        requireThat(b.project, "not_a_project_board", 400);
        const v = await visible(actor, b);
        if (operation === "goals" && child) {
          requireThat(Object.hasOwn(v.goals, child), "not_found", 404);
          send(res, 200, goalDetail(v, child));
          return;
        }
        requireThat(!child, "not_found", 404);
        const q = z
          .object({
            status: z
              .enum(["insufficient", "derivable", "certified", "refuted"])
              .optional(),
            state: z
              .enum(["proposed", "active", "paused", "retired", "refuted"])
              .optional(),
            branch: bindingSchema.shape.branch.optional(),
          })
          .strict()
          .parse(Object.fromEntries(url.searchParams));
        if (q.branch !== undefined)
          requireThat(
            v.branches && Object.hasOwn(v.branches, q.branch),
            "not_found",
            404,
          );
        send(
          res,
          200,
          operation === "goals"
            ? {
                revision: b.revision,
                goals: goalSummaries(v, { status: q.status, branch: q.branch }),
              }
            : {
                revision: b.revision,
                decompositions: decompositions(v, q.state),
              },
        );
        return;
      }
      if (operation === "next" && !child && method === "GET") {
        // §2.7.5: the goals most worth working on, best first.
        const name = url.searchParams.get("branch") ?? undefined;
        if (
          name !== undefined &&
          b.project &&
          !(await canRead(actor, b.project.database, name))
        )
          throw new Fault("not_found", 404);
        const limit = z.coerce
          .number()
          .int()
          .min(1)
          .max(500)
          .default(20)
          .parse(url.searchParams.get("limit") ?? undefined);
        send(res, 200, {
          revision: b.revision,
          candidates: rankGoals(b, name)
            .slice(0, limit)
            .map((c) => ({
              ...c,
              statement: b.goals[c.goal].statement,
              negates: b.negations?.[c.goal] ?? null,
            })),
        });
        return;
      }
      if (!operation && method === "GET") {
        send(res, 200, await visible(actor, b));
        return;
      }
      if (operation === "facts" && child && method === "GET") {
        requireThat(b.facts[child], "not_found", 404);
        const f = await lean.fact("read", {
          kind: "certificate",
          id: child,
          source: true,
        });
        checkFact(b, f);
        // A reviewer reads a premise's statement together with the definitions it uses;
        // definitions registered with a sketch are no longer in its certified source.
        const ids = z.array(z.string()).catch([]).parse(f.definitions ?? []);
        const definition_sources = await Promise.all(
          ids.map(async (id) => {
            const d = await lean.fact("read", {
              kind: "definition",
              id,
              source: true,
            });
            return {
              id,
              name: String(d.name ?? ""),
              source: typeof d.source === "string" ? d.source : "",
            };
          }),
        );
        send(res, 200, { ...f, definition_sources });
        return;
      }
      requireThat(b.node_ref, "no_document_binding");
      if (operation === "documents" && method === "GET") {
        send(res, 200, await docs.load(b.node_ref));
        return;
      }
      if (operation === "documents" && method === "PUT") {
        requireScope(who, "maintain", "edit");
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
        requireScope(who, "maintain", "edit");
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
  // The LaTeX renderers are child processes: they stop with the server, or they keep it alive.
  http.on("close", () => documents?.shutdown());
  return http;
}
