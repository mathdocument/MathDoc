import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import {
  contractDigest,
  declarationBindingContractSchema,
  documentPermissionContractSchema,
  legacyNodeRevision,
  proofEnvironmentContractSchema,
  stableJson,
  writebackContractSchema,
} from "../src/contracts.js";

const fixture = JSON.parse(
  readFileSync(
    new URL("./contracts/v1/contracts.fixture.json", import.meta.url),
    "utf8",
  ),
);

test("all four v1 draft schemas and fixtures are versioned and deterministic", () => {
  const schemaNames = [
    "document-permission",
    "proof-environment",
    "declaration-binding",
    "writeback-batch",
  ];
  for (const name of schemaNames) {
    const schema = JSON.parse(
      readFileSync(
        new URL(`../contracts/v1/${name}.schema.json`, import.meta.url),
        "utf8",
      ),
    );
    assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");
    assert.match(schema.$id, /\/contracts\/v1\//);
  }

  const permission = documentPermissionContractSchema.parse(
    fixture.document_permission,
  );
  const write = permission.authorization_matrix.find(
    (entry) => entry.action === "write",
  )!;
  assert(write.roles.includes("editor"));
  assert(!write.roles.includes("viewer"));

  const environments = fixture.proof_environments.map((value: unknown) =>
    proofEnvironmentContractSchema.parse(value),
  );
  assert.notEqual(contractDigest(environments[0]), contractDigest(environments[1]));
  assert.equal(environments[1].replaces_environment_id, environments[0].environment_id);

  const bindings = fixture.declaration_bindings.map(
    ({ source, contract }: { source: string; contract: unknown }) => {
      assert.equal(
        createHash("sha256").update(source).digest("hex"),
        (contract as { source_sha256: string }).source_sha256,
      );
      return declarationBindingContractSchema.parse(contract);
    },
  );
  assert.deepEqual(
    bindings.map((binding: { classification: string }) => binding.classification),
    ["definition_only", "theorem", "unsupported"],
  );
  assert.equal(bindings[2].classification, "unsupported");

  const writeback = writebackContractSchema.parse(fixture.writeback);
  assert.equal(writeback.operations.length, 2);
  assert.equal(writeback.certification_visibility, "after_full_batch_commit");

  const first = contractDigest(writeback);
  const reordered = Object.fromEntries(Object.entries(writeback).reverse());
  assert.equal(contractDigest(reordered), first);
  assert.equal(stableJson(reordered), stableJson(writeback));
});

test("contract rejects guessed declaration bindings and unsafe writeback shapes", () => {
  const theorem = structuredClone(fixture.declaration_bindings[1].contract);
  theorem.component = "lean-worker-sketch";
  assert.equal(declarationBindingContractSchema.safeParse(theorem).success, false);

  const ambiguous = structuredClone(fixture.declaration_bindings[2].contract);
  delete ambiguous.details;
  assert.equal(declarationBindingContractSchema.safeParse(ambiguous).success, false);

  const updateWithoutRevision = structuredClone(fixture.writeback);
  delete updateWithoutRevision.operations[1].expected_node_revision;
  assert.equal(writebackContractSchema.safeParse(updateWithoutRevision).success, false);
});

test("legacy node revision is independent of dependency and metadata order", () => {
  const { node, expected } = fixture.legacy_revision;
  assert.equal(legacyNodeRevision(node), expected);
  const reordered = structuredClone(node);
  reordered.depens.reverse();
  reordered.blocks[0].metadata = { z: "last", a: "first" };
  assert.equal(legacyNodeRevision(reordered), expected);
});
