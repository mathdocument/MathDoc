import { afterEach, expect, test, vi } from "vitest";
import { LeanEditorSession, type LeanEditorSessionProps } from "./lean-session";
import { hasUnsavedDrafts } from "../../lib/unsaved";

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
