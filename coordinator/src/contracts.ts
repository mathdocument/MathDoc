import { createHash } from "node:crypto";
import { z } from "zod";

const id = z.string().min(1).max(300);
const sha256 = z.string().regex(/^[a-f0-9]{64}$/);
const documentBinding = z
  .object({
    database: z.string().regex(/^[A-Za-z0-9_-]+$/),
    branch: z.string().regex(/^[A-Za-z0-9_-]+$/),
    data_version: id,
  })
  .strict();

export const documentPermissionContractSchema = z
  .object({
    schema_version: z.literal("mathdoc.document-permission.v1-draft"),
    workspace_id: id,
    owner: id,
    branch_rules: z
      .array(
        z
          .object({
            branch: z.string().regex(/^[A-Za-z0-9_-]+$/),
            inherit_workspace_role: z.boolean(),
            grants: z.array(
              z
                .object({
                  actor: id,
                  role: z.enum(["owner", "editor", "viewer"]),
                })
                .strict(),
            ),
          })
          .strict(),
      )
      .min(1),
    authorization_matrix: z
      .array(
        z
          .object({
            action: z.enum([
              "read",
              "write",
              "history",
              "branch_create",
              "branch_delete",
              "export",
              "import",
              "database_create",
              "database_delete",
              "member_manage",
              "proof_request_create",
            ]),
            roles: z.array(z.enum(["admin", "owner", "editor", "viewer"])),
          })
          .strict(),
      )
      .min(1),
  })
  .strict();

export const contextSchema = z
  .object({
    raw: z.array(z.string()).optional(),
    local_options: z.array(z.string()).optional(),
    universes: z.array(z.string()).optional(),
    opens: z.array(z.string()).optional(),
    namespaces: z.array(z.string()).max(0).optional(),
    local_notation: z.array(z.string()).optional(),
    local_attrs: z.array(z.string()).optional(),
    variables: z.array(z.string()).optional(),
  })
  .strict();

export const baseRefSchema = z
  .object({
    base_id: z.string(),
    base_key: z.number().int(),
    root_module: z.string(),
    lean_version: z.string(),
    lean_githash: z.string(),
    package_revision: z.string(),
    package_manifest_hash: z.string(),
    module_list_hash: z.string(),
    build_flags_hash: z.string(),
    platform: z.string(),
    promotion_generation: z.number().int().nonnegative(),
    schema_version: z.literal(1),
  })
  .strict();
const nodeSnapshot = z
  .object({ node_id: z.string().uuid(), revision: sha256 })
  .strict();
export const proofRequestBindingSchema = z
  .object({
    database: z.string().regex(/^[A-Za-z0-9_-]+$/),
    branch: z.string().regex(/^[A-Za-z0-9_-]+$/),
    root: nodeSnapshot,
    dependencies: z.array(nodeSnapshot),
    data_version: id,
  })
  .strict();

export const proofEnvironmentContractSchema = z
  .object({
    schema_version: z.literal("mathdoc.proof-environment.v1-draft"),
    environment_id: id,
    base: baseRefSchema,
    context: contextSchema,
    options: z.record(z.unknown()),
    minimum_trust: z.enum(["claimed", "audited", "trusted"]),
    proof_request: proofRequestBindingSchema,
    project_lean_compatibility: z.literal("preserved_unmodified"),
    replaces_environment_id: id.optional(),
  })
  .strict();

const bindingBase = {
  schema_version: z.literal("mathdoc.declaration-binding.v1-draft"),
  node_id: z.string().uuid(),
  block_revision: sha256,
  source_sha256: sha256,
  required_context: contextSchema,
};
export const declarationBindingContractSchema = z
  .discriminatedUnion("classification", [
    z
      .object({
        ...bindingBase,
        classification: z.literal("definition_only"),
        definitions: z
          .array(
            z
              .object({
                name: id,
                declaration_kind: z.enum([
                  "def",
                  "abbrev",
                  "noncomputable_def",
                  "structure",
                  "inductive",
                ]),
                dependency_definition_ids: z.array(id),
                definition_id: id.optional(),
              })
              .strict(),
          )
          .min(1),
      })
      .strict(),
    z
      .object({
        ...bindingBase,
        classification: z.literal("theorem"),
        conclusion: z
          .object({
            name: id,
            declaration_kind: z.enum(["theorem", "lemma"]),
            goal: z.record(z.unknown()).optional(),
          })
          .strict(),
        premise_placeholders: z.array(
          z
            .object({
              declaration_name: id,
              node_id: z.string().uuid(),
              goal: z.record(z.unknown()),
            })
            .strict(),
        ),
        definitions: z.array(id),
        new_definitions: z.array(id),
        component: z.enum(["lean-worker", "lean-worker-sketch"]),
        mode: z.enum(["leaf", "sketch"]),
      })
      .strict(),
    z
      .object({
        ...bindingBase,
        classification: z.literal("unsupported"),
        reason: z.enum([
          "multiple_conclusions",
          "mixed_declarations",
          "unsupported_declaration",
          "unregistered_statement_definition",
          "missing_environment_instance",
          "ambiguous_binding",
          "local_context_mismatch",
          "premise_name_collision",
        ]),
        details: z.string().min(1),
      })
      .strict(),
  ])
  .superRefine((value, context) => {
    if (value.classification === "theorem") {
      const fail = (message: string) =>
        context.addIssue({ code: z.ZodIssueCode.custom, message });
      if (value.mode === "sketch" && !value.premise_placeholders.length)
        fail("no_premises");
      if (value.mode === "leaf" && value.premise_placeholders.length)
        fail("leaf_has_premises");
      if (!value.conclusion.goal && !value.new_definitions.length)
        fail("goal_required");
      const names = [
        value.conclusion.name,
        ...value.premise_placeholders.map((p) => p.declaration_name),
      ];
      if (new Set(names).size !== names.length) fail("premise_name_collision");
    }
    if (
      value.classification === "theorem" &&
      ((value.component === "lean-worker-sketch" && value.mode !== "sketch") ||
        (value.component === "lean-worker" && value.mode === "sketch"))
    )
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "component_mode_mismatch",
      });
  });

