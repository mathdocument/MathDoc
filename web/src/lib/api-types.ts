// Wire types for the versioned MathDoc web service.

export type NodeSummary = { fnode: string, title: string, depth: number, };

/** The local check. No local Lean runs any more, so Lean code is "unverified" here and its
 * standing comes from `lean_certification`; "verified" remains in the type for old data. */
export type FormalCodeStatus = "no_code" | "unverified" | "verified";

/**
 * Formal certification from LeanGround (migration plan §7.1), separate from any local check.
 * `not_submitted`: the node is not bound to a proof request yet. `insufficient` with
 * `truncated` means the search stopped early, not that no route exists. `stale`: the Lean
 * text or a definition it uses changed after submission. `rejected`: the node could not be
 * converted or LeanGround refused it (`reason`).
 */
export type LeanCertification =
  | { status: "not_submitted" }
  | {
      status: "insufficient" | "derivable" | "certified" | "stale" | "rejected",
      project: string, goal?: string, minimum_trust: string, checked_at: string | null,
      truncated: boolean, reason?: string, details?: string,
    };

/** `lean`/`rocq`: the local check. `lean_certification`: the LeanGround dimension. */
export type FormalizationStatus = { lean: FormalCodeStatus, rocq: FormalCodeStatus, lean_certification?: LeanCertification | null, };

export type DependencyCandidatesEmpty = { "kind": "no_match" } | { "kind": "excluded", source: number, existing_dependencies: number, } | { "kind": "result_limit", available: number, };

export type DependencyCandidates = { nodes: Array<NodeSummary>,
/**
 * `None` when `nodes` is non-empty; otherwise explains why no node was returned.
 */
empty: DependencyCandidatesEmpty | null, };

export type GraphRootItem = { fnode: string, title: string, component_size: number, topo_depth: number, };

export type GraphCheckReport = { nodes: number, edges: number, };

export type SrcBlock = { srctype: string, content: string, metadata: { [key in string]: string }, };

export type NodeDetail = { module?: string, fnode: string, title: string, depth: number,
/**
 * Revision of the database node represented by this response.
 */
revision: string,
/**
 * Direct dependency fnodes (in source order, deduplicated).
 */
depens: Array<string>, blocks: Array<SrcBlock>, formalization: FormalizationStatus, };

export type NodePreview = NodeSummary & { formalization: FormalizationStatus };

export type NodeView = { node: NodeDetail, referrers: Array<NodePreview>, children: Array<NodePreview>, };

export type ResolveResponse = { fnode: string, title: string, };

export type GraphFull = { nodes: Array<NodeSummary & { lean: FormalCodeStatus }>, edges: Array<[number, number]>, };

export type SearchQuery = { q: string, n?: number, };

export type ResolveQuery = { ref: string, };

export type BlockBody = { content: string, };

export type TitleBody = { title: string, };

export type AddDepBody = { dep_fnode: string, };

export type RmDepBody = { dep_fnodes: Array<string>, };

export type NewNodeBody = { title: string,
/**
 * If set, the new node is added as a direct dependency of this node.
 */
parent_fnode?: string | null, };

export type ErrorResponse = { error: string, };
