// Proof environments and the conversion of document nodes into LeanGround requests
// (plan §5, §7.2, §7.3; contracts mathdoc.proof-environment.v1 and
// mathdoc.declaration-binding.v1). Environments live in PostgreSQL next to, never inside,
// the legacy Project/lean document.
import { createHash } from "node:crypto";
import type pg from "pg";
import { z } from "zod";
import {
  baseRefSchema,
  contextSchema,
  contractDigest,
  declarationBindingContractSchema,
  leanContentSha256,
  proofEnvironmentContractSchema,
  proofRequestIdentity,
  proofRequestIsStale,
  proofRequestSnapshot,
} from "./contracts.js";
import {
  body,
  classify,
  contextMismatch,
  type Decl,
  placeholder,
  proposition,
  type Scan,
  scan,
} from "./lean-source.js";
import { trustSchema } from "./domain.js";
import type { Node } from "./docs/model.js";

export const ENVIRONMENT_MIGRATION = `
CREATE TABLE IF NOT EXISTS mdc_environment(database text NOT NULL, branch text NOT NULL, body jsonb NOT NULL, PRIMARY KEY(database, branch));
`;

const optionValue = z.union([z.string(), z.number(), z.boolean()]);
export const environmentInputSchema = z
  .object({
    base_key: z.number().int().nonnegative(),
    context: contextSchema.default({}),
    options: z.record(optionValue).default({}),
    minimum_trust: trustSchema.default("audited"),
  })
  .strict();
export type EnvironmentInput = z.infer<typeof environmentInputSchema>;
export interface Environment extends EnvironmentInput {
  base: z.infer<typeof baseRefSchema>;
  /** contract environmentIdentity: BaseRef, context, options, minimum_trust only. */
  environment_id: string;
  replaces_environment_id?: string;
  updated_by: string;
}

export function environmentId(
  e: Pick<Environment, "base" | "context" | "options" | "minimum_trust">,
): string {
  return contractDigest({
    base: e.base,
    context: e.context,
    options: e.options,
    minimum_trust: e.minimum_trust,
  });
}

/** One proof environment per document branch. Changing it never rewrites old requests. */
export class Environments {
  constructor(private pool: pg.Pool) {}
  async get(database: string, branch: string): Promise<Environment | null> {
    const row = (
      await this.pool.query(
        "SELECT body FROM mdc_environment WHERE database=$1 AND branch=$2",
        [database, branch],
      )
    ).rows[0];
    return row ? (row.body as Environment) : null;
  }
  async put(
    database: string,
    branch: string,
    input: EnvironmentInput,
    base: z.infer<typeof baseRefSchema>,
    actor: string,
  ): Promise<Environment> {
    const prior = await this.get(database, branch);
    const env: Environment = {
      ...input,
      base,
      environment_id: "",
      updated_by: actor,
    };
    env.environment_id = environmentId(env);
    if (prior && prior.environment_id !== env.environment_id)
      env.replaces_environment_id = prior.environment_id;
    else if (prior?.replaces_environment_id)
      env.replaces_environment_id = prior.replaces_environment_id;
    await this.pool.query(
      "INSERT INTO mdc_environment VALUES($1,$2,$3) ON CONFLICT (database,branch) DO UPDATE SET body=EXCLUDED.body",
      [database, branch, env],
    );
    return env;
  }
}

/** How one document node takes part in a ProofRequest. */
export interface NodeBinding {
  node: string;
  title: string;
  role: "definition" | "theorem" | "unsupported";
  /** Lean text fingerprint when the node was converted; later edits make it differ. */
  lean_content_sha256: string;
  classification_source?: "syntax_scan" | "explicit_metadata";
  /** theorem: conclusion name and resolved goal. */
  conclusion?: string;
  goal?: string;
  /** theorem: direct premise nodes; definition closure used by the statement. */
  premises?: string[];
  definition_nodes?: string[];
  /** definition: registered LeanGround definition IDs, in source order. */
  definition_ids?: string[];
  state:
    "open" | "submitted" | "registered" | "rejected" | "pending" | "certified";
  certificate?: string;
  /** Contract declaration-binding.v1 of the last submission. */
  binding?: unknown;
  reason?: string;
  details?: string;
}

