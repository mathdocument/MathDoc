import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";

const jsonResponse = (revision: string) => Response.json({ revision });

afterEach(() => {
  vi.unstubAllGlobals();
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
