// Batched writeback of a certified route into the bound document branch (plan §7.2;
// contract mathdoc.writeback-batch.v1, decision C). Every node of the batch is checked
// against the Lean text it had when it was converted; any conflict aborts the whole batch,
// so no partial "certified" state is ever written. Node IDs are derived from the batch,
// so replaying a batch never creates a node twice.
import { createHash } from "node:crypto";
import { z } from "zod";
import { writebackContractSchema } from "./contracts.js";
import { scan, withoutPlaceholders, preludeText } from "./lean-source.js";
import { digestText, type NodeBinding } from "./proof.js";
import type { Board, Run } from "./domain.js";
import {
  moduleRoot,
  nodeRevision,
  type Node,
  type Block,
} from "./docs/model.js";
import type { Snapshot } from "./docs/graph.js";

export interface Writeback {
  batch_id: string;
  run: string;
  status: "committed" | "aborted";
  version?: string;
  reason?: string;
  details?: string;
  contract?: z.infer<typeof writebackContractSchema>;
  created: string[];
  updated: string[];
}
export interface CertificateRecord {
  id: string;
  goal: string;
  premises: string[];
  caller_ref?: string | null;
  source: string;
  bindings: { name: string; role: string; goal: string }[];
  definitions?: string[];
}
export interface DefinitionRecord {
  id: string;
  name: string;
  source: string;
  deps?: string[];
}

export const batchId = (run: Run) =>
  `wb-${createHash("sha256").update(`${run.id}\0${run.certification_id}`).digest("hex").slice(0, 32)}`;

