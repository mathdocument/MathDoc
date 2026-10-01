import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { WorkspaceController } from "./controller";
import { nodeSession } from "../../lib/node-session";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it("refreshes colors after external certification, skips unchanged/hidden polls and stops on unmount", async () => {
  vi.useFakeTimers();
  const document = { hidden: false, documentElement: { dataset: { theme: "light" } } };
  vi.stubGlobal("document", document);
  const revision = vi.spyOn(api, "graphRevision").mockResolvedValue({ graph: "g1", lean: 1 });
  const sync = vi.spyOn(nodeSession, "syncView").mockResolvedValue(true);
  const controller = new WorkspaceController({});
  const stop = controller.watchStatuses();
  await vi.advanceTimersByTimeAsync(0);
  expect(sync).toHaveBeenCalledTimes(1);
  const initial = controller.graphRevision;
  await vi.advanceTimersByTimeAsync(2000);
  expect(sync).toHaveBeenCalledTimes(1);
  revision.mockResolvedValue({ graph: "g1", lean: 2 });
  await vi.advanceTimersByTimeAsync(2000);
  expect(sync).toHaveBeenCalledTimes(2);
  expect(controller.graphRevision).toBe(initial + 1);
  document.hidden = true;
  const count = revision.mock.calls.length;
  await vi.advanceTimersByTimeAsync(4000);
  expect(revision).toHaveBeenCalledTimes(count);
  stop();
  expect(revision.mock.calls[0][0]?.aborted).toBe(true);
  document.hidden = false;
  await vi.advanceTimersByTimeAsync(4000);
  expect(revision).toHaveBeenCalledTimes(count);
});

it("treats a successful empty-project refresh as an empty state", async () => {
  vi.stubGlobal("document", { documentElement: { dataset: { theme: "light" } } });
  vi.spyOn(api, "graphCheck").mockResolvedValue({ nodes: 0, edges: 0 });
  vi.spyOn(api, "roots").mockResolvedValue([]);
  vi.spyOn(api, "full").mockResolvedValue({ nodes: [], edges: [] });
  const controller = new WorkspaceController({});

  await controller.refreshView();

  expect(controller.refreshError).toBeNull();
  expect(controller.initialError).toContain("no nodes");
  expect(controller.workspaceSession.report).toEqual({ nodes: 0, edges: 0 });
  expect(controller.refreshing).toBe(false);
});
