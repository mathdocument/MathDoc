// Wire types for the versioned MathDoc web service.

export type NodeSummary = { fnode: string, title: string, depth: number, };

/** The local check. No local Lean runs any more, so Lean code is "unverified" here and its
 * standing comes from `lean_certification`; "verified" remains in the type for old data. */
export type FormalCodeStatus = "no_code" | "unverified" | "verified";

/**
 * Formal certification from LeanGround (docs/mathdoc.md §4.5), separate from any local check.
 * `not_submitted`: the node is not bound to a proof request yet. `insufficient` with
 * `truncated` means the search stopped early, not that no route exists. `stale`: the Lean
 * text or a definition it uses changed after submission. `rejected`: the node could not be
 * converted or LeanGround refused it (`reason`). `registered`: a definition node whose
 * definitions LeanGround registered (`definition_ids`); definitions have no goal to prove.
 * `imported`: an external proof import's upstream file, unchanged (`path` at `commit`); the
 * import's project check covers it, so it is not submitted on its own.
 */
export type LeanCertification =
  | { status: "not_submitted" }
  | { status: "imported", module: string, path: string, commit: string }
  | {
      status: "insufficient" | "derivable" | "certified" | "stale" | "rejected" | "registered",
      /** `branch`: whose conversion of the node is shown (absent for stage 4 requests). */
      project: string, branch?: string, goal?: string, minimum_trust: string, checked_at: string | null,
      truncated: boolean, reason?: string, details?: string, definition_ids?: string[],
    };

/** Whether the node's Lean block is a definition (registered with LeanGround, nothing to prove)
 * or a theorem (a goal): as the shown conversion binds it, else as the block declares it. Null
 * without a Lean block, or when the block cannot be read. */
export type LeanRole = "definition" | "theorem";

/** `lean`/`rocq`: the local check. `lean_certification`: the LeanGround dimension. */
export type FormalizationStatus = { lean: FormalCodeStatus, rocq: FormalCodeStatus, lean_certification?: LeanCertification | null, lean_role?: LeanRole | null, };

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

/** `certification`: the status of `lean_certification`, null without a Lean block. */
export type GraphFull = { nodes: Array<NodeSummary & { lean: FormalCodeStatus, lean_role?: LeanRole | null, certification?: LeanCertification["status"] | null }>, edges: Array<[number, number]>, };

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