/** Deterministic UUID for a node created by a batch. */
function derivedUuid(batch: string, key: string): string {
  const h = createHash("sha256").update(`${batch}\0${key}`).digest("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-${"89ab"[parseInt(h[16], 16) % 4]}${h.slice(17, 20)}-${h.slice(20, 32)}`;
}

/** The caller_ref used for converted document nodes: node:<uuid>:<lean sha>. */
export const nodeCallerRef = (node: string, sha: string) =>
  `node:${node}:${sha}`;

export class Conflict extends Error {
  constructor(
    readonly reason: string,
    readonly details: string,
  ) {
    super(reason);
  }
}

const leanBlock = (n: Node) => n.blocks.find((b) => b.srctype === "lean");

/** Already committed: the root carries this batch's operation marker. */
export function committed(b: Board, s: Snapshot, batch: string): boolean {
  const root = s.nodes.get(b.request!.node);
  return (
    root !== undefined &&
    leanBlock(root)?.metadata.coordination_operation === batch
  );
}

/**
 * Build the batch. Throws Conflict when any bound node no longer has the Lean text it had
 * at conversion, or when writing would rename a public declaration.
 */
export function buildBatch(input: {
  board: Board;
  run: Run;
  snapshot: Snapshot;
  certificates: Map<string, CertificateRecord>;
  definitions: Map<string, DefinitionRecord>;
}): { changes: Node[]; writeback: Writeback } {
  const { board: b, run, snapshot: s, certificates, definitions } = input;
  const batch = batchId(run);
  const bindings = Object.values(b.nodes ?? {});
  const byGoal = new Map<string, NodeBinding>();
  for (const n of bindings.sort((x, y) => x.node.localeCompare(y.node)))
    if (n.role === "theorem" && n.goal && !byGoal.has(n.goal))
      byGoal.set(n.goal, n);
  const byDefinition = new Map<string, string>();
  for (const n of bindings)
    for (const id of n.definition_ids ?? []) byDefinition.set(id, n.node);

  const changes = new Map<string, Node>();
  const created: string[] = [];
  const operations: z.infer<typeof writebackContractSchema>["operations"] = [];
  const root = moduleRoot(s.project);
  const newNode = (key: string, title: string): Node => {
    const fnode = derivedUuid(batch, key);
    return {
      fnode,
      title,
      module: `${root}.N_${fnode.replaceAll("-", "")}`,
      depens: [],
      blocks: [],
    };
  };
  const setLean = (n: Node, content: string, metadata: Block["metadata"]) => {
    const prior = leanBlock(n);
    const block = {
      srctype: "lean",
      content,
      metadata: { ...(prior?.metadata ?? {}), ...metadata },
    };
    n.blocks = prior
      ? n.blocks.map((x) => (x.srctype === "lean" ? block : x))
      : [...n.blocks, block];
  };

  // Definitions the route uses that no definition node carries become new nodes.
  const definitionNode = (id: string): string => {
    const existing = byDefinition.get(id);
    if (existing) return existing;
    const d = definitions.get(id);
    if (!d)
      throw new Conflict("unknown_definition", `definition ${id} was not read`);
    const n = newNode(`definition:${id}`, d.name);
    byDefinition.set(id, n.fnode);
    setLean(n, d.source.endsWith("\n") ? d.source : `${d.source}\n`, {
      definition_id: id,
      coordination_operation: batch,
    });
    n.depens = (d.deps ?? []).map(definitionNode);
    changes.set(n.fnode, n);
    created.push(n.fnode);
    operations.push({
      operation_id: `${batch}:${n.fnode}`,
      kind: "create_definition_node",
      node_id: n.fnode,
      source_sha256: digestText(leanBlock(n)!.content),
      definition_id: id,
      preserve_public_declaration_name: true,
    });
    return n.fnode;
  };

  // First pass: decide the target node of every step so that providers can be linked.
  const target = new Map<string, Node>();
  for (const step of run.route.steps) {
    const cert = certificates.get(step.certificate_id);
    if (!cert) throw new Conflict("unknown_certificate", step.certificate_id);
    const bound = byGoal.get(cert.goal);
    if (bound) {
      const current = s.nodes.get(bound.node);
      if (!current || !leanBlock(current))
        throw new Conflict(
          "document_conflict",
          `node ${bound.title} was deleted or lost its Lean block`,
        );
      if (digestText(leanBlock(current)!.content) !== bound.lean_content_sha256)
        throw new Conflict(
          "document_conflict",
          `node ${bound.title} changed after it was converted; its statement may no longer match`,
        );
      target.set(step.certificate_id, structuredClone(current));
    } else {
      const conclusion = cert.bindings.find((x) => x.role === "conclusion");
      const n = newNode(`goal:${cert.goal}`, conclusion?.name ?? cert.goal);
      target.set(step.certificate_id, n);
      created.push(n.fnode);
    }
  }
  const provider = new Map<string, string>(); // goal -> node
  for (const step of run.route.steps)
    provider.set(
      certificates.get(step.certificate_id)!.goal,
      target.get(step.certificate_id)!.fnode,
    );

  for (const step of run.route.steps) {
    const cert = certificates.get(step.certificate_id)!;
    const n = target.get(step.certificate_id)!;
    const isNew = !s.nodes.has(n.fnode);
    const bound = byGoal.get(cert.goal);
    const before = isNew ? null : structuredClone(n);
    const declarations = withoutPlaceholders(cert.source);
    let content: string;
    if (
      bound &&
      cert.caller_ref === nodeCallerRef(bound.node, bound.lean_content_sha256)
    )
      content = leanBlock(n)!.content; // converted from this very text
    else
      content = bound
        ? preludeText(scan(leanBlock(n)!.content)) + declarations
        : declarations;
    if (bound) {
      const names = scan(content).decls.map((d) => d.name);
      if (!names.includes(bound.conclusion!))
        throw new Conflict(
          "public_declaration_renamed",
          `the certified source for ${bound.title} does not declare ${bound.conclusion}`,
        );
    }
    const depens = new Set(n.depens);
    for (const premise of cert.premises) {
      const p = provider.get(premise);
      if (p && p !== n.fnode) depens.add(p);
    }
    for (const id of cert.definitions ?? []) depens.add(definitionNode(id));
    n.depens = [...depens];
    const certification =
      n.fnode === b.request!.node ? run.certification_id! : cert.id;
    setLean(n, content, {
      certification_id: certification,
      certified_lean_sha256: digestText(content),
      coordination_operation: batch,
    });
    changes.set(n.fnode, n);
    operations.push({
      operation_id: `${batch}:${n.fnode}`,
      kind: isNew ? "create_goal_node" : "update_lean_block",
      node_id: n.fnode,
      ...(before ? { expected_node_revision: nodeRevision(before) } : {}),
      source_sha256: digestText(content),
      certification_id: certification,
      preserve_public_declaration_name: true,
    });
  }
  if (!changes.has(b.request!.node))
    throw new Conflict(
      "root_not_in_route",
      "the certified route does not end at the bound root node",
    );
  const contract = writebackContractSchema.parse({
    schema_version: "mathdoc.writeback-batch.v1",
    batch_id: batch,
    replay_key: run.id,
    review_branch: {
      database: b.request!.database,
      branch: b.request!.branch,
      data_version: s.version,
    },
    source_proof_request_id: b.request!.identity,
    operations,
    conflict_resolution: "abort_entire_batch",
    certification_visibility: "after_full_batch_commit",
  });
  return {
    changes: [...changes.values()],
    writeback: {
      batch_id: batch,
      run: run.id,
      status: "committed",
      contract,
      created,
      updated: [...changes.keys()].filter((id) => !created.includes(id)),
    },
  };
}
