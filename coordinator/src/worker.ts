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
  ) {}
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
      case "provision":
        await this.lean.fact("project", {
          id: b.id,
          members: [this.leanActor, ...b.members],
        });
        await this.store.checkpoint(
          j,
          (b) => {
            b.ready = true;
            delete b.sync_error;
          },
          true,
        );
        return;
      case "resolve": {
        const resolved = await this.lean.call("/v1/goals/resolve", {
          base: b.base,
          proposition: j.payload.proposition,
          context: b.context,
          options: b.options,
        });
        const goal = identitySchema.parse(resolved.goal_key);
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
        const goal = b.goals[String(p.goal)].identity;
        const cert = await this.lean.fact("submit", {
          component: p.component,
          caller_ref: j.id,
          scope: { visibility: "project", project_id: b.id },
          request: {
            base: b.base,
            goal,
            context: b.context,
            options: b.options,
            source: p.source,
            expected_root: p.expected_root,
            mode: p.mode,
            minimum_trust: b.minimum_trust,
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
        await this.store.checkpoint(
          j,
          (b) => {
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
      default:
        throw new Fault("unknown_job");
    }
  }
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
  // fingerprint is a derived response-only field; BaseRef input rejects extras.
  const { fingerprint: _, ...baseInput } = base;
  const resolved = await lean.call("/v1/goals/resolve", {
    base: baseInput,
    proposition,
    context,
    options,
  });
  return { base: baseInput, goal: identitySchema.parse(resolved.goal_key) };
}