export interface ProofRequest {
  database: string;
  branch: string;
  node: string;
  environment_id: string;
  replaces_environment_id?: string;
  bound: boolean;
  contract?: z.infer<typeof proofEnvironmentContractSchema>;
  identity?: string;
  stale: boolean;
  checked_version?: string;
  error?: string;
}

interface Converted {
  scan: Scan;
  decl?: Decl;
}
export interface Conversion {
  bindings: Record<string, NodeBinding>;
  converted: Map<string, Converted>;
  /** Definition nodes, dependencies first. */
  definitions: string[];
  theorems: string[];
}

const lean = (n: Node) => n.blocks.find((b) => b.srctype === "lean");

/**
 * Classify every Lean node the root depends on. Nodes without a Lean block are informal
 * and take no part. A problem in one node is recorded on that node and never guessed.
 */
export function convert(
  nodes: Map<string, Node>,
  root: string,
  fixed: z.infer<typeof contextSchema>,
): Conversion {
  const out: Conversion = {
    bindings: {},
    converted: new Map(),
    definitions: [],
    theorems: [],
  };
  const reachable: string[] = [];
  const seen = new Set<string>();
  const visit = (id: string) => {
    if (seen.has(id) || !nodes.has(id)) return;
    seen.add(id);
    const n = nodes.get(id)!;
    if (!lean(n)) return;
    for (const d of n.depens) visit(d);
    reachable.push(id); // post-order: dependencies first
  };
  visit(root);
  const formal = (id: string) =>
    nodes.get(id)!.depens.filter((d) => seen.has(d) && lean(nodes.get(d)!));
  for (const id of reachable) {
    const n = nodes.get(id)!;
    const block = lean(n)!;
    const s = scan(block.content);
    const c = classify(s, block.metadata);
    const binding: NodeBinding = {
      node: id,
      title: n.title,
      role: c.role,
      lean_content_sha256: leanContentSha256(n)!,
      state: "pending",
    };
    out.converted.set(id, { scan: s });
    out.bindings[id] = binding;
    if (c.role === "unsupported") {
      Object.assign(binding, {
        state: "rejected",
        reason: c.rejection.reason,
        details: c.rejection.details,
      });
      continue;
    }
    binding.classification_source = c.source;
    const mismatch = contextMismatch(s.context, fixed);
    if (mismatch) {
      Object.assign(binding, {
        state: "rejected",
        reason: "local_context_mismatch",
        details: mismatch,
      });
      continue;
    }
    if (c.role === "theorem") {
      out.converted.get(id)!.decl = c.conclusion;
      binding.conclusion = c.conclusion.name;
      binding.state = c.conclusion.sorry ? "open" : "pending";
    }
  }
  const role = (id: string) => out.bindings[id]?.role;
  const reject = (b: NodeBinding, reason: string, details: string) => {
    if (b.state === "rejected") return;
    Object.assign(b, { state: "rejected", reason, details });
  };
  // Definition closure: follow definition nodes only; theorem dependencies are premises.
  const closure = (id: string) => {
    const found = new Set<string>();
    const walk = (x: string) => {
      for (const d of formal(x))
        if (role(d) === "definition" && !found.has(d)) {
          found.add(d);
          walk(d);
        }
    };
    walk(id);
    return [...found];
  };
  for (const id of reachable) {
    const b = out.bindings[id];
    const deps = formal(id);
    const blocked = deps.filter((d) => out.bindings[d].state === "rejected");
    if (b.role === "definition") {
      const theorem = deps.find((d) => role(d) === "theorem");
      if (theorem)
        reject(
          b,
          "unsupported_declaration",
          `a definition node cannot depend on theorem node ${out.bindings[theorem].title}`,
        );
      else if (blocked.length)
        reject(
          b,
          "unregistered_statement_definition",
          `depends on rejected node ${out.bindings[blocked[0]].title}`,
        );
      else out.definitions.push(id);
    } else if (b.role === "theorem") {
      b.premises = deps.filter((d) => role(d) === "theorem");
      b.definition_nodes = closure(id);
      if (blocked.length)
        reject(
          b,
          role(blocked[0]) === "definition"
            ? "unregistered_statement_definition"
            : "ambiguous_binding",
          `depends on rejected node ${out.bindings[blocked[0]].title}`,
        );
      const own = out.converted.get(id)!.scan.decls.map((d) => d.name);
      const names = [
        ...own,
        ...b.premises.map((p) => out.bindings[p].conclusion!),
      ];
      const dup = names.find((x, i) => names.indexOf(x) !== i);
      if (dup)
        reject(
          b,
          "premise_name_collision",
          `the name ${dup} is used by more than one premise or by this node`,
        );
      out.theorems.push(id);
    }
  }
  return out;
}

