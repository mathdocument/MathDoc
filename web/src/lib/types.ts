// Shared API types — mirror the Rust DTOs in src/web/api.rs.
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

export interface GraphRootItem {
  fnode: string;
  title: string;
  rel_path: string;
  component_size: number;
  broken: boolean;
  topo_depth: number;
}
