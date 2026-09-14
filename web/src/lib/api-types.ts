// Wire types for the versioned MathDoc web service.

export type NodeSummary = { fnode: string, title: string, broken: boolean, depth: number, };

export type FormalCodeStatus = "no_code" | "unverified" | "verified";

export type FormalizationStatus = { lean: FormalCodeStatus, rocq: FormalCodeStatus, };

export type DependencyCandidatesEmpty = { "kind": "no_match" } | { "kind": "excluded", source: number, existing_dependencies: number, invalid_or_duplicate: number, } | { "kind": "result_limit", available: number, };

export type DependencyCandidates = { nodes: Array<NodeSummary>, 
/**
 * `None` when `nodes` is non-empty; otherwise explains why no node was returned.
 */
empty: DependencyCandidatesEmpty | null, };

export type GraphRootItem = { fnode: string, title: string, component_size: number, broken: boolean, topo_depth: number, };

export type IssueKind = "Missing" | "Invalid";

export type GraphIssue = { kind: IssueKind, fnode: string, title: string, error: string, };

export type GraphCheckReport = { nodes: number, edges: number, missing: Array<GraphIssue>, invalid: Array<GraphIssue>, cycles: Array<Array<string>>, };

export type SrcBlock = { srctype: string, content: string, metadata: { [key in string]: string }, };

export type NodeDetail = { module?: string, fnode: string, title: string, broken: boolean, depth: number, 
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
