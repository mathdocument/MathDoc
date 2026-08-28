use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct NodeSummary {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub broken: bool,
    pub depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDegrees {
    pub in_degree: u32,
    pub out_degree: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FormalCodeStatus {
    #[default]
    NoCode,
    Unverified,
    Verified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct FormalizationStatus {
    pub lean: FormalCodeStatus,
    pub rocq: FormalCodeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DependencyCandidates {
    pub nodes: Vec<NodeSummary>,
    /// `None` when `nodes` is non-empty; otherwise explains why no node was returned.
    pub empty: Option<DependencyCandidatesEmpty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DependencyCandidatesEmpty {
    NoMatch,
    /// Disjoint counts using source, existing dependency, then health precedence.
    Excluded {
        source: usize,
        existing_dependencies: usize,
        invalid_or_duplicate: usize,
    },
    ResultLimit {
        available: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyItem {
    pub depth: u32,
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GraphRootItem {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub component_size: u32,
    pub broken: bool,
    pub topo_depth: u32,
}

/// Issue kind surfaced through the API.
/// "duplicate" and "broken" are internal DB/depgraph states; both map to Invalid here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum IssueKind {
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GraphIssue {
    pub kind: IssueKind,
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GraphCheckReport {
    pub nodes: u32,
    pub edges: u32,
    pub missing: Vec<GraphIssue>,
    pub invalid: Vec<GraphIssue>,
    pub cycles: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct DependencyTraversalReport {
    pub items: Vec<DependencyItem>,
    pub issues_by_fnode: HashMap<String, GraphIssue>,
    /// Cycles detected in the traversed subgraph. Each cycle is [A, B, ..., A].
    pub cycles: Vec<Vec<String>>,
}
