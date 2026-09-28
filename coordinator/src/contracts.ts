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

export const proofEnvironmentContractSchema = z
  .object({
    schema_version: z.literal("mathdoc.proof-environment.v1-draft"),
    environment_id: id,
    base: z.record(z.unknown()),
    context: z.record(z.unknown()),
    options: z.record(z.unknown()),
    minimum_trust: z.enum(["claimed", "audited", "trusted"]),
    document: documentBinding,
    project_lean_compatibility: z.literal("preserved_unmodified"),
    replaces_environment_id: id.optional(),
  })
  .strict();

const bindingBase = {
  schema_version: z.literal("mathdoc.declaration-binding.v1-draft"),
  node_id: z.string().uuid(),
  block_revision: sha256,
  source_sha256: sha256,
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
        ]),
        details: z.string().min(1),
      })
      .strict(),
  ])
  .superRefine((value, context) => {
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
    kind: z.enum(["update_lean_block", "create_definition_node", "create_goal_node"]),
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
      "certified_batch_wins",
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
  return createHash("sha256")
    .update(JSON.stringify(canonical))
    .digest("hex");
}
