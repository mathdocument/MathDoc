// Thin fetch wrapper around the JSON API exposed by `mdc serve`.
// All functions throw on network/parse errors; 4xx/5xx become Error with the
// server's { error: string } message.

import type {
  AddDepBody,
  BlockBody,
  DependencyCandidates,
  ErrorResponse,
  GraphCheckReport,
  GraphFull,
  GraphRootItem,
  NewNodeBody,
  NodeDetail,
  NodeInfo,
  NodeView,
  ResolveResponse,
  RmDepBody,
  TitleBody,
} from "./types";

export function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

export class ApiError extends Error {
  constructor(message: string, public readonly status: number, public readonly body: unknown) {
    super(message);
    this.name = "ApiError";
  }

  get isConflict(): boolean {
    return this.status === 409 || this.status === 412;
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
        ? String((body as ErrorResponse).error)
        : `HTTP ${resp.status}`;
    throw new ApiError(msg, resp.status, body);
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

function newNode(
  params: NewNodeBody & { parent_fnode: string },
  expectedRevision: string,
): Promise<NodeDetail>;
function newNode(
  params: NewNodeBody & { parent_fnode?: null },
): Promise<NodeDetail>;
function newNode(params: NewNodeBody, expectedRevision?: string): Promise<NodeDetail> {
  const create = (revision?: string) => req<NodeDetail>(`/api/node/new`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(revision ? { "if-match": `"${revision}"` } : {}),
    },
    body: JSON.stringify(params),
  });
  if (params.parent_fnode) {
    if (!expectedRevision) throw new Error("linked node creation requires a revision");
    return mutateNode(params.parent_fnode, expectedRevision, create);
  }
  return create();
}

export const api = {
  roots: () => req<GraphRootItem[]>("/api/graph/roots"),
  graphCheck: () => req<GraphCheckReport>("/api/graph/check"),
  refreshWorkspace: () => req<GraphCheckReport>("/api/workspace/refresh", { method: "POST" }),
  full: (signal?: AbortSignal) => req<GraphFull>("/api/graph/full", { signal }),
  search: (q: string, n = 200, signal?: AbortSignal) =>
    req<NodeInfo[]>(`/api/search?q=${encodeURIComponent(q)}&n=${n}`, { signal }),
  resolve: (ref: string) =>
    req<ResolveResponse>(`/api/resolve?ref=${encodeURIComponent(ref)}`),
  nodeView: (fnode: string) =>
    req<NodeView>(`/api/node/${encodeURIComponent(fnode)}/view`),
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
        headers: { "content-type": "application/json", "if-match": `"${revision}"` },
        body: JSON.stringify({ content } satisfies BlockBody),
      },
    )),
  deleteBlock: (fnode: string, srctype: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(
      `/api/node/${encodeURIComponent(fnode)}/block/${encodeURIComponent(srctype)}`,
      {
        method: "DELETE",
        headers: { "if-match": `"${revision}"` },
      },
    )),
  putTitle: (fnode: string, title: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/title`, {
      method: "PUT",
      headers: { "content-type": "application/json", "if-match": `"${revision}"` },
      body: JSON.stringify({ title } satisfies TitleBody),
    })),
  addDep: (fnode: string, depFnode: string, expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/add`, {
      method: "POST",
      headers: { "content-type": "application/json", "if-match": `"${revision}"` },
      body: JSON.stringify({ dep_fnode: depFnode } satisfies AddDepBody),
    })),
  rmDeps: (fnode: string, depFnodes: string[], expectedRevision: string) =>
    mutateNode(fnode, expectedRevision, (revision) => req<NodeDetail>(`/api/node/${encodeURIComponent(fnode)}/dep/rm`, {
      method: "POST",
      headers: { "content-type": "application/json", "if-match": `"${revision}"` },
      body: JSON.stringify({ dep_fnodes: depFnodes } satisfies RmDepBody),
    })),
  newNode,
};
