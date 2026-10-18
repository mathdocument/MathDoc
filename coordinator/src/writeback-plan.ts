// What accepting a run writes (docs/mathdoc.md §4.9). The writeback job commits the batch;
// the review view previews the same batch without committing it, so the preview shows exactly
// what the job will write while the document stays as it was read.
import {
  attached,
  checkFact,
  requireThat,
  type Board,
  type Run,
} from "./domain.js";
import type { Snapshot } from "./docs/graph.js";
import type { Node } from "./docs/model.js";
import type { NodeBinding } from "./proof.js";
import type { LeanGround } from "./remote.js";
import {
  buildBatch,
  Conflict,
  type CertificateRecord,
  type DefinitionRecord,
} from "./writeback.js";
import {
  buildModuleBatch,
  routeSharedModules,
  type ModuleCertificateRecord,
  type SharedRecord,
} from "./modules.js";
import { goalName } from "./goal-view.js";

/** The route's certificate sources and the definitions they need that no node holds yet. */
export async function batchInputs(
  lean: LeanGround,
  b: Board,
  run: Run,
  nodes: Record<string, NodeBinding>,
) {
  const certificates = new Map<string, CertificateRecord>();
  for (const step of run.route.steps) {
    const cert = await lean.fact("read", {
      kind: "certificate",
      id: step.certificate_id,
      source: true,
    });
    checkFact(b, cert);
    certificates.set(step.certificate_id, cert as CertificateRecord);
  }
  const definitions = new Map<string, DefinitionRecord>();
  const known = new Set(
    Object.values(nodes).flatMap((n) => n.definition_ids ?? []),
  );
  const pending = [...certificates.values()].flatMap(
    (c) => c.definitions ?? [],
  );
  while (pending.length) {
    const id = pending.pop()!;
    if (known.has(id) || definitions.has(id)) continue;
    const d = await lean.fact("read", { kind: "definition", id, source: true });
    definitions.set(id, d as DefinitionRecord);
    pending.push(...((d.deps as string[] | undefined) ?? []));
  }
  return { certificates, definitions };
}

/** A module route's certificates with their files, and the shared modules no document node holds. */
export async function moduleBatchInputs(lean: LeanGround, b: Board, run: Run) {
  const certificates = new Map<string, ModuleCertificateRecord>();
  for (const step of run.route.steps) {
    const cert = await lean.fact("read", {
      kind: "certificate",
      id: step.certificate_id,
      source: true,
    });
    checkFact(b, cert);
    certificates.set(step.certificate_id, cert as ModuleCertificateRecord);
  }
  const shared = new Map<string, SharedRecord>();
  for (const id of routeSharedModules(b, certificates))
    shared.set(
      id,
      (await lean.fact("read", {
        kind: "module",
        id,
        source: true,
      })) as SharedRecord,
    );
  return { certificates, shared };
}

const leanText = (n: Node | undefined) =>
  n?.blocks.find((b) => b.srctype === "lean")?.content ?? null;

/** One change accepting would make, with the node's Lean text before and after. */
export interface PlannedChange {
  kind: "update_lean_block" | "create_goal_node" | "create_definition_node";
  node: string;
  title: string;
  before: string | null;
  after: string;
}

/**
 * The batch accepting this certified run would write into its branch, read against the
 * branch as it is now, and the assembled source LeanGround certified. Nothing is written.
 */
export async function previewWriteback(
  lean: LeanGround,
  load: (database: string, branch: string) => Promise<Snapshot>,
  b: Board,
  runId: string,
) {
  const run = Object.hasOwn(b.runs, runId) ? b.runs[runId] : undefined;
  requireThat(run, "not_found", 404);
  requireThat(
    run.status === "certified" && run.certification_id && run.branch,
    "not_certified",
    409,
  );
  const branch = attached(b, run.branch);
  const final = await lean.fact("read", {
    kind: "certificate",
    id: run.certification_id,
    source: true,
  });
  checkFact(b, final);
  const assembled = typeof final.source === "string" ? final.source : null;
  const snap = await load(branch.request.database, branch.request.branch);
  try {
    const built =
      b.mode === "modules"
        ? buildModuleBatch({
            board: b,
            request: branch.request,
            nodes: branch.nodes,
            run,
            snapshot: snap,
            ...(await moduleBatchInputs(lean, b, run)),
            label: (goal) => goalName(b, goal),
          })
        : buildBatch({
            request: branch.request,
            nodes: branch.nodes,
            run,
            snapshot: snap,
            ...(await batchInputs(lean, b, run, branch.nodes)),
          });
    const changed = new Map(built.changes.map((n) => [n.fnode, n]));
    const changes: PlannedChange[] = (
      built.writeback.contract?.operations ?? []
    ).map((o) => ({
      kind: o.kind,
      node: o.node_id,
      title:
        changed.get(o.node_id)?.title ??
        snap.nodes.get(o.node_id)?.title ??
        o.node_id,
      before: leanText(snap.nodes.get(o.node_id)),
      after: leanText(changed.get(o.node_id)) ?? "",
    }));
    return {
      status: "ready" as const,
      assembled,
      version: snap.version,
      changes,
    };
  } catch (e) {
    if (!(e instanceof Conflict)) throw e;
    return {
      status: "conflict" as const,
      assembled,
      version: snap.version,
      reason: e.reason,
      details: e.details,
    };
  }
}
