import { afterEach, describe, expect, it, vi } from "vitest";
import type { NodeDetail, NodeInfo } from "./types";
import { api } from "./api";
import { NodeSession } from "./state.svelte";
import { removeDraft, setDraftDirty } from "./unsaved";

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
  afterEach(() => vi.restoreAllMocks());

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

  it("rejects an external generation during relation synchronization", async () => {
    const session = new NodeSession();
    const original = { node: node("r1"), referrers: [], children: [] };
    session.snapshot = original;
    vi.spyOn(api, "nodeView").mockResolvedValue({ node: node("r2"), referrers: [], children: [] });

    await expect(session.syncView()).rejects.toThrow("changed externally");
    expect(session.snapshot).toBe(original);
  });

  it("updates relation columns while preserving an active draft generation", async () => {
    const session = new NodeSession();
    const original = node("r1");
    session.snapshot = { node: original, referrers: [], children: [] };
    const relation: NodeInfo = {
      fnode: "relation",
      title: "Relation",
      rel_path: "relation.mdoc",
      broken: false,
      depth: 2,
    };
    let resolveView!: (view: { node: NodeDetail; referrers: NodeInfo[]; children: NodeInfo[] }) => void;
    vi.spyOn(api, "nodeView").mockReturnValue(new Promise((resolve) => {
      resolveView = resolve;
    }));

    const syncing = session.syncView();
    const draft = Symbol("draft");
    setDraftDirty(draft, true);
    resolveView({ node: node("r1"), referrers: [relation], children: [relation] });

    await expect(syncing).resolves.toBe(true);
    expect(session.node).toBe(original);
    expect(session.referrers.items).toEqual([relation]);
    expect(session.children.items).toEqual([relation]);
    removeDraft(draft);
  });

  it("drops a stale snapshot when an explicit refresh fails", async () => {
    const session = new NodeSession();
    session.snapshot = { node: node("r1"), referrers: [], children: [] };
    vi.spyOn(api, "nodeView").mockRejectedValue(new Error("node not found"));

    await expect(session.select("node", {
      skipTransition: true,
      skipUnsavedGuard: true,
      clearOnError: true,
    })).resolves.toBe(false);
    expect(session.node).toBeNull();
  });
});

it("keeps successful navigation and browser history isolated between sessions", async () => {
  const browserA = { state: () => null, commit: vi.fn() };
  const browserB = { state: () => null, commit: vi.fn() };
  const a = new NodeSession(browserA);
  const b = new NodeSession(browserB);
  vi.spyOn(api, "nodeView").mockImplementation(async (fnode) => ({
    node: { ...node("r1"), fnode }, referrers: [], children: [],
  }));
  const opts = { skipTransition: true, skipUnsavedGuard: true };
  expect(await a.select("a", opts)).toBe(true);
  expect(await b.select("b", opts)).toBe(true);
  expect(await a.select("next-a", opts)).toBe(true);
  expect(a.history).toEqual(["a", "next-a"]);
  expect(b.history).toEqual(["b"]);
  expect(browserA.commit).toHaveBeenLastCalledWith("push", "next-a", {
    mdcHistory: 1, fnode: "next-a", index: 1, entries: ["a", "next-a"],
  });
  await a.clearSelection({ skipUnsavedGuard: true });
  expect(a.selectedFnode).toBeNull();
  expect(b.selectedFnode).toBe("b");
  expect(browserB.commit).toHaveBeenCalledTimes(1);
  vi.restoreAllMocks();
});
