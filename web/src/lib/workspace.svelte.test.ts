import { afterEach, expect, it, vi } from "vitest";
import { api } from "./api";
import { WorkspaceSession } from "./workspace.svelte";
import type { GraphCheckReport } from "./types";

const report = (nodes: number): GraphCheckReport => ({
  nodes, edges: 0, missing: [], invalid: [], cycles: [],
});
afterEach(() => vi.restoreAllMocks());

it("does not let an older refresh overwrite a newer report", async () => {
  let resolveOld!: (value: GraphCheckReport) => void;
  vi.spyOn(api, "graphCheck")
    .mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }))
    .mockResolvedValueOnce(report(2));
  const session = new WorkspaceSession();
  const old = session.refresh();
  expect(await session.refresh()).toBe(true);
  resolveOld(report(1));
  expect(await old).toBe(false);
  expect(session.report?.nodes).toBe(2);
  expect(session.loading).toBe(false);
});

it("preserves mutation counts when an in-flight graph check finishes", async () => {
  let finish!: (value: GraphCheckReport) => void;
  vi.spyOn(api, "graphCheck").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const session = new WorkspaceSession();
  session.report = report(1);
  const pending = session.refresh();
  session.applyDelta(1, 1);
  finish(report(1));
  expect(await pending).toBe(false);
  expect(session.report).toMatchObject({ nodes: 2, edges: 1 });
  expect(session.stale).toBe(true);
});
