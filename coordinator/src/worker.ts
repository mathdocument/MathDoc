import { z } from "zod";
import { Store, Job } from "./store.js";
import { LeanGround, RemoteError } from "./remote.js";
import { Documents } from "./documents.js";
import {
  Board,
  Fault,
  identitySchema,
  addGoal,
  ingest,
  recompute,
  requireThat,
  expire,
} from "./domain.js";
import type { Terminus } from "./docs/terminus.js";
import { baseRefSchema } from "./contracts.js";
import { DocError } from "./docs/names.js";
import type { Snapshot } from "./docs/graph.js";
import {
  conclusionProposition,
  convert,
  definitionSource,
  isStale,
  requestContract,
  submission,
  theoremBinding,
  type Conversion,
  type Environment,
  type NodeBinding,
} from "./proof.js";
import {
  buildBatch,
  batchId,
  committed,
  Conflict,
  nodeCallerRef,
  type CertificateRecord,
  type DefinitionRecord,
  type Writeback,
} from "./writeback.js";
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
  private async snapshot(b: Board): Promise<Snapshot> {
    requireThat(this.terminus && b.request, "documents_not_configured", 503);
    try {
      return await this.terminus
        .database(b.request.database, b.request.branch)
        .load();
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
  private environment(b: Board): Environment {
    return {
      base_key: Number(b.base.base_key),
      base: b.base as Environment["base"],
      context: b.context as Environment["context"],
      options: b.options as Environment["options"],
      minimum_trust: b.minimum_trust,
      environment_id: b.request!.environment_id,
      ...(b.request!.replaces_environment_id
        ? { replaces_environment_id: b.request!.replaces_environment_id }
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
            : `LeanGround rejected the node: ${e.code}`,
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
    const resolved = await this.lean.fact("resolve", {
      base: b.base,
      proposition,
      context: b.context,
      options: b.options,
      definitions,
    });
    return identitySchema.parse(resolved.goal_key);
  }
  private definitionIds(c: Conversion, id: string) {
    return (c.bindings[id].definition_nodes ?? []).flatMap(
      (d) => c.bindings[d].definition_ids ?? [],
    );
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
   * Plan §7.3: register definition nodes (dependencies first), resolve every theorem
   * node's statement, and submit nodes that carry a proof (sketches when they have
   * premise nodes). Nodes whose proof is `sorry` stay open goals with a prove task.
   */
  private async bind(b: Board) {
    const snap = await this.snapshot(b);
    const root = snap.nodes.get(b.request!.node);
    requireThat(root, "document_not_found", 404);
    const c = convert(snap.nodes, b.request!.node, b.context as never);
    const top = c.bindings[b.request!.node];
    requireThat(top, "root_has_no_lean_block");
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
    if (!goals[b.request!.node])
      return { c, error: `root_${top.reason ?? "unresolved"}` } as const;
    const certs: unknown[] = [];
    for (const id of c.theorems) {
      const cert = await this.submitNode(b, c, id, goals);
      if (cert) certs.push(cert);
    }
    const { contract, identity } = requestContract(
      this.environment(b),
      b.request!,
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
      await this.store.fail(j, code, e instanceof RemoteError && e.retryable);
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
    const check = structuredClone(b);
    ingest(check, value);
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
        const bound =
          b.request && !b.request.bound ? await this.bind(b) : undefined;
        await this.store.checkpoint(
          j,
          (b) => {
            if (bound?.error !== undefined) {
              b.nodes = bound.c.bindings;
              b.request!.error = bound.error;
              b.sync_error = bound.error;
              return;
            }
            if (bound) {
              const root = bound.goals[b.request!.node];
              b.root = String(root.key);
              addGoal(b, b.root, identitySchema.parse(root), b.title);
              for (const [id, goal] of Object.entries(bound.goals))
                addGoal(
                  b,
                  String(goal.key),
                  identitySchema.parse(goal),
                  bound.c.bindings[id].title,
                );
              b.nodes = bound.c.bindings;
              Object.assign(b.request!, {
                bound: true,
                contract: bound.contract,
                identity: bound.identity,
                stale: false,
                checked_version: bound.version,
              });
              for (const cert of bound.certs) ingest(b, cert);
            }
            b.ready = true;
            delete b.sync_error;
          },
          true,
        );
        return;
      }
      case "submit_node": {
        const id = String(j.payload.node);
        const snap = await this.snapshot(b);
        const c = convert(snap.nodes, b.request!.node, b.context as never);
        const binding = c.bindings[id];
        requireThat(binding, "not_found", 404);
        requireThat(binding.role === "theorem", "not_a_theorem_node");
        // Definitions keep their IDs from binding time; an edited definition node makes
        // the whole request stale instead (contract proof-environment.v1).
        for (const [nid, prior] of Object.entries(b.nodes ?? {}))
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
            if (goals[id])
              addGoal(
                b,
                String(goals[id].key),
                identitySchema.parse(goals[id]),
                binding.title,
              );
            b.nodes = { ...b.nodes, [id]: binding };
            if (cert) ingest(b, cert);
          },
          true,
        );
        return;
      }
      case "resolve": {
        const goal = await this.resolveGoal(
          b,
          String(j.payload.proposition),
          (j.payload.definitions as string[] | undefined) ?? [],
        );
        requireThat(goal.key === j.payload.goal, "identity_mismatch");
        await this.store.checkpoint(
          j,
          (b) => addGoal(b, goal.key, goal, String(j.payload.proposition)),
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
        await this.store.checkpoint(j, (b) => ingest(b, cert), true);
        return;
      }
      case "import": {
        const cert = await this.shared(b, String(j.payload.certificate));
        await this.store.checkpoint(j, (b) => ingest(b, cert), true);
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
        // Plan §9: a changed statement must not keep showing an old certification.
        let stale: { stale: boolean; version: string } | undefined;
        if (b.request?.bound && b.request.contract && this.terminus) {
          const snap = await this.snapshot(b);
          stale = {
            stale: isStale(b.request.contract, snap.nodes),
            version: snap.version,
          };
        }
        await this.store.checkpoint(
          j,
          (b) => {
            if (stale && b.request) {
              b.request.stale = stale.stale;
              b.request.checked_version = stale.version;
            }
            for (const id of unavailable)
              if (b.facts[id]) b.facts[id].available = false;
            for (const f of fetched) ingest(b, f);
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
              ingest(b, cert);
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
          run?.status === "certified" && run.certification_id && b.request,
          "not_certified",
        );
        const batch = batchId(run);
        const done = b.writebacks?.[batch];
        if (done?.status === "committed") {
          await this.store.checkpoint(j, () => {}, true);
          return;
        }
        const snap = await this.snapshot(b);
        const record = (w: Writeback) =>
          this.store.checkpoint(
            j,
            (b) => {
              b.writebacks = { ...b.writebacks, [w.batch_id]: w };
              if (w.status !== "committed") return;
              b.accepted = {
                certification_id: run.certification_id!,
                revision: w.version!,
                operation: w.batch_id,
              };
              // The batch is the request's own result: follow the written Lean text so
              // that it does not make the request stale.
              const written = new Map(
                (w.contract?.operations ?? []).map((o) => [
                  o.node_id,
                  o.source_sha256,
                ]),
              );
              for (const [id, sha] of written)
                if (b.nodes?.[id]) {
                  b.nodes[id].lean_content_sha256 = sha;
                  b.nodes[id].state = "certified";
                }
              const pr = b.request!.contract!.proof_request;
              pr.data_version = w.version!;
              for (const n of [pr.root, ...pr.dependencies])
                if (written.has(n.node_id))
                  n.lean_content_sha256 = written.get(n.node_id)!;
              b.request!.stale = false;
            },
            true,
          );
        if (committed(b, snap, batch)) {
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
        const certificates = new Map<string, CertificateRecord>();
        for (const step of run.route.steps) {
          const cert = await this.shared(b, step.certificate_id, true);
          certificates.set(step.certificate_id, cert as CertificateRecord);
        }
        const definitions = new Map<string, DefinitionRecord>();
        const known = new Set(
          Object.values(b.nodes ?? {}).flatMap((n) => n.definition_ids ?? []),
        );
        const pending = [...certificates.values()].flatMap(
          (c) => c.definitions ?? [],
        );
        while (pending.length) {
          const id = pending.pop()!;
          if (known.has(id) || definitions.has(id)) continue;
          const d = await this.lean.fact("read", {
            kind: "definition",
            id,
            source: true,
          });
          definitions.set(id, d as DefinitionRecord);
          pending.push(...((d.deps as string[] | undefined) ?? []));
        }
        let built;
        try {
          built = buildBatch({
            board: b,
            run,
            snapshot: snap,
            certificates,
            definitions,
          });
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
            b,
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
  // The identity-bound resolve (plan §12); without definitions it yields the same
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
  b: Board,
  snap: Snapshot,
  changes: import("./docs/model.js").Node[],
  batch: string,
  actor: string,
) {
  return terminus
    .database(b.request!.database, b.request!.branch)
    .putBundle(
      changes,
      null,
      null,
      snap.version,
      `Coordination writeback ${batch}`,
      actor,
    );
}
