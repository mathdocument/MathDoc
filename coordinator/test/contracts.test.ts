import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { contractSchemas } from "../scripts/contract-schemas.js";
import {
  contractDigest,
  declarationBindingContractSchema,
  documentPermissionContractSchema,
  legacyNodeRevision,
  proofEnvironmentContractSchema,
  stableJson,
  writebackContractSchema,
  environmentIdentity,
  proofRequestIdentity,
  proofRequestIsStale,
  bindingRejection,
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
    assert.equal(
      schema.$schema,
      "https://json-schema.org/draft/2020-12/schema",
    );
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
  assert.notEqual(
    contractDigest(environments[0]),
    contractDigest(environments[1]),
  );
  assert.equal(
    environments[1].replaces_environment_id,
    environments[0].environment_id,
  );

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
    bindings.map(
      (binding: { classification: string }) => binding.classification,
    ),
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
  assert.equal(
    declarationBindingContractSchema.safeParse(theorem).success,
    false,
  );

  const ambiguous = structuredClone(fixture.declaration_bindings[2].contract);
  delete ambiguous.details;
  assert.equal(
    declarationBindingContractSchema.safeParse(ambiguous).success,
    false,
  );

  const updateWithoutRevision = structuredClone(fixture.writeback);
  delete updateWithoutRevision.operations[1].expected_node_revision;
  assert.equal(
    writebackContractSchema.safeParse(updateWithoutRevision).success,
    false,
  );
});

test("legacy node revision is independent of dependency and metadata order", () => {
  for (const { node, expected } of fixture.legacy_revision) {
    assert.equal(legacyNodeRevision(node), expected);
    const reordered = structuredClone(node);
    reordered.depens.reverse();
    reordered.blocks[0].metadata = { z: "last", a: "first" };
    assert.equal(legacyNodeRevision(reordered), expected);
  }
});

test("node snapshots invalidate only root and dependency edits", () => {
  const environment = proofEnvironmentContractSchema.parse(
    fixture.proof_environments[0],
  );
  for (const scenario of fixture.staleness_cases) {
    const snapshot = structuredClone(environment);
    snapshot.proof_request.data_version = scenario.data_version;
    assert.equal(
      environmentIdentity(snapshot),
      environmentIdentity(environment),
    );
    assert.equal(
      proofRequestIdentity(snapshot),
      proofRequestIdentity(environment),
    );
    const revisions = Object.fromEntries(
      scenario.nodes.map((node: Parameters<typeof legacyNodeRevision>[0]) => [
        node.fnode,
        legacyNodeRevision(node),
      ]),
    );
    assert.equal(
      proofRequestIsStale(snapshot, revisions),
      scenario.expected_stale,
      scenario.change,
    );
  }
  assert.notEqual(
    environmentIdentity(fixture.proof_environments[1]),
    environmentIdentity(environment),
  );
});

test("fixed context and premise names reject incompatible bindings", () => {
  const theorem = fixture.declaration_bindings[1].contract;
  for (const scenario of fixture.context_cases) {
    const binding = declarationBindingContractSchema.parse({
      ...theorem,
      required_context: scenario.required,
    });
    assert.equal(
      bindingRejection(binding, fixture.proof_environments[0].context),
      scenario.expected,
    );
  }
  for (const scenario of fixture.premise_collision_cases) {
    const binding = {
      ...theorem,
      mode: "sketch",
      component: "lean-worker-sketch",
      conclusion: { ...theorem.conclusion, name: scenario.conclusion },
      premise_placeholders: scenario.names.map(
        (declaration_name: string, i: number) => ({
          declaration_name,
          node_id: fixture.declaration_bindings[i].contract.node_id,
          goal: { key: "fixture-premise" },
        }),
      ),
    };
    const result = declarationBindingContractSchema.safeParse(binding);
    assert(!result.success);
    assert(
      result.error.issues.some(
        (issue) => issue.message === "premise_name_collision",
      ),
    );
    assert.equal(bindingRejection(binding, {}), "premise_name_collision");
  }
});

test("JSON Schemas stay generated and accept/reject the same structural cases as Zod", () => {
  const ajv = new Ajv2020({ strict: false });
  addFormats(ajv);
  const schemas = contractSchemas();
  const validators = Object.fromEntries(
    Object.entries(schemas).map(([name, schema]) => {
      assert.deepEqual(
        JSON.parse(
          readFileSync(
            new URL(`../contracts/v1/${name}.schema.json`, import.meta.url),
            "utf8",
          ),
        ),
        schema,
      );
      return [name, ajv.compile(schema)];
    }),
  );
  const check = (
    name: string,
    zod: { safeParse: (value: unknown) => { success: boolean } },
    value: unknown,
    expected: boolean,
  ) => {
    assert.equal(zod.safeParse(value).success, expected, `${name}: Zod`);
    assert.equal(
      validators[name](value),
      expected,
      `${name}: JSON Schema ${JSON.stringify(validators[name].errors)}`,
    );
  };
  check(
    "document-permission",
    documentPermissionContractSchema,
    fixture.document_permission,
    true,
  );
  check("writeback-batch", writebackContractSchema, fixture.writeback, true);
  check(
    "writeback-batch",
    writebackContractSchema,
    { ...fixture.writeback, conflict_resolution: "certified_batch_wins" },
    false,
  );
  const env = fixture.proof_environments[0];
  check("proof-environment", proofEnvironmentContractSchema, env, true);
  for (const context of [
    { project_file: "set_option autoImplicit false" },
    { opens: "open Nat" },
    { namespaces: ["N"] },
    { definitions: [] },
  ])
    check(
      "proof-environment",
      proofEnvironmentContractSchema,
      { ...env, context },
      false,
    );
  for (const key of Object.keys(env.base)) {
    const base = { ...env.base };
    delete base[key];
    check(
      "proof-environment",
      proofEnvironmentContractSchema,
      { ...env, base },
      false,
    );
  }
  check(
    "proof-environment",
    proofEnvironmentContractSchema,
    { ...env, base: { base_key: 17, project: "Mathlib" } },
    false,
  );
  for (const { contract } of fixture.declaration_bindings)
    check(
      "declaration-binding",
      declarationBindingContractSchema,
      contract,
      true,
    );
  const leaf = fixture.declaration_bindings[1].contract;
  const premise = {
    declaration_name: "premise",
    node_id: fixture.declaration_bindings[0].contract.node_id,
    goal: { key: "fixture-premise" },
  };
  const sketch = {
    ...leaf,
    mode: "sketch",
    component: "lean-worker-sketch",
    premise_placeholders: [premise],
  };
  check("declaration-binding", declarationBindingContractSchema, sketch, true);
  check(
    "declaration-binding",
    declarationBindingContractSchema,
    { ...sketch, premise_placeholders: [] },
    false,
  );
  check(
    "declaration-binding",
    declarationBindingContractSchema,
    { ...leaf, premise_placeholders: [premise] },
    false,
  );
  check(
    "declaration-binding",
    declarationBindingContractSchema,
    { ...leaf, component: "lean-worker-sketch" },
    false,
  );
  const noGoal = structuredClone(leaf);
  delete noGoal.conclusion.goal;
  check("declaration-binding", declarationBindingContractSchema, noGoal, false);
  check(
    "declaration-binding",
    declarationBindingContractSchema,
    { ...noGoal, new_definitions: ["fixture-definition"] },
    true,
  );
});
