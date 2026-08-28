import { describe, expect, it } from "vitest";
import type { NodeDetail } from "./types";
import { NodeSession } from "./state.svelte";

function node(revision: string): NodeDetail {
  return {
    fnode: "node",
    title: `Node ${revision}`,
    rel_path: "node.mdoc",
    broken: false,
    depth: 1,
    revision,
    depens: [revision],
    blocks: [{ srctype: "text", content: revision, metadata: {} }],
    formalization: { lean: "no_code", rocq: "no_code" },
  };
}

describe("NodeSession", () => {
  it("keeps one complete node generation behind both views", () => {
    const session = new NodeSession();
    session.snapshot = { node: node("r1"), referrers: [], children: [] };
    const updated = node("r2");

    session.acceptNode(updated);
    session.selectionCleared = true;

    expect(session.node).toBe(updated);
    expect(session.selectedFnode).toBeNull();
    expect(session.selectedLoad).toEqual({ kind: "idle" });
    expect(session.load).toEqual({ kind: "ready", node: updated });
  });
});
