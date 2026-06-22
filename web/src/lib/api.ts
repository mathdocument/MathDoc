// Thin fetch wrapper around the JSON API exposed by `mdc serve`.
// All functions throw on network/parse errors; 4xx/5xx become Error with the
// server's { error: string } message.

import type {
  GraphRootItem,
  NodeDetail,
  NodeInfo,
} from "./types";

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, init);
  const text = await resp.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      body = text;
    }
  }
  if (!resp.ok) {
    const msg =
      typeof body === "object" && body !== null && "error" in body
        ? String((body as { error: unknown }).error)
        : `HTTP ${resp.status}`;
    throw new ApiError(msg, resp.status);
  }
  return body as T;
}

export const api = {
  roots: () => req<GraphRootItem[]>("/api/graph/roots"),
  full: () =>
    req<{ nodes: NodeInfo[]; edges: { source: string; target: string }[] }>(
      "/api/graph/full",
    ),
  search: (q: string, n = 200) =>
    req<NodeInfo[]>(`/api/search?q=${encodeURIComponent(q)}&n=${n}`),
  resolve: (ref: string) =>
    req<{ fnode: string; title: string; rel_path: string }>(
      `/api/resolve?ref=${encodeURIComponent(ref)}`,
    ),
  node: (fnode: string) => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}`),
  referrers: (fnode: string) =>
    req<NodeInfo[]>(`/api/node/${encodeURIComponent(fnode)}/referrers`),
  children: (fnode: string) =>
    req<NodeInfo[]>(`/api/node/${encodeURIComponent(fnode)}/children`),
  putBlock: (fnode: string, srctype: string, content: string) =>
    req<NodeDetail>(
      `/api/node/${encodeURIComponent(fnode)}/block/${encodeURIComponent(srctype)}`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ content }),
      },
    ),
  deleteBlock: (fnode: string, srctype: string) =>
    req<NodeDetail>(
      `/api/node/${encodeURIComponent(fnode)}/block/${encodeURIComponent(srctype)}`,
      { method: "DELETE" },
    ),
  putTitle: (fnode: string, title: string) =>
    req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/title`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ title }),
    }),
  addDep: (fnode: string, depFnode: string) =>
    req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/add`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ dep_fnode: depFnode }),
    }),
  rmDeps: (fnode: string, depFnodes: string[]) =>
    req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/rm`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ dep_fnodes: depFnodes }),
    }),
  newNode: (params: { title: string; file?: string; parent_fnode?: string }) =>
    req<NodeDetail>(`/api/node/new`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(params),
    }),
};
