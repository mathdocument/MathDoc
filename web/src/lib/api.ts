// Thin fetch wrapper around the JSON API exposed by the local mdc service.
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
import { projectPath } from "./project-path";

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

export async function fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, init);
  const text = await resp.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      if (resp.ok) throw new ApiError("Invalid JSON response", resp.status, text);
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

function req<T>(path: string, init?: RequestInit): Promise<T> {
  return fetchJson<T>(projectPath(path), init);
}

export interface ServiceStatus {
  server: { running: boolean; port: number | null; url: string | null };
  projects: Record<string, { running: boolean; url: string | null }>;
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

export interface LeanProject {
  toolchain: string; lakefile: string; manifest: string | null;
  lakefile_name?: "lakefile.toml" | "lakefile.lean";
  module_root?: string;
  files?: Record<string, string>;
}
export const api = {
  project: () => req<{ revision: string; project: LeanProject }>("/api/project/lean"),
  putProject: (project: LeanProject, revision: string) => req<{ revision: string; project: LeanProject }>("/api/project/lean", {
    method: "PUT", headers: { "content-type": "application/json", "if-match": `"${revision}"` }, body: JSON.stringify(project),
  }),
  leanSession: (fnode: string, revision: string) => req<{ id: string; filename: string; source: string }>(`/api/node/${encodeURIComponent(fnode)}/lean/session`, {
    method: "POST", headers: { "if-match": `"${revision}"` },
  }),
  closeLeanSession: (id: string) => req<void>(`/api/lean/session/${encodeURIComponent(id)}`, {
    method: "DELETE", keepalive: true,
  }),
  checkLean: (fnode: string, revision: string, build = false) => req<{ fnode: string; revision: string; passed: boolean; certified: boolean; has_sorry: boolean | null; built: boolean; cache_hit: boolean; elapsed_ms: number; diagnostics: unknown[]; dependency_errors: string[] }>(`/api/node/${encodeURIComponent(fnode)}/lean/check`, {
    method: "POST", headers: { "content-type": "application/json", "if-match": `"${revision}"` }, body: JSON.stringify({ build }),
  }),
  roots: () => req<GraphRootItem[]>("/api/graph/roots"),
  graphCheck: () => req<GraphCheckReport>("/api/graph/check"),
  full: (signal?: AbortSignal) => req<GraphFull>("/api/graph/full", { signal }),
  search: (q: string, n = 200, signal?: AbortSignal) =>
    req<NodeInfo[]>(`/api/search?q=${encodeURIComponent(q)}&n=${n}`, { signal }),
  resolve: (ref: string) =>
    req<ResolveResponse>(`/api/resolve?ref=${encodeURIComponent(ref)}`),
  nodeView: (fnode: string) =>
    req<NodeView>(`/api/node/${encodeURIComponent(fnode)}/view`),
  deleteNode: async (fnode: string, expectedRevision: string) => {
    const revision = await (nodeMutationTails.get(fnode) ?? Promise.resolve(expectedRevision));
    return req<{ fnode: string; deleted: true; removed_edges: number }>(`/api/node/${encodeURIComponent(fnode)}`, {
      method: "DELETE", headers: { "if-match": `"${revision}"` },
    });
  },
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
