import { z } from "zod";
import { Store, Job } from "./store.js";
import { clip, LeanGround, RemoteError } from "./remote.js";
import { Documents } from "./documents.js";
import {
  Board,
  Fault,
  identitySchema,
  addGoal,
  attached,
  derivePicks,
  checkFact,
  ingest,
  recompute,
  refute,
  requireThat,
  expire,
  stateGoal,
  type Declaration,
  type Identity,
} from "./domain.js";
import type { Terminus } from "./docs/terminus.js";
import { baseRefSchema } from "./contracts.js";
import { proposition, scan } from "./lean-source.js";
import { DocError } from "./docs/names.js";
import type { Snapshot } from "./docs/graph.js";
import { batchInputs, moduleBatchInputs } from "./writeback-plan.js";
import {
  conclusionProposition,
  convert,
  definitionSource,
  digestText,
  isStale,
  requestContract,
  submission,
  theoremBinding,
  type Conversion,
  type Environment,
  type NodeBinding,
  type ProofRequest,
} from "./proof.js";
import {
  buildBatch,
  batchId,
  committed,
  Conflict,
  nodeCallerRef,
  type Writeback,
} from "./writeback.js";
import {
  addSharedModule,
  buildModuleBatch,
  leanBlock,
  moduleStatement,
  negatedDeclaration,
  negationOf,
  projectFiles,
  statementOf,
  userName,
} from "./modules.js";
import { goalName } from "./goal-view.js";
import type { ModuleStatement, SharedModule } from "./domain.js";

