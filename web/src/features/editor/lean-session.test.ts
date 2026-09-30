import { afterEach, expect, test, vi } from "vitest";
import { LeanEditorSession, type LeanEditorSessionProps } from "./lean-session";
import { hasUnsavedDrafts } from "../../lib/unsaved";
import { api } from "../../lib/api";
import type { NodeDetail, NodeView } from "../../lib/types";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
test("native messages are scoped to this frame; navigation resets drafts without retiring the runtime", () => {
  vi.stubGlobal("location", { origin: "http://localhost" });
  const postMessage = vi.fn(),
    contentWindow = { postMessage };
  const frame = { contentWindow } as unknown as HTMLIFrameElement;
  const props: LeanEditorSessionProps = {
    fnode: "a",
    revision: "1",
    module: "A",
    block: { srctype: "lean", content: "saved", metadata: {} },
    theme: "light",
    onChange: vi.fn(),
  };
  const s = new LeanEditorSession(props);
  s.attachFrame(frame);
  s.update(props);
  const message = (
    data: unknown,
    source: unknown = contentWindow,
    origin = "http://localhost",
  ) => s.message({ data, source, origin } as MessageEvent);
  message({ type: "lean-runtime-ready" });
  message(
    { type: "lean-change", fnode: "a", value: "foreign" },
    {},
    "https://foreign.test",
  );
  expect(s.content).toBe("saved");
  message({ type: "lean-change", fnode: "a", value: "draft" });
  expect(props.onChange).toHaveBeenLastCalledWith("draft");
  expect(hasUnsavedDrafts()).toBe(true);
  s.update({ ...props, module: "Renamed" });
  expect(s.content).toBe("draft");
  expect(postMessage).toHaveBeenCalledWith(
    expect.objectContaining({
      type: "lean-select",
      module: "Renamed",
      source: "draft",
    }),
    "http://localhost",
  );
  s.update({
    ...props,
    fnode: "b",
    block: { ...props.block!, content: "next" },
  });
  expect(s.content).toBe("next");
  expect(s.runtimeReady).toBe(true);
  expect(hasUnsavedDrafts()).toBe(false);
  expect(postMessage).toHaveBeenCalledWith(
    { type: "lean-source", fnode: "a", value: "saved" },
    "http://localhost",
  );
  s.destroy();
});

test("certification never replaces the editor with a different database revision", async () => {
  const node: NodeDetail = {
    fnode: "a", name: "A", revision: "1", depth: 0, depens: [],
    blocks: [{ srctype: "lean", content: "saved", metadata: {} }],
    formalization: { lean: "verified", rocq: "unverified" },
  };
  const onCertified = vi.fn();
  const session = new LeanEditorSession({
    fnode: node.fnode, revision: node.revision, theme: "light", block: node.blocks[0], onCertified,
  });
  const checked = {
    fnode: "a", revision: "1", passed: true, certified: true, has_sorry: false,
    built: false, cache_hit: true, elapsed_ms: 0, diagnostics: [], dependency_errors: [],
  };
  session.content = session.baseline = "saved";
  let finish!: (view: NodeView) => void;
  vi.spyOn(api, "nodeView").mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  try {
    const pending = session.certified(checked);
    finish({ node: { ...node, revision: "2" }, children: [], referrers: [] });
    await pending;
    expect(onCertified).not.toHaveBeenCalled();
    expect(session.result).toBeNull();
    expect(session.error).toContain("changed externally");

    await session.certified({ ...checked, fnode: "another-node" });
    expect(api.nodeView).toHaveBeenCalledTimes(1);

    const retry = session.certified(checked);
    finish({ node, children: [], referrers: [] });
    await retry;
    expect(onCertified).toHaveBeenCalledExactlyOnceWith(node);

    let fail!: (error: Error) => void;
    vi.mocked(api.nodeView).mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; }));
    const stale = session.certified(checked);
    session.content = "new draft";
    fail(new Error("late error"));
    await stale;
    expect(session.error).toBeNull();
  } finally { session.destroy(); }
});
