import { zodToJsonSchema } from "zod-to-json-schema";
import {
  documentPermissionContractSchema,
  proofEnvironmentContractSchema,
  declarationBindingContractSchema,
  writebackContractSchema,
} from "../src/contracts.js";

// JSON Schema covers structure; projection uniqueness and cross-document context
// comparisons are semantic checks in contracts.ts, not JSON Schema keywords.
export function contractSchemas(): Record<string, any> {
  const schemas: Record<string, any> = {};
  for (const [name, schema] of Object.entries({
    "document-permission": documentPermissionContractSchema,
    "proof-environment": proofEnvironmentContractSchema,
    "declaration-binding": declarationBindingContractSchema,
    "writeback-batch": writebackContractSchema,
  })) {
    schemas[name] = {
      ...zodToJsonSchema(schema, {
        target: "jsonSchema7",
        $refStrategy: "none",
      }),
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $id: `https://mathdoc.local/contracts/v1/${name}.schema.json`,
    };
  }
  const theorem = schemas["declaration-binding"].anyOf[1];
  theorem.allOf = [
    {
      if: { properties: { mode: { const: "sketch" } } },
      then: {
        properties: {
          component: { const: "lean-worker-sketch" },
          premise_placeholders: { minItems: 1 },
        },
      },
      else: {
        properties: {
          component: { const: "lean-worker" },
          premise_placeholders: { maxItems: 0 },
        },
      },
    },
    {
      if: { properties: { new_definitions: { maxItems: 0 } } },
      then: { properties: { conclusion: { required: ["goal"] } } },
    },
  ];
  theorem.$comment =
    "Semantic validation also requires unique premise declaration names, distinct from conclusion.name; failure is unsupported/premise_name_collision. Required context must match the fixed ProofRequest context per contracts.ts.";
  schemas["proof-environment"].properties.proof_request.$comment =
    "Snapshots fingerprint the Lean block text only (leanContentSha256), never the whole-node revision. Semantic validation also requires root and dependency node_ids to be distinct; dependencies follow proofRequestSnapshot in contracts.ts (definitions transitively, theorem premises directly).";
  schemas["writeback-batch"].properties.operations.items.allOf = [
    {
      if: { properties: { kind: { const: "update_lean_block" } } },
      then: { required: ["expected_node_revision", "certification_id"] },
    },
    {
      if: { properties: { kind: { const: "create_definition_node" } } },
      then: { required: ["definition_id"] },
    },
  ];
  return schemas;
}