/** The submission for a theorem node: premise placeholders, then the node's declarations. */
export function submission(conversion: Conversion, id: string) {
  const b = conversion.bindings[id];
  const c = conversion.converted.get(id)!;
  const premises = b.premises ?? [];
  const source =
    premises
      .map((p) => placeholder(conversion.converted.get(p)!.decl!))
      .join("") + body(c.scan);
  return {
    source,
    expected_root: b.conclusion!,
    component: premises.length ? "lean-worker-sketch" : "lean-worker",
    mode: premises.length ? ("sketch" as const) : ("leaf" as const),
  };
}

export function conclusionProposition(conversion: Conversion, id: string) {
  return proposition(conversion.converted.get(id)!.decl!);
}
export function definitionSource(conversion: Conversion, id: string) {
  return body(conversion.converted.get(id)!.scan);
}

/** Contract declaration-binding.v1 for a theorem submission; parsing enforces its rules. */
export function theoremBinding(
  conversion: Conversion,
  id: string,
  goals: Record<string, Record<string, unknown>>,
  definitionIds: string[],
  source: string,
) {
  const b = conversion.bindings[id];
  const s = submission(conversion, id);
  return declarationBindingContractSchema.parse({
    schema_version: "mathdoc.declaration-binding.v1",
    node_id: id,
    block_revision: b.lean_content_sha256,
    source_sha256: digestText(source),
    required_context: conversion.converted.get(id)!.scan.context,
    classification: "theorem",
    classification_source: b.classification_source,
    conclusion: {
      name: b.conclusion,
      declaration_kind:
        conversion.converted.get(id)!.decl!.kind === "lemma"
          ? "lemma"
          : "theorem",
      goal: goals[id],
    },
    premise_placeholders: (b.premises ?? []).map((p) => ({
      declaration_name: conversion.bindings[p].conclusion,
      node_id: p,
      goal: goals[p],
    })),
    definitions: definitionIds,
    new_definitions: [],
    component: s.component,
    mode: s.mode,
  });
}

export function digestText(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** The environment contract with the node snapshot (fingerprints of Lean text only). */
export function requestContract(
  env: Environment,
  request: Pick<ProofRequest, "database" | "branch" | "node">,
  nodes: Map<string, Node>,
  conversion: Conversion,
  dataVersion: string,
) {
  // Informal nodes (no Lean block) are not part of the request; unsupported nodes are
  // fingerprinted like premises so that editing them still makes the request stale.
  const formal: Record<string, Node> = {};
  for (const id of Object.keys(conversion.bindings)) {
    const n = nodes.get(id)!;
    formal[id] = {
      ...n,
      depens: n.depens.filter((d) => d in conversion.bindings),
    };
  }
  const roles = Object.fromEntries(
    Object.values(conversion.bindings).map((b) => [
      b.node,
      b.role === "definition" ? "definition" : "theorem",
    ]),
  ) as Record<string, "definition" | "theorem">;
  const snap = proofRequestSnapshot(formal, request.node, roles);
  const contract = proofEnvironmentContractSchema.parse({
    schema_version: "mathdoc.proof-environment.v1",
    environment_id: env.environment_id,
    base: env.base,
    context: env.context,
    options: env.options,
    minimum_trust: env.minimum_trust,
    proof_request: {
      database: request.database,
      branch: request.branch,
      root: snap.root,
      dependencies: snap.dependencies,
      data_version: dataVersion,
    },
    project_lean_compatibility: "preserved_unmodified",
    ...(env.replaces_environment_id
      ? { replaces_environment_id: env.replaces_environment_id }
      : {}),
  });
  return { contract, identity: proofRequestIdentity(contract) };
}

export function isStale(
  contract: z.infer<typeof proofEnvironmentContractSchema>,
  nodes: Map<string, Node>,
): boolean {
  const current: Record<string, string | null> = {};
  for (const n of [
    contract.proof_request.root,
    ...contract.proof_request.dependencies,
  ]) {
    const node = nodes.get(n.node_id);
    current[n.node_id] = node ? leanContentSha256(node) : null;
  }
  return proofRequestIsStale(contract, current);
}
