import { afterEach, describe, expect, it, vi } from "vitest";
import type { NodeDetail, NodePreview } from "./types";
import { api } from "./api";
import { latexApi, type LatexPreviewResult } from "./latex";
import { NodeSession } from "./state.svelte";
import { removeDraft, setDraftDirty } from "./unsaved";

function node(revision: string): NodeDetail {
  return {
    fnode: "node",
    title: `Node ${revision}`,
    broken: false,
    depth: 1,
    revision,
    depens: [revision],
    blocks: [{ srctype: "text", content: revision, metadata: {} }],
    formalization: { lean: "unverified", rocq: "unverified" },
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

  it("prepares reading-mode navigation before committing and cancels superseded previews", async () => {
    const session = new NodeSession({state: () => null, commit: vi.fn()});
    const opts = {skipTransition: true, skipUnsavedGuard: true};
    vi.spyOn(api, "nodeView").mockImplementation(async fnode => ({
      node: {...node("r1"), fnode, blocks: [{srctype: "latex", content: fnode, metadata: {}}]}, referrers: [], children: [],
    }));
    let finish!: (result: LatexPreviewResult) => void;
    const preview = vi.spyOn(latexApi, "preview").mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    await session.select("a", opts);
    expect(preview).not.toHaveBeenCalled();
    session.latexPreview = true;
    const pending = session.select("b", opts);
    await vi.waitFor(() => expect(preview).toHaveBeenCalledTimes(1));
    expect(session.node?.fnode).toBe("a");
    expect(session.history).toEqual(["a"]);
    const result = {project_key: "p", context_key: "c", html: "ready", labels: [], diagnostics: []};
    finish(result);
    expect(await pending).toBe(true);
    expect(session.load).toMatchObject({node: {fnode: "b"}, latexPreview: {preview: result, error: null}});

    const stale = session.select("stale", opts);
    await vi.waitFor(() => expect(preview).toHaveBeenCalledTimes(2));
    const signal = preview.mock.calls[1][2]!;
    session.latexPreview = false;
    await session.select("next", opts);
    expect(signal.aborted).toBe(true);
    finish(result);
    expect(await stale).toBe(false);
    expect(session.node?.fnode).toBe("next");
    expect(session.history).toEqual(["a", "b", "next"]);

    // A broken preview still opens the node so its source can be corrected.
    session.latexPreview = true;
    preview.mockRejectedValueOnce(new Error("invalid macro"));
    expect(await session.select("broken", opts)).toBe(true);
    expect(session.load).toMatchObject({node: {fnode: "broken"}, latexPreview: {preview: null, error: "invalid macro"}});
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
    const relation: NodePreview = {
      fnode: "relation",
      title: "Relation",
      broken: false,
      depth: 2,
      formalization: { lean: "verified", rocq: "unverified" },
    };
    let resolveView!: (view: { node: NodeDetail; referrers: NodePreview[]; children: NodePreview[] }) => void;
    vi.spyOn(api, "nodeView").mockReturnValue(new Promise((resolve) => {
      resolveView = resolve;
    }));

    const syncing = session.syncView();
    const draft = Symbol("draft");
    setDraftDirty(draft, true);
    resolveView({ node: node("r1"), referrers: [relation], children: [relation] });

    await expect(syncing).resolves.toBe(true);
    expect(session.node).toBe(original);
    expect(session.referrers).toEqual([relation]);
    expect(session.children).toEqual([relation]);
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
