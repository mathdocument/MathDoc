use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::core::{DependencyItem, GraphIssue, IssueKind};
use crate::mdoc::MdocNode;
use crate::workspace::to_rel_path;

pub struct GraphState {
    pub root_fnode: String,
    pub dep_graph: HashMap<String, Vec<String>>,
    pub nodes_by_fnode: HashMap<String, MdocNode>,
    pub missing_fnodes: HashSet<String>,
    pub invalid_fnodes: HashSet<String>,
    pub broken_issues: HashMap<String, GraphIssue>,
    pub invalid_file_issues: Vec<GraphIssue>,
    pub scanned_file_count: usize,
}

impl Default for GraphState {
    fn default() -> Self {
        GraphState {
            root_fnode: String::new(),
            dep_graph: HashMap::new(),
            nodes_by_fnode: HashMap::new(),
            missing_fnodes: HashSet::new(),
            invalid_fnodes: HashSet::new(),
            broken_issues: HashMap::new(),
            invalid_file_issues: Vec::new(),
            scanned_file_count: 0,
        }
    }
}

impl GraphState {
    pub fn reset(&mut self) {
        self.dep_graph.clear();
        self.nodes_by_fnode.clear();
        self.missing_fnodes.clear();
        self.invalid_fnodes.clear();
        self.broken_issues.clear();
        self.invalid_file_issues.clear();
        self.scanned_file_count = 0;
    }

    pub fn is_broken(&self, fnode: &str) -> bool {
        self.missing_fnodes.contains(fnode) || self.invalid_fnodes.contains(fnode)
    }

    pub fn mark_missing(&mut self, fnode: &str) {
        self.missing_fnodes.insert(fnode.to_string());
        self.invalid_fnodes.remove(fnode);
        self.nodes_by_fnode.remove(fnode);
        self.dep_graph.entry(fnode.to_string()).or_default();
        self.broken_issues.insert(
            fnode.to_string(),
            GraphIssue {
                kind: IssueKind::Missing,
                fnode: fnode.to_string(),
                title: "<missing>".to_string(),
                rel_path: "<unknown>".to_string(),
                error: format!("no mdoc matched reference: {fnode}"),
            },
        );
    }

    pub fn record_invalid(&mut self, issue: GraphIssue) {
        upsert_issue(&mut self.invalid_file_issues, issue.clone());
        if !(issue.fnode.starts_with('<') && issue.fnode.ends_with('>')) {
            self.invalid_fnodes.insert(issue.fnode.clone());
            self.missing_fnodes.remove(&issue.fnode);
            self.nodes_by_fnode.remove(&issue.fnode);
            self.dep_graph.entry(issue.fnode.clone()).or_default();
            self.broken_issues.insert(issue.fnode.clone(), issue);
        }
    }

    pub fn clear_broken(&mut self, fnode: &str) {
        self.missing_fnodes.remove(fnode);
        self.invalid_fnodes.remove(fnode);
        self.broken_issues.remove(fnode);
    }

    /// Build a `DependencyItem` for `fnode` using the current state.
    pub fn dependency_item(&self, fnode: &str, depth: u32, mdcroot: &Path) -> DependencyItem {
        if let Some(node) = self.nodes_by_fnode.get(fnode) {
            return DependencyItem {
                depth,
                fnode: node.fnode.clone(),
                title: node.title.clone(),
                rel_path: to_rel_path(mdcroot, &node.path),
            };
        }
        if let Some(issue) = self.broken_issues.get(fnode) {
            return DependencyItem {
                depth,
                fnode: issue.fnode.clone(),
                title: issue.title.clone(),
                rel_path: issue.rel_path.clone(),
            };
        }
        DependencyItem {
            depth,
            fnode: fnode.to_string(),
            title: "<missing>".to_string(),
            rel_path: "<unknown>".to_string(),
        }
    }
}

// ── Issue helpers ─────────────────────────────────────────────────────────────

fn upsert_issue(issues: &mut Vec<GraphIssue>, issue: GraphIssue) {
    for existing in issues.iter_mut() {
        if existing.kind == issue.kind
            && existing.fnode == issue.fnode
            && existing.rel_path == issue.rel_path
        {
            *existing = issue;
            return;
        }
    }
    issues.push(issue);
}

pub(super) fn make_invalid_issue(
    mdcroot: &Path,
    path: &Path,
    error: &str,
    fnode: &str,
) -> GraphIssue {
    GraphIssue {
        kind: IssueKind::Invalid,
        fnode: fnode.to_string(),
        title: "<invalid>".to_string(),
        rel_path: to_rel_path(mdcroot, path),
        error: error.to_string(),
    }
}