/** What converting a branch found (§4.7; module projects: docs/mathdoc.md §4.13). */
interface Bound {
  c: Pick<Conversion, "bindings"> & Partial<Conversion>;
  goals?: Record<string, Record<string, unknown>>;
  certs?: unknown[];
  contract?: ReturnType<typeof requestContract>["contract"];
  identity?: string;
  version?: string;
  error: string | undefined;
  /** Module projects: how the root goal is stated, and the shared module registered for it. */
  module?: {
    statement: ModuleStatement;
    shared?: SharedModule;
    declaration?: Declaration;
  };
}
const eventsSchema = z.object({
  cursor: z.number().int().nonnegative(),
  has_more: z.boolean(),
  events: z.array(
    z.object({
      sequence: z.number().int().nonnegative(),
      kind: z.string(),
      fact_id: z.string(),
    }),
  ),
});
export class Worker {
  constructor(
    private store: Store,
    private lean: LeanGround,
    private docs: Documents,
    private leanActor: string,
    private terminus: Terminus | null = null,
  ) {}
  private async snapshot(request: ProofRequest): Promise<Snapshot> {
    return this.documents(request, (d) => d.load());
  }
  /** The branch's TerminusDB data version, without loading its documents. */
  private async version(request: ProofRequest): Promise<string> {
    return this.documents(request, (d) => d.version());
  }
  private async documents<T>(
    request: ProofRequest,
    fn: (d: ReturnType<Terminus["database"]>) => Promise<T>,
  ): Promise<T> {
    requireThat(this.terminus, "documents_not_configured", 503);
    try {
      return await fn(this.terminus.database(request.database, request.branch));
    } catch (e) {
      if (e instanceof DocError)
        throw new RemoteError(
          e.status === 404 ? "document_not_found" : "documents_unavailable",
          e.status >= 500 || e.status === 409,
          e.status,
        );
      throw e;
    }
  }
  private environment(b: Board, request: ProofRequest): Environment {
    return {
      base_key: Number(b.base.base_key),
      base: b.base as Environment["base"],
      context: b.context as Environment["context"],
      options: b.options as Environment["options"],
      minimum_trust: b.minimum_trust,
      environment_id: request.environment_id,
      ...(request.replaces_environment_id
        ? { replaces_environment_id: request.replaces_environment_id }
        : {}),
      updated_by: b.owner,
    };
  }
  private scope(b: Board) {
    return { visibility: "project", project_id: b.id };
  }
  /** LeanGround rejections of one node are recorded on it; outages retry the job. */
  private async attempt<T>(
    binding: NodeBinding,
    fn: () => Promise<T>,
  ): Promise<T | undefined> {
    try {
      return await fn();
    } catch (e) {
      if (e instanceof RemoteError && !e.retryable) {
        const taken = ["name_taken", "definition_name_conflict"].includes(
          e.code,
        );
        Object.assign(binding, {
          state: "rejected",
          reason: taken ? "definition_name_taken" : e.code,
          details: taken
            ? "a definition with this name and different content is already registered; rename it on a review branch"
            : (e.details ?? `LeanGround rejected the node: ${e.code}`),
        });
        return undefined;
      }
      throw e;
    }
  }
  private async resolveGoal(
    b: Board,
    proposition: string,
    definitions: string[],
  ) {
    return (await this.resolveStatement(b, proposition, definitions)).identity;
  }
  /**
   * Resolve a proposition; `definitions` are the IDs LeanGround reports the statement to
   * reference. Only such IDs are recorded on goals: goal drafts return their source, so IDs
   * taken from a request could disclose another project's definitions.
   */
  private async resolveStatement(
    b: Board,
    proposition: string,
    definitions: string[],
  ) {
    const resolved = await this.lean.fact("resolve", {
      base: b.base,
      proposition,
      context: b.context,
      options: b.options,
      definitions,
    });
    return {
      identity: identitySchema.parse(resolved.goal_key),
      definitions: z.array(z.string()).catch([]).parse(resolved.definitions),
    };
  }
  private definitionIds(c: Conversion, id: string) {
    return (c.bindings[id].definition_nodes ?? []).flatMap(
      (d) => c.bindings[d].definition_ids ?? [],
    );
  }
  /** How a converted theorem node states its goal (recorded on the board for goal drafts). */
  private nodeStatement(c: Conversion, id: string) {
    const d = c.converted.get(id)!.decl!;
    return {
      proposition: conclusionProposition(c, id),
      definitions: this.definitionIds(c, id),
      declaration: { name: d.name, binders: d.binders ?? "", type: d.type! },
    };
  }
  /**
   * docs/mathdoc.md §2.7.2: state the premises of a submitted sketch. LeanGround binds each
   * placeholder name to its GoalKey but returns only the key, so a premise whose identity the
   * board does not know is resolved from its placeholder declaration with the certificate's
   * definitions. A different key is recorded on the job, never guessed; an outage retries.
   */
  private async statePremises(
    b: Board,
    cert: Record<string, any>,
    source: string,
  ) {
    const bindings = z
      .array(z.object({ name: z.string(), role: z.string(), goal: z.string() }))
      .catch([])
      .parse(cert.bindings);
    // A certificate lists the definitions it uses only when there are some.
    const definitions = z
      .array(z.string())
      .catch([])
      .parse(cert.definitions ?? []);
    const decls = scan(source).decls;
    const out: {
      goal: string;
      name: string | null;
      new: boolean;
      statement: string;
      identity?: Identity;
      declaration?: Declaration;
      definitions?: string[];
    }[] = [];
    for (const key of new Set(z.array(z.string()).parse(cert.premises))) {
      const name =
        bindings.find((x) => x.role === "premise" && x.goal === key)?.name ??
        null;
      const entry: (typeof out)[number] = {
        goal: key,
        name,
        new: !Object.hasOwn(b.goals, key),
        statement: "placeholder_not_found",
      };
      out.push(entry);
      const d = decls.find(
        (x) => x.name === name && (x.kind === "theorem" || x.kind === "lemma"),
      );
      if (!d?.type) continue;
      entry.declaration = {
        name: d.name,
        binders: d.binders ?? "",
        type: d.type,
      };
      if (b.goals[key]?.identity) {
        entry.statement = "known";
        continue;
      }
      try {
        const r = await this.resolveStatement(b, proposition(d), definitions);
        if (r.identity.key === key) {
          entry.identity = r.identity;
          entry.definitions = r.definitions;
          entry.statement = "resolved";
        } else entry.statement = "identity_mismatch";
      } catch (e) {
        if (!(e instanceof RemoteError) || e.retryable) throw e;
        entry.statement = e.code;
      }
    }
    return { premises: out, definitions };
  }
  /** Submit one converted theorem node; returns the certificate or undefined. */
  private async submitNode(
    b: Board,
    c: Conversion,
    id: string,
    goals: Record<string, Record<string, unknown>>,
  ) {
    const binding = c.bindings[id];
    if (binding.state !== "pending") return undefined;
    const premises = binding.premises ?? [];
    const missing = premises.find((p) => !goals[p]);
    if (missing) {
      Object.assign(binding, {
        state: "rejected",
        reason: "ambiguous_binding",
        details: `premise ${c.bindings[missing].title} has no resolved statement`,
      });
      return undefined;
    }
    const definitions = this.definitionIds(c, id);
    const s = submission(c, id);
    binding.binding = theoremBinding(c, id, goals, definitions, s.source);
    const cert = await this.attempt(binding, () =>
      this.lean.fact("submit", {
        component: s.component,
        caller_ref: nodeCallerRef(id, binding.lean_content_sha256),
        scope: this.scope(b),
        request: {
          base: b.base,
          goal: goals[id],
          context: b.context,
          options: b.options,
          source: s.source,
          expected_root: s.expected_root,
          mode: s.mode,
          minimum_trust: b.minimum_trust,
          definitions,
        },
      }),
    );
    if (cert) {
      binding.state = "submitted";
      binding.certificate = String(cert.id);
      const expected = premises.map((p) => String(goals[p].key)).sort();
      const got = [...(cert.premises as string[])].sort();
      if (JSON.stringify(expected) !== JSON.stringify(got))
        binding.details = `LeanGround derived premises ${got.join(", ")} instead of ${expected.join(", ")}`;
    }
    return cert;
  }
  /**
   * docs/mathdoc.md §4.7: register definition nodes (dependencies first), resolve every theorem
   * node's statement, and submit nodes that carry a proof (sketches when they have
   * premise nodes). Nodes whose proof is `sorry` stay open goals with a prove task.
   * A branch whose root resolves to another goal than `expectedRoot` submits nothing.
   */
  private async bind(
    b: Board,
    request: ProofRequest,
    expectedRoot?: string,
  ): Promise<Bound> {
    if (b.mode === "modules") return this.bindModules(b, request, expectedRoot);
    const snap = await this.snapshot(request);
    const c = convert(snap.nodes, request.node, b.context as never);
    if (!snap.nodes.has(request.node))
      return { c, error: "root_not_on_branch" } as const;
    const top = c.bindings[request.node];
    if (!top) return { c, error: "root_has_no_lean_block" } as const;
    // A rejected root is reported with every node's own reason; nothing is guessed.
    if (top.role !== "theorem")
      return { c, error: `root_${top.reason ?? "not_a_theorem"}` } as const;
    for (const id of c.definitions) {
      const d = c.bindings[id];
      if (d.state !== "pending") continue;
      const deps = snap.nodes
        .get(id)!
        .depens.filter((x) => c.bindings[x]?.role === "definition");
      if (deps.some((x) => c.bindings[x].state !== "registered")) {
        Object.assign(d, {
          state: "rejected",
          reason: "unregistered_statement_definition",
          details: "a definition it uses was not registered",
        });
        continue;
      }
      const out = await this.attempt(d, () =>
        this.lean.fact("define", {
          scope: this.scope(b),
          request: {
            base: b.base,
            context: b.context,
            options: b.options,
            source: definitionSource(c, id),
            definitions: deps.flatMap((x) => c.bindings[x].definition_ids!),
          },
        }),
      );
      if (out) {
        d.definition_ids = z
          .array(z.object({ id: z.string() }).passthrough())
          .parse(out.definitions)
          .map((x) => x.id);
        d.state = "registered";
      }
    }
    const goals: Record<string, Record<string, unknown>> = {};
    for (const id of c.theorems) {
      const t = c.bindings[id];
      if (t.state === "rejected") continue;
      if (
        (t.definition_nodes ?? []).some(
          (x) => c.bindings[x].state !== "registered",
        )
      ) {
        Object.assign(t, {
          state: "rejected",
          reason: "unregistered_statement_definition",
          details: "a definition its statement may use was not registered",
        });
        continue;
      }
      const goal = await this.attempt(t, () =>
        this.resolveGoal(
          b,
          conclusionProposition(c, id),
          this.definitionIds(c, id),
        ),
      );
      if (goal) {
        goals[id] = goal;
        t.goal = goal.key;
      }
    }
    if (!goals[request.node])
      return { c, error: `root_${top.reason ?? "unresolved"}` } as const;
    // Every branch of a project proves the same theorem.
    if (expectedRoot && String(goals[request.node].key) !== expectedRoot)
      return { c, error: "root_statement_differs" } as const;
    const certs: unknown[] = [];
    for (const id of c.theorems) {
      const cert = await this.submitNode(b, c, id, goals);
      if (cert) certs.push(cert);
    }
    const { contract, identity } = requestContract(
      this.environment(b, request),
      request,
      snap.nodes,
      c,
      snap.version,
    );
    return {
      c,
      goals,
      certs,
      contract,
      identity,
      version: snap.version,
      error: undefined,
    };
  }
  /**
   * A module project's branch (docs/mathdoc.md §4.13): the root node's file is the
   * statement, and the Lean files it reaches are registered together as one shared module.
   * Nothing else is submitted: proofs come from `mdc prove` / `mdc split`.
   */
  private async bindModules(
    b: Board,
    request: ProofRequest,
    expectedRoot?: string,
  ): Promise<Bound> {
    const snap = await this.snapshot(request);
    const bindings: Record<string, NodeBinding> = {};
    const c = { bindings };
    if (!snap.nodes.has(request.node))
      return { c, error: "root_not_on_branch" };
    if (!leanBlock(snap.nodes.get(request.node)!))
      return { c, error: "root_has_no_lean_block" };
    const files = projectFiles(snap.nodes, request.node);
    const bind = (id: string, role: NodeBinding["role"]) => {
      const n = snap.nodes.get(id)!;
      bindings[id] = {
        node: id,
        title: n.title,
        role,
        lean_content_sha256: digestText(leanBlock(n)!.content),
        state: "pending",
      };
      return bindings[id];
    };
    for (const [id, p] of files.problems)
      Object.assign(bind(id, id === request.node ? "theorem" : "definition"), {
        state: "rejected",
        ...p,
      });
    if (!files.root)
      return {
        c,
        error: `root_${bindings[request.node]?.reason ?? "unresolved"}`,
      };
    const top = bind(request.node, "theorem");
    top.definition_nodes = files.shared.map((f) => f.node);
    const blocked = [...files.problems.keys()].filter(
      (x) => x !== request.node,
    );
    if (blocked.length) {
      Object.assign(top, {
        state: "rejected",
        reason: "unregistered_statement_definition",
        details: `node ${bindings[blocked[0]].title} cannot be a module`,
      });
      return { c, error: "root_unregistered_statement_definition" };
    }
    let shared: SharedModule | undefined;
    if (files.shared.length) {
      const nodes = files.shared.map((f) => bind(f.node, "definition"));
      const out = await this.attempt(nodes[0], () =>
        this.lean.modules("shared", {
          base: b.base,
          files: Object.fromEntries(
            files.shared.map((f) => [f.path, f.content]),
          ),
          scope: this.scope(b),
        }),
      );
      if (!out) {
        // One registration for all of them: each node shows why it failed.
        for (const n of nodes.slice(1))
          Object.assign(n, {
            state: "rejected",
            reason: nodes[0].reason,
            details: nodes[0].details,
          });
        Object.assign(top, {
          state: "rejected",
          reason: "unregistered_statement_definition",
          details: "the files it depends on were not registered",
        });
        return { c, error: "root_unregistered_statement_definition" };
      }
      shared = {
        id: z.string().min(1).parse(out.id),
        modules: z
          .array(z.object({ module: z.string() }))
          .parse(out.modules)
          .map((m) => m.module),
        nodes: files.shared.map((f) => f.node),
      };
      for (const n of nodes)
        Object.assign(n, { state: "registered", definition_ids: [shared.id] });
    }
    const named = leanBlock(snap.nodes.get(request.node)!)!.metadata
      .lean_conclusion;
    const resolved = await this.attempt(top, () =>
      this.lean.modules("resolve", {
        base: b.base,
        files: { [files.root!.path]: files.root!.content },
        shared: shared ? [shared.id] : [],
        ...(named ? { statement: named } : {}),
      }),
    );
    if (!resolved) return { c, error: `root_${top.reason ?? "unresolved"}` };
    const goal = identitySchema.parse(resolved.goal_key);
    const constant = z.string().min(1).parse(resolved.statement?.const);
    Object.assign(top, { goal: goal.key, conclusion: constant, state: "open" });
    if (expectedRoot && goal.key !== expectedRoot)
      return { c, error: "root_statement_differs" };
    const { contract, identity } = requestContract(
      this.environment(b, request),
      request,
      snap.nodes,
      c as Conversion,
      snap.version,
    );
    return {
      c,
      goals: { [request.node]: goal },
      certs: [],
      contract,
      identity,
      version: snap.version,
      error: undefined,
      module: {
        statement: {
          const: constant,
          shared: shared ? [shared.id] : [],
          node: request.node,
          path: files.root.path,
        },
        ...(shared ? { shared } : {}),
        declaration: statementOf(
          { [files.root.path]: files.root.content },
          constant,
        ),
      },
    };
  }
  /**
   * ¬P in a module project (docs/mathdoc.md §4.13): `theorem MDCRefute.not_X : ¬ type_of% @X`
   * appended to the file stating P, registered like any module statement. Its GoalKey is the
   * one declaration mode gives `¬ (P)`, so a proof written either way refutes P.
   */
  private async negateModule(j: Job, b: Board, target: string) {
    const g = b.goals[target];
    requireThat(g?.module && b.project, "goal_unresolved");
    const database = b.project.database;
    const stated = await moduleStatement(
      {
        board: b,
        snapshot: (branch) =>
          this.documents({ database, branch } as ProofRequest, (d) => d.load()),
        certificate: (id) => this.shared(b, id, true),
      },
      target,
    );
    const neg = negationOf(stated);
    const r = await this.lean.modules("resolve", {
      base: b.base,
      files: neg.files,
      shared: g.module.shared,
      statement: neg.const,
    });
    const identity = identitySchema.parse(r.goal_key);
    const name = `not_${g.declaration?.name ?? userName(g.module.const)}`;
    await this.store.checkpoint(
      j,
      (b) => {
        const key = identity.key;
        requireThat(key !== target, "identity_mismatch");
        addGoal(b, key, identity, name);
        const n = b.goals[key];
        n.module ??= {
          const: neg.const,
          shared: [...b.goals[target].module!.shared],
          negates: { goal: target, path: neg.path },
        };
        n.declaration ??= negatedDeclaration(
          name,
          b.goals[target].declaration,
          neg.const,
        );
        b.negations = { ...b.negations, [key]: target };
        // The negation may already be proved.
        const proof = Object.values(b.facts).find(
          (f) => f.goal === key && !f.premises.length && f.available,
        );
        if (proof) refute(b, target, proof.id, Date.now());
      },
      true,
      { goal: target, negation: identity.key, name },
    );
  }
  /** Record a branch conversion on the board (inside a checkpoint). */
  private applyBind(b: Board, name: string, bound: Bound, actor: string) {
    const branch = attached(b, name);
    branch.nodes = bound.c.bindings;
    if (bound.error !== undefined || !bound.goals) {
      Object.assign(branch.request, { bound: false, error: bound.error });
      return;
    }
    const root = identitySchema.parse(bound.goals[branch.request.node]);
    // The first attached branch fixes the project's root goal.
    if (!b.root) {
      b.root = root.key;
      addGoal(b, b.root, root, b.title);
    }
    requireThat(b.root === root.key, "root_statement_differs");
    for (const [id, goal] of Object.entries(bound.goals)) {
      addGoal(
        b,
        String(goal.key),
        identitySchema.parse(goal),
        bound.c.bindings[id].title,
      );
      if (bound.module) {
        const g = b.goals[String(goal.key)];
        g.module ??= bound.module.statement;
        if (bound.module.declaration)
          g.declaration ??= bound.module.declaration;
      } else
        stateGoal(
          b,
          String(goal.key),
          this.nodeStatement(bound.c as Conversion, id),
        );
    }
    if (bound.module?.shared) addSharedModule(b, bound.module.shared);
    Object.assign(branch.request, {
      bound: true,
      contract: bound.contract,
      identity: bound.identity,
      stale: false,
      checked_version: bound.version,
    });
    delete branch.request.error;
    // The branch's editor wrote these decompositions into the document (§2.7.3).
    for (const cert of bound.certs ?? [])
      ingest(b, cert, { source: "document", actor, at: Date.now() });
    derivePicks(b, branch);
  }
  async tick() {
    await this.store.scheduleSync();
    const j = await this.store.claim();
    if (!j) return false;
    const timer = setInterval(
      () => void this.store.renew(j).catch(() => {}),
      20000,
    );
    timer.unref();
    try {
      await this.run(j);
    } catch (e) {
      const code =
        e instanceof Fault
          ? e.code
          : e instanceof z.ZodError
            ? "invalid_upstream_contract"
            : "worker_error";
      await this.store.fail(
        j,
        code,
        e instanceof RemoteError && e.retryable,
        e instanceof RemoteError ? e.details : undefined,
      );
    } finally {
      clearInterval(timer);
    }
    return true;
  }
  private async shared(b: Board, id: string, source = false) {
    const value = await this.lean.fact("read", {
      kind: "certificate",
      id,
      source,
    });
    checkFact(b, value);
    return value;
  }
  private async run(j: Job) {
    let b = await this.store.get(j.project, j.actor);
    switch (j.kind) {
      case "provision": {
        await this.lean.fact("project", {
          id: b.id,
          members: [this.leanActor, ...b.members],
        });
        // Branches submitted before the board was ready (at least the creating one).
        const bound: [string, Awaited<ReturnType<Worker["bind"]>>][] = [];
        let root = b.root || undefined;
        for (const [name, branch] of Object.entries(b.branches ?? {}))
          if (!branch.request.bound) {
            const r = await this.bind(b, branch.request, root);
            if (r.error === undefined && r.goals)
              root ??= String(r.goals[branch.request.node].key);
            bound.push([name, r]);
          }
        await this.store.checkpoint(
          j,
          (b) => {
            for (const [name, r] of bound) this.applyBind(b, name, r, j.actor);
            b.ready = true;
            delete b.sync_error;
          },
          true,
        );
        return;
      }
      case "attach": {
        const name = String(j.payload.branch);
        const r = await this.bind(
          b,
          attached(b, name).request,
          b.root || undefined,
        );
        await this.store.checkpoint(
          j,
          (b) => this.applyBind(b, name, r, j.actor),
          true,
        );
        return;
      }
      case "submit_node": {
        const id = String(j.payload.node);
        const name = String(j.payload.branch);
        const branch = attached(b, name);
        const snap = await this.snapshot(branch.request);
        const c = convert(snap.nodes, branch.request.node, b.context as never);
        const binding = c.bindings[id];
        requireThat(binding, "not_found", 404);
        requireThat(binding.role === "theorem", "not_a_theorem_node");
        // Definitions keep their IDs from binding time; an edited definition node makes
        // the whole request stale instead (contract proof-environment.v1).
        for (const [nid, prior] of Object.entries(branch.nodes))
          if (c.bindings[nid]?.role === "definition" && prior.definition_ids) {
            c.bindings[nid].definition_ids = prior.definition_ids;
            c.bindings[nid].state =
              c.bindings[nid].lean_content_sha256 === prior.lean_content_sha256
                ? "registered"
                : "rejected";
          }
        const goals: Record<string, Record<string, unknown>> = {};
        for (const nid of [id, ...(binding.premises ?? [])]) {
          const t = c.bindings[nid];
          if (t.state === "rejected") continue;
          const goal = await this.attempt(t, () =>
            this.resolveGoal(
              b,
              conclusionProposition(c, nid),
              this.definitionIds(c, nid),
            ),
          );
          if (goal) {
            goals[nid] = goal;
            t.goal = goal.key;
          }
        }
        if (binding.state === "open")
          Object.assign(binding, {
            state: "rejected",
            reason: "open_goal",
            details: "the proof is still sorry",
          });
        const cert = goals[id]
          ? await this.submitNode(b, c, id, goals)
          : undefined;
        await this.store.checkpoint(
          j,
          (b) => {
            if (goals[id]) {
              addGoal(
                b,
                String(goals[id].key),
                identitySchema.parse(goals[id]),
                binding.title,
              );
              stateGoal(b, String(goals[id].key), this.nodeStatement(c, id));
            }
            const branch = attached(b, name);
            branch.nodes = { ...branch.nodes, [id]: binding };
            if (cert)
              ingest(b, cert, {
                source: "document",
                actor: j.actor,
                at: Date.now(),
              });
            derivePicks(b, branch);
          },
          true,
        );
        return;
      }
      case "resolve": {
        const r = await this.resolveStatement(
          b,
          String(j.payload.proposition),
          (j.payload.definitions as string[] | undefined) ?? [],
        );
        const goal = r.identity;
        requireThat(goal.key === j.payload.goal, "identity_mismatch");
        await this.store.checkpoint(
          j,
          (b) => {
            addGoal(b, goal.key, goal, String(j.payload.proposition));
            stateGoal(b, goal.key, {
              proposition: String(j.payload.proposition),
              definitions: r.definitions,
            });
          },
          true,
        );
        return;
      }
      case "submit": {
        const p = j.payload;
        const goal =
          p.goal === undefined ? undefined : b.goals[String(p.goal)].identity;
        const newDefinitions =
          (p.new_definitions as string[] | undefined) ?? [];
        const cert = await this.lean.fact("submit", {
          component: p.component,
          caller_ref: j.id,
          scope: { visibility: "project", project_id: b.id },
          request: {
            base: b.base,
            ...(goal ? { goal } : {}),
            context: b.context,
            options: b.options,
            source: p.source,
            expected_root: p.expected_root,
            mode: p.mode,
            minimum_trust: b.minimum_trust,
            definitions: (p.definitions as string[] | undefined) ?? [],
            ...(newDefinitions.length
              ? { new_definitions: newDefinitions }
              : {}),
          },
        });
        const stated = await this.statePremises(b, cert, String(p.source));
        const description =
          typeof p.description === "string" ? p.description : undefined;
        // Written with the job when it ends; `state` is the certificate's state on the board.
        const result: Record<string, unknown> = {
          certificate: cert.id,
          goal: cert.goal,
          mode: p.mode,
          premises: stated.premises.map(
            ({ goal, name, new: fresh, statement }) => ({
              goal,
              name,
              new: fresh,
              statement,
            }),
          ),
        };
        await this.store.checkpoint(
          j,
          (b) => {
            ingest(b, cert, {
              source: "agent",
              actor: j.actor,
              at: Date.now(),
            });
            const fact = b.facts[String(cert.id)];
            result.state = fact.state;
            if (description && !fact.description)
              fact.description = description;
            for (const s of stated.premises) {
              if (s.identity) addGoal(b, s.goal, s.identity);
              if (s.declaration)
                stateGoal(b, s.goal, {
                  proposition: proposition(s.declaration),
                  definitions: s.definitions ?? stated.definitions,
                  declaration: s.declaration,
                });
            }
          },
          true,
          result,
        );
        return;
      }
      case "submit_modules": {
        // docs/mathdoc.md §4.13: a contribution's files, checked by LeanGround's
        // lean-modules component. New shared modules (`mdc split --shared`) are registered first.
        const p = j.payload as {
          goal: string;
          files: Record<string, string>;
          root: string;
          shared?: string[];
          new_shared?: Record<string, string>;
          mode: "leaf" | "sketch";
          description?: string;
        };
        const goal = b.goals[p.goal]?.identity;
        requireThat(goal, "goal_identity_unavailable");
        let shared = [...(p.shared ?? [])];
        let registered: SharedModule | undefined;
        if (p.new_shared) {
          const out = await this.lean.modules("shared", {
            base: b.base,
            files: p.new_shared,
            shared,
            scope: this.scope(b),
          });
          registered = {
            id: z.string().min(1).parse(out.id),
            modules: z
              .array(z.object({ module: z.string() }))
              .parse(out.modules)
              .map((m) => m.module),
          };
          shared = [...new Set([...shared, registered.id])].sort();
        }
        const cert = await this.lean.modules("submit", {
          base: b.base,
          goal,
          files: p.files,
          root: p.root,
          shared,
          mode: p.mode,
          scope: this.scope(b),
          caller_ref: j.id,
        });
        const identities = z
          .record(identitySchema)
          .catch({})
          .parse(cert.premise_goals ?? {});
        const placeholders = z
          .array(
            z.object({ name: z.string(), role: z.string(), goal: z.string() }),
          )
          .parse(cert.bindings)
          .filter((x) => x.role === "premise");
        const used = z
          .array(z.string())
          .catch(shared)
          .parse(cert.shared ?? shared);
        const result: Record<string, unknown> = {
          certificate: cert.id,
          goal: cert.goal,
          mode: p.mode,
          premises: placeholders.map((x) => ({
            goal: x.goal,
            name: x.name,
            new: !Object.hasOwn(b.goals, x.goal),
            statement: identities[x.goal] ? "resolved" : "identity_missing",
          })),
          ...(registered ? { shared: registered.id } : {}),
        };
        await this.store.checkpoint(
          j,
          (b) => {
            if (registered)
              addSharedModule(b, {
                ...registered,
                certificate: String(cert.id),
              });
            ingest(b, cert, {
              source: "agent",
              actor: j.actor,
              at: Date.now(),
            });
            const fact = b.facts[String(cert.id)];
            result.state = fact.state;
            if (p.description && !fact.description)
              fact.description = p.description;
            for (const x of placeholders) {
              addGoal(b, x.goal, identities[x.goal]);
              const g = b.goals[x.goal];
              g.statement ||= x.name;
              // The first decomposition stating a premise is where its drafts start.
              g.module ??= {
                const: x.name,
                shared: used,
                certificate: String(cert.id),
              };
              g.declaration ??= statementOf(p.files, x.name);
            }
          },
          true,
          result,
        );
        return;
      }
      case "negate": {
        // §2.7.5: resolve ¬(P) in the project's environment; its GoalKey is the negation's
        // identity, so any proof of the same statement, however written, refutes P.
        const target = String(j.payload.goal);
        const g = b.goals[target];
        if (b.mode === "modules") {
          await this.negateModule(j, b, target);
          return;
        }
        requireThat(g?.proposition, "goal_unresolved");
        const prop = `¬ (${g.proposition})`;
        const r = await this.resolveStatement(b, prop, g.definitions ?? []);
        const name = `not_${g.declaration?.name ?? `goal_${target.slice(0, 8)}`}`;
        await this.store.checkpoint(
          j,
          (b) => {
            const key = r.identity.key;
            requireThat(key !== target, "identity_mismatch");
            addGoal(b, key, r.identity, name);
            stateGoal(b, key, {
              proposition: prop,
              definitions: r.definitions,
              declaration: { name, binders: "", type: prop },
            });
            b.negations = { ...b.negations, [key]: target };
            // The negation may already be proved.
            const proof = Object.values(b.facts).find(
              (f) => f.goal === key && !f.premises.length && f.available,
            );
            if (proof) refute(b, target, proof.id, Date.now());
          },
          true,
          { goal: target, negation: r.identity.key, name },
        );
        return;
      }
      case "import": {
        const cert = await this.shared(b, String(j.payload.certificate));
        await this.store.checkpoint(
          j,
          (b) =>
            ingest(b, cert, {
              source: "agent",
              actor: j.actor,
              at: Date.now(),
            }),
          true,
        );
        return;
      }
      case "sync": {
        // Re-read known records so ACL revocation cannot leave permanently valid UI caches.
        const fetched: unknown[] = [];
        const unavailable: string[] = [];
        for (const id of Object.keys(b.facts)) {
          try {
            fetched.push(await this.shared(b, id));
          } catch (e) {
            if (
              (e instanceof RemoteError &&
                (e.remoteStatus === 404 || e.remoteStatus === 403)) ||
              (e instanceof Fault && e.code === "fact_not_shared")
            )
              unavailable.push(id);
            else throw e;
          }
        }
        const page = eventsSchema.parse(
          await this.lean.fact("events", { after: b.cursor, limit: 100 }),
        );
        requireThat(page.cursor >= b.cursor, "invalid_event_cursor");
        for (const ev of page.events) {
          requireThat(
            ev.sequence > b.cursor && ev.sequence <= page.cursor,
            "invalid_event_sequence",
          );
          if (ev.kind !== "certificate") continue;
          try {
            fetched.push(await this.shared(b, ev.fact_id));
          } catch (e) {
            if (
              e instanceof Fault &&
              [
                "fact_not_shared",
                "environment_mismatch",
                "trust_rejected",
              ].includes(e.code)
            )
              continue;
            if (e instanceof RemoteError && [403, 404].includes(e.remoteStatus))
              continue;
            throw e;
          }
        }
        // docs/mathdoc.md §4.9: a changed statement must not keep showing an old certification. Each
        // attached branch is checked against its own document.
        const stale: Record<string, { stale: boolean; version?: string }> = {};
        if (this.terminus)
          for (const [name, branch] of Object.entries(b.branches ?? {})) {
            const contract = branch.request.contract;
            if (!branch.request.bound || !contract) continue;
            try {
              // §2.7.5: an unchanged data version means unchanged documents.
              const version = await this.version(branch.request);
              if (version === branch.request.checked_version) continue;
              const snap = await this.snapshot(branch.request);
              stale[name] = {
                stale: isStale(contract, snap.nodes),
                version: snap.version,
              };
            } catch (e) {
              // A deleted branch can no longer be accepted; the others still sync.
              if (!(
                e instanceof RemoteError && e.code === "document_not_found"
              ))
                throw e;
              stale[name] = { stale: true };
            }
          }
        await this.store.checkpoint(
          j,
          (b) => {
            for (const [name, s] of Object.entries(stale)) {
              if (!b.branches || !Object.hasOwn(b.branches, name)) continue;
              b.branches[name].request.stale = s.stale;
              if (s.version)
                b.branches[name].request.checked_version = s.version;
            }
            for (const id of unavailable)
              if (b.facts[id]) b.facts[id].available = false;
            for (const f of fetched) ingest(b, f, { source: "sync" }, false);
            b.cursor = page.cursor;
            b.last_sync = page.has_more ? 0 : Date.now();
            delete b.sync_error;
            expire(b, Date.now());
            recompute(b);
          },
          true,
        );
        return;
      }
      case "assemble": {
        const rid = String(j.payload.run);
        const run = b.runs[rid];
        requireThat(run, "unknown_run");
        if (!run.plan_id) {
          // Recheck project-visible inputs immediately before creating a plan.
          for (const s of run.route.steps)
            await this.shared(b, s.certificate_id);
          // A paused route must not start a new plan, even if the job was queued earlier.
          requireThat(
            run.route.steps.every(
              (s) => b.facts[s.certificate_id]?.state === "active",
            ),
            "route_paused",
          );
          const plan = await this.lean.fact("plan", {
            goal: b.root,
            route: run.route,
            minimum_trust: b.minimum_trust,
          });
          const pid = z.string().min(1).parse(plan.id);
          await this.store.checkpoint(j, (b) => {
            requireThat(
              run.route.steps.every(
                (s) => b.facts[s.certificate_id]?.state === "active",
              ),
              "route_paused",
            );
            b.runs[rid].plan_id = pid;
            b.runs[rid].status = "running";
          });
          b = await this.store.get(j.project, j.actor);
        }
        const planId = b.runs[rid].plan_id!;
        // Scope widening is owned by the coordinator service account; every input
        // was checked as public or shared with precisely this project.
        await this.lean.fact("widen", {
          kind: "plan",
          id: planId,
          scope: { visibility: "project", project_id: b.id },
        });
        const result = await this.lean.fact("assemble", {
          plan_id: planId,
          component: run.component,
          attempt_id: rid,
        });
        requireThat(
          result.plan_id === planId && result.goal === b.root,
          "assembly_binding_mismatch",
        );
        if (result.status === "certified") {
          const cid = z.string().min(1).parse(result.certification_id);
          await this.lean.fact("widen", {
            kind: "certificate",
            id: cid,
            scope: { visibility: "project", project_id: b.id },
          });
          const cert = await this.shared(b, cid);
          requireThat(
            cert.goal === b.root && cert.premises.length === 0,
            "invalid_final_certificate",
          );
          await this.store.checkpoint(
            j,
            (b) => {
              ingest(b, cert, { source: "assembly" });
              b.runs[rid].status = "certified";
              b.runs[rid].certification_id = cid;
            },
            true,
          );
        } else {
          requireThat(result.status === "failed", "invalid_assembly_status");
          await this.store.checkpoint(
            j,
            (b) => {
              b.runs[rid].status = "failed";
              b.runs[rid].failure = String(
                result.failure?.reason ?? "verification_failed",
              );
              if (typeof result.failure?.message === "string")
                b.runs[rid].details = clip(result.failure.message);
              b.runs[rid].retryable = result.failure?.retryable === true;
            },
            true,
          );
        }
        return;
      }
      case "accept": {
        const run = b.runs[String(j.payload.run)];
        requireThat(
          run?.status === "certified" && run.certification_id && b.node_ref,
          "not_certified",
        );
        const cert = await this.shared(b, run.certification_id, true);
        requireThat(
          cert.goal === b.root && cert.premises.length === 0,
          "invalid_final_certificate",
        );
        const source = z.string().parse(cert.source);
        const revision = await this.docs.accept(
          b.node_ref,
          source,
          cert.id,
          j.id,
          j.actor,
        );
        await this.store.checkpoint(
          j,
          (b) => {
            b.accepted = {
              certification_id: cert.id,
              revision,
              operation: j.id,
            };
          },
          true,
        );
        return;
      }
      case "writeback": {
        const rid = String(j.payload.run);
        const run = b.runs[rid];
        requireThat(
          run?.status === "certified" && run.certification_id,
          "not_certified",
        );
        const name = run.branch!;
        const branch = attached(b, name);
        const batch = batchId(run);
        const done = branch.writebacks[batch];
        if (done?.status === "committed") {
          await this.store.checkpoint(j, () => {}, true);
          return;
        }
        const snap = await this.snapshot(branch.request);
        const record = (w: Writeback) =>
          this.store.checkpoint(
            j,
            (b) => {
              const branch = attached(b, name);
              const { bindings, ...kept } = w;
              branch.writebacks = { ...branch.writebacks, [w.batch_id]: kept };
              if (w.status !== "committed") return;
              // Nodes the batch created are converted as written (their goals are on the board).
              for (const n of bindings ?? [])
                if (!Object.hasOwn(branch.nodes, n.node))
                  branch.nodes[n.node] = n;
              branch.accepted = {
                certification_id: run.certification_id!,
                revision: w.version!,
                operation: w.batch_id,
              };
              // The batch is the branch's own result: follow the written Lean text so
              // that it does not make the branch stale.
              const written = new Map(
                (w.contract?.operations ?? []).map((o) => [
                  o.node_id,
                  o.source_sha256,
                ]),
              );
              for (const [id, sha] of written)
                if (Object.hasOwn(branch.nodes, id)) {
                  branch.nodes[id].lean_content_sha256 = sha;
                  branch.nodes[id].state = "certified";
                }
              const pr = branch.request.contract!.proof_request;
              pr.data_version = w.version!;
              for (const n of [pr.root, ...pr.dependencies])
                if (written.has(n.node_id))
                  n.lean_content_sha256 = written.get(n.node_id)!;
              branch.request.stale = false;
            },
            true,
          );
        if (committed(branch.request, snap, batch)) {
          await record({
            ...(done ?? {
              batch_id: batch,
              run: rid,
              created: [],
              updated: [],
            }),
            status: "committed",
            version: snap.version,
          });
          return;
        }
        let built;
        try {
          if (b.mode === "modules") {
            const inputs = await moduleBatchInputs(this.lean, b, run);
            built = buildModuleBatch({
              board: b,
              request: branch.request,
              nodes: branch.nodes,
              run,
              snapshot: snap,
              ...inputs,
              label: (goal) => goalName(b, goal),
            });
          } else {
            const { certificates, definitions } = await batchInputs(
              this.lean,
              b,
              run,
              branch.nodes,
            );
            built = buildBatch({
              request: branch.request,
              nodes: branch.nodes,
              run,
              snapshot: snap,
              certificates,
              definitions,
            });
          }
        } catch (e) {
          if (!(e instanceof Conflict)) throw e;
          await record({
            batch_id: batch,
            run: rid,
            status: "aborted",
            reason: e.reason,
            details: e.details,
            created: [],
            updated: [],
          });
          return;
        }
        let version: string;
        try {
          snap.validateChanges(built.changes);
          version = await putBatch(
            this.terminus!,
            branch.request,
            snap,
            built.changes,
            batch,
            j.actor,
          );
        } catch (e) {
          if (e instanceof DocError && e.status === 409)
            throw new RemoteError("document_revision_conflict", true, 409);
          if (e instanceof DocError) {
            await record({
              ...built.writeback,
              status: "aborted",
              reason: "document_rejected",
              details: e.message,
            });
            return;
          }
          throw e;
        }
        await record({ ...built.writeback, version });
        return;
      }
      default:
        throw new Fault("unknown_job");
    }
  }
}
/** The complete BaseRef of a base key (contract proof-environment.v1 §2.2). */
export async function fetchBase(lean: LeanGround, baseKey: number) {
  const response = await lean.call(`/v1/bases/${baseKey}`);
  const base = z.record(z.unknown()).parse(response.base);
  // fingerprint is a derived response-only field; BaseRef input rejects extras.
  const { fingerprint: _, ...baseInput } = base;
  return baseRefSchema.parse(baseInput);
}
export async function resolveProject(
  lean: LeanGround,
  baseKey: number,
  proposition: string,
  context: Record<string, unknown>,
  options: Record<string, unknown>,
) {
  const response = await lean.call(`/v1/bases/${baseKey}`);
  const base = z.record(z.unknown()).parse(response.base);
  const { fingerprint: _, ...baseInput } = base;
  // The identity-bound resolve (docs/mathdoc.md §4.10); without definitions it yields the same
  // GoalKey as the legacy /v1/goals/resolve.
  const resolved = await lean.fact("resolve", {
    base: baseInput,
    proposition,
    context,
    options,
    definitions: [],
  });
  return { base: baseInput, goal: identitySchema.parse(resolved.goal_key) };
}

/** One TerminusDB commit for the whole batch, guarded by the snapshot's data version. */
async function putBatch(
  terminus: Terminus,
  request: ProofRequest,
  snap: Snapshot,
  changes: import("./docs/model.js").Node[],
  batch: string,
  actor: string,
) {
  return terminus
    .database(request.database, request.branch)
    .putBundle(
      changes,
      null,
      null,
      snap.version,
      `Coordination writeback ${batch}`,
      actor,
    );
}
