import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import { projectName, projectPath } from "./project-path";

const jsonResponse = (revision: string) => Response.json({ revision });

afterEach(() => {
  vi.unstubAllGlobals();
});

it("routes node requests and Lean pages inside the selected database and branch", async () => {
  vi.stubGlobal("location", { pathname: "/p/mathlib4/agent-one/lean.html" });
  const fetchMock = vi.fn<typeof fetch>(() => Promise.resolve(Response.json({})));
  vi.stubGlobal("fetch", fetchMock);
  await api.nodeView("same-uuid");
  expect(fetchMock.mock.calls[0]![0]).toBe("/p/mathlib4/agent-one/api/node/same-uuid/view");
  expect(projectPath("/lean.html?session=one")).toBe("/p/mathlib4/agent-one/lean.html?session=one");
  expect(projectName("/p/db/main-other/")).toBe("db/main-other");
  expect(projectName("/p/db/../main/")).toBeNull();
  expect(projectName("/")).toBeNull();
});

describe("node mutations", () => {
  it("serializes writes and carries the committed revision forward", async () => {
    let finishFirst!: () => void;
    const fetchMock = vi.fn()
      .mockImplementationOnce(() => new Promise<Response>((resolve) => {
        finishFirst = () => resolve(jsonResponse("revision-2"));
      }))
      .mockResolvedValueOnce(jsonResponse("revision-3"));
    vi.stubGlobal("fetch", fetchMock);

    const first = api.putTitle("queue-node", "First", "revision-1");
    const second = api.putBlock("queue-node", "text", "Second", "revision-1");
    await Promise.resolve();
    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    finishFirst();
    await Promise.all([first, second]);

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const firstHeaders = fetchMock.mock.calls[0]![1]!.headers as Record<string, string>;
    const secondHeaders = fetchMock.mock.calls[1]![1]!.headers as Record<string, string>;
    expect(firstHeaders["if-match"]).toBe('"revision-1"');
    expect(secondHeaders["if-match"]).toBe('"revision-2"');
  });

  it("preserves the latest revision after a queued write fails", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(jsonResponse("revision-2"))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: "rejected" }), {
        status: 422,
        headers: { "content-type": "application/json" },
      }))
      .mockResolvedValueOnce(jsonResponse("revision-3"));
    vi.stubGlobal("fetch", fetchMock);

    const first = api.putTitle("failure-queue-node", "First", "revision-1");
    const second = api.putTitle("failure-queue-node", "Rejected", "revision-1");
    const third = api.putBlock("failure-queue-node", "text", "Third", "revision-1");
    const results = await Promise.allSettled([first, second, third]);

    expect(results.map((result) => result.status)).toEqual(["fulfilled", "rejected", "fulfilled"]);
    const thirdHeaders = fetchMock.mock.calls[2]![1]!.headers as Record<string, string>;
    expect(thirdHeaders["if-match"]).toBe('"revision-2"');
  });

  it("sends preconditions for relationship mutations", async () => {
    const fetchMock = vi.fn<typeof fetch>(() => Promise.resolve(jsonResponse("revision-2")));
    vi.stubGlobal("fetch", fetchMock);

    await api.addDep("parent", "child", "revision-1");
    await api.newNode({ title: "Child", parent_fnode: "parent" }, "revision-1");

    for (const call of fetchMock.mock.calls) {
      const headers = call[1]!.headers as Record<string, string>;
      expect(headers["if-match"]).toBeDefined();
    }
  });
});

it("preserves the status and structured body of a revision conflict", async () => {
  const body = { error: "resource changed; refresh and retry" };
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json(body, { status: 412 })));
  await expect(api.putTitle("conflicted-node", "Title", "old"))
    .rejects.toMatchObject({ name: "ApiError", status: 412, body, isConflict: true });
});