const writeOperation = z
  .object({
    operation_id: id,
    kind: z.enum([
      "update_lean_block",
      "create_definition_node",
      "create_goal_node",
    ]),
    node_id: z.string().uuid(),
    expected_node_revision: sha256.optional(),
    source_sha256: sha256,
    certification_id: id.optional(),
    definition_id: id.optional(),
    preserve_public_declaration_name: z.boolean(),
  })
  .strict();
export const writebackContractSchema = z
  .object({
    schema_version: z.literal("mathdoc.writeback-batch.v1-draft"),
    batch_id: id,
    replay_key: id,
    review_branch: documentBinding,
    source_proof_request_id: id,
    operations: z.array(writeOperation).min(1),
    conflict_resolution: z.enum([
      "abort_entire_batch",
      "reviewer_merge_required",
    ]),
    certification_visibility: z.literal("after_full_batch_commit"),
  })
  .strict()
  .superRefine((value, context) => {
    for (const [index, operation] of value.operations.entries()) {
      if (
        operation.kind === "update_lean_block" &&
        (!operation.expected_node_revision || !operation.certification_id)
      )
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["operations", index],
          message: "update_requires_revision_and_certification",
        });
      if (
        operation.kind === "create_definition_node" &&
        !operation.definition_id
      )
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["operations", index],
          message: "definition_node_requires_definition_id",
        });
    }
  });

export function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(object[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function contractDigest(value: unknown): string {
  return createHash("sha256").update(stableJson(value)).digest("hex");
}

type Environment = z.infer<typeof proofEnvironmentContractSchema>;
export function environmentIdentity(environment: Environment): string {
  const { base, context, options, minimum_trust } = environment;
  return contractDigest({ base, context, options, minimum_trust });
}
export function proofRequestIdentity(environment: Environment): string {
  const { data_version: _snapshot, ...binding } = environment.proof_request;
  return contractDigest({
    environment: environmentIdentity(environment),
    ...binding,
    dependencies: [...binding.dependencies].sort((a, b) =>
      a.node_id.localeCompare(b.node_id),
    ),
  });
}
export function proofRequestIsStale(
  environment: Environment,
  revisions: Record<string, string>,
): boolean {
  return [
    environment.proof_request.root,
    ...environment.proof_request.dependencies,
  ].some((node) => revisions[node.node_id] !== node.revision);
}
// Conservative exact comparison: no semantic rewriting or reordering of commands.
export function bindingContextMatches(
  required: z.infer<typeof contextSchema>,
  fixed: z.infer<typeof contextSchema>,
): boolean {
  return Object.entries(required).every(
    ([key, lines]) =>
      stableJson(lines) === stableJson(fixed[key as keyof typeof fixed] ?? []),
  );
}

export function bindingRejection(
  input: unknown,
  fixed: z.infer<typeof contextSchema>,
): string | null {
  const parsed = declarationBindingContractSchema.safeParse(input);
  if (!parsed.success) {
    return parsed.error.issues.some(
      (issue) => issue.message === "premise_name_collision",
    )
      ? "premise_name_collision"
      : "ambiguous_binding";
  }
  const binding = parsed.data;
  if (binding.classification === "unsupported") return binding.reason;
  return bindingContextMatches(binding.required_context, fixed)
    ? null
    : "local_context_mismatch";
}

// Mirrors Node::revision at baseline a4d61e3: field order is the Rust struct
// order, dependencies are sorted, and metadata is a sorted BTreeMap.
export function legacyNodeRevision(node: {
  fnode: string;
  title: string;
  module: string;
  depens: string[];
  blocks: Array<{
    srctype: string;
    content: string;
    metadata: Record<string, string>;
  }>;
}): string {
  const canonical = {
    fnode: node.fnode,
    title: node.title,
    module: node.module,
    depens: [...node.depens].sort(),
    blocks: node.blocks.map((block) => ({
      srctype: block.srctype,
      content: block.content,
      metadata: Object.fromEntries(Object.entries(block.metadata).sort()),
    })),
  };
  return createHash("sha256").update(JSON.stringify(canonical)).digest("hex");
}
