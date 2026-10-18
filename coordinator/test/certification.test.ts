// The certification dimension of a node's status (docs/mathdoc.md §4.5) as the document API
// shows it, and the role the shown conversion binds the node to.
import assert from "node:assert/strict";
import { test } from "node:test";
import { certificationLookup } from "../src/certification.js";
import { leanContentSha256 } from "../src/contracts.js";
import type { Board } from "../src/domain.js";
import type { Node } from "../src/docs/model.js";
import type { NodeBinding } from "../src/proof.js";

const node = (fnode: string, lean: string): Node => ({
  fnode,
  title: fnode,
  module: "",
  depens: [],
  blocks: [{ srctype: "lean", content: lean, metadata: {} }],
});
const bound = (
  n: Node,
  role: NodeBinding["role"],
  state: NodeBinding["state"],
  extra: Partial<NodeBinding> = {},
): NodeBinding => ({
  node: n.fnode,
  title: n.title,
  role,
  state,
  lean_content_sha256: leanContentSha256(n)!,
  ...extra,
});

test("a definition an accepted writeback marked certified stays registered; roles come from the binding", () => {
  const def = node("d", "def two : Nat := 2\n");
  const thm = node("t", "def helper : Nat := 1\ntheorem t : two = 2 := rfl\n");
  const waiting = node("w", "def three : Nat := 3\n");
  const free = node("f", "theorem f : True := trivial\n");
  const board = {
    id: "p",
    revision: 1,
    minimum_trust: "audited",
    project: { database: "db", environment_id: "e", root_node: "t" },
    goals: { g: { key: "g", statement: "", status: "certified" } },
    branches: {
      main: {
        nodes: {
          d: bound(def, "definition", "certified", { definition_ids: ["D1"] }),
          t: bound(thm, "theorem", "certified", { goal: "g" }),
          w: bound(waiting, "definition", "pending"),
        },
      },
    },
  } as unknown as Board;
  const nodes = new Map([def, thm, waiting, free].map((n) => [n.fnode, n]));
  const lookup = certificationLookup([board], nodes, "main");
  assert.equal(lookup(def)?.status, "registered");
  assert.deepEqual(
    (lookup(def) as { definition_ids?: string[] }).definition_ids,
    ["D1"],
  );
  assert.equal(lookup(thm)?.status, "certified");
  // Still waiting for its dependencies to be registered.
  assert.equal(lookup(waiting)?.status, "not_submitted");
  assert.equal(lookup(free)?.status, "not_submitted");
  // A block with a definition and a theorem is bound as a theorem; unbound nodes have no role here.
  assert.deepEqual(
    [def, thm, waiting, free].map((n) => lookup.role(n)),
    ["definition", "theorem", "definition", null],
  );
});
