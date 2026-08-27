// Shared API types mirror the Rust API DTOs. NodeInfo matches core::NodeSummary.
// Keep field names in sync when adding/removing endpoints.

export interface NodeInfo {
  fnode: string;
  title: string;
  rel_path: string;
  broken: boolean;
  depth: number;
}

export interface DependencyCandidates {
  nodes: NodeInfo[];
  empty: DependencyCandidatesEmpty | null;
}

export type DependencyCandidatesEmpty =
  | { kind: "no_match" }
  | {
      kind: "excluded";
      source: number;
      existing_dependencies: number;
      invalid_or_duplicate: number;
    }
  | { kind: "result_limit"; available: number };

export interface SrcBlock {
  srctype: string;
  content: string;
  metadata: Record<string, string>;
}

export type FormalCodeStatus = "no_code" | "unverified" | "verified";

export interface FormalizationStatus {
  lean: FormalCodeStatus;
  rocq: FormalCodeStatus;
}

export interface NodeDetail {
  fnode: string;
  title: string;
  rel_path: string;
  broken: boolean;
  depth: number;
  revision: string;
  depens: string[];
  blocks: SrcBlock[];
  formalization: FormalizationStatus;
}

export interface NodeView {
  node: NodeDetail;
  referrers: NodeInfo[];
  children: NodeInfo[];
}

export interface ResolveResponse {
  fnode: string;
  title: string;
  rel_path: string;
}

export type GraphEdge = [sourceIndex: number, targetIndex: number];

export interface GraphFull {
  nodes: NodeInfo[];
  edges: GraphEdge[];
}

export interface GraphRootItem {
  fnode: string;
  title: string;
  rel_path: string;
  component_size: number;
  broken: boolean;
  topo_depth: number;
}

export interface GraphIssue {
  kind: "Missing" | "Invalid";
  fnode: string;
  title: string;
  rel_path: string;
  error: string;
}

export interface GraphCheckReport {
  nodes: number;
  edges: number;
  missing: GraphIssue[];
  invalid: GraphIssue[];
  cycles: string[][];
}
