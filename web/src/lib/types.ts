// Shared API types mirror the Rust API DTOs. NodeInfo matches core::NodeSummary.
// Keep field names in sync when adding/removing endpoints.

export interface NodeInfo {
  fnode: string;
  title: string;
  rel_path: string;
  broken: boolean;
  depth: number;
}

export interface SrcBlock {
  srctype: string;
  content: string;
  metadata: Record<string, string>;
}

export interface NodeDetail {
  fnode: string;
  title: string;
  rel_path: string;
  broken: boolean;
  depth: number;
  depens: string[];
  blocks: SrcBlock[];
}

export interface ResolveResponse {
  fnode: string;
  title: string;
  rel_path: string;
}

export interface GraphEdge {
  source: string;
  target: string;
}

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
