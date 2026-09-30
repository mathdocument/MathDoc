import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { WorkspaceController } from "./controller";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
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
