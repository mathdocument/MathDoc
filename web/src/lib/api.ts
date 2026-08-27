// Thin fetch wrapper around the JSON API exposed by `mdc serve`.
// All functions throw on network/parse errors; 4xx/5xx become Error with the
// server's { error: string } message.

import type {
  DependencyCandidates,
  GraphCheckReport,
  GraphFull,
  GraphRootItem,
  NodeDetail,
  NodeInfo,
  NodeView,
  ResolveResponse,
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

export function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
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

const nodeMutationTails = new Map<string, Promise<string>>();

function mutateNode(
  fnode: string,
  expectedRevision: string,
  operation: (revision: string) => Promise<NodeDetail>,
): Promise<NodeDetail> {
  const previous = nodeMutationTails.get(fnode) ?? Promise.resolve(expectedRevision);
  let attemptedRevision = expectedRevision;
  const response = previous.then((revision) => {
    attemptedRevision = revision;
    return operation(revision);
  });
  const tail = response.then((node) => node.revision, () => attemptedRevision);
  nodeMutationTails.set(fnode, tail);
  void tail.finally(() => {
    if (nodeMutationTails.get(fnode) === tail) nodeMutationTails.delete(fnode);
  });
  return response;
}

export const api = {
  roots: () => req<GraphRootItem[]>("/api/graph/roots"),
  graphCheck: () => req<GraphCheckReport>("/api/graph/check"),
  full: (fresh = false, signal?: AbortSignal) =>
    req<GraphFull>(`/api/graph/full${fresh ? "?fresh=true" : ""}`, { signal }),
  search: (q: string, n = 200, signal?: AbortSignal) =>
    req<NodeInfo[]>(`/api/search?q=${encodeURIComponent(q)}&n=${n}`, { signal }),
  resolve: (ref: string) =>
    req<ResolveResponse>(`/api/resolve?ref=${encodeURIComponent(ref)}`),
  node: (fnode: string, fresh = false) =>
    req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}${fresh ? "?fresh=true" : ""}`),
  nodeView: (fnode: string, fresh = false) =>
    req<NodeView>(
      `/api/node/${encodeURIComponent(fnode)}/view${fresh ? "?fresh=true" : ""}`,
    ),
  children: (fnode: string) =>
    req<NodeInfo[]>(`/api/node/${encodeURIComponent(fnode)}/children`),
  dependencyCandidates: (fnode: string, q: string, n = 50, signal?: AbortSignal) =>
    req<DependencyCandidates>(
      `/api/node/${encodeURIComponent(fnode)}/dep/candidates?q=${encodeURIComponent(q)}&n=${n}`,
      { signal },
    ),
  putBlock: (fnode: string, srctype: string, content: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(
      `/api/node/${encodeURIComponent(fnode)}/block/${encodeURIComponent(srctype)}`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ content, expected_revision: revision }),
      },
    )),
  deleteBlock: (fnode: string, srctype: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(
      `/api/node/${encodeURIComponent(fnode)}/block/${encodeURIComponent(srctype)}`,
      {
        method: "DELETE",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ expected_revision: revision }),
      },
    )),
  putTitle: (fnode: string, title: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/title`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ title, expected_revision: revision }),
    })),
  addDep: (fnode: string, depFnode: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, () => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/add`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ dep_fnode: depFnode }),
    })),
  rmDeps: (fnode: string, depFnodes: string[], expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, () => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/rm`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ dep_fnodes: depFnodes }),
    })),
  newNode: (
    params: { title: string; file?: string; parent_fnode?: string },
    expectedRevision?: string,
  ) => {
    const create = () => req<NodeDetail>(`/api/node/new`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(params),
    });
    return params.parent_fnode && expectedRevision
      ? mutateNode(params.parent_fnode, expectedRevision, create)
      : create();
  },
};
