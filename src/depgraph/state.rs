use std::collections::HashMap;
use std::path::Path;

use crate::core::{DependencyItem, GraphIssue, IssueKind};
use crate::mdocnode::MdocNode;
use crate::workspace::to_rel_path;

#[derive(Default)]
pub(super) struct GraphState {
    pub(super) root_fnode: String,
    pub(super) dep_graph: HashMap<String, Vec<String>>,
    pub(super) nodes_by_fnode: HashMap<String, MdocNode>,
    pub(super) broken_issues: HashMap<String, GraphIssue>,
}

impl GraphState {
    pub fn is_broken(&self, fnode: &str) -> bool {
        self.broken_issues.contains_key(fnode)
    }

    pub fn mark_missing(&mut self, fnode: &str) {
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
        if !(issue.fnode.starts_with('<') && issue.fnode.ends_with('>')) {
            self.nodes_by_fnode.remove(&issue.fnode);
            self.dep_graph.entry(issue.fnode.clone()).or_default();
            self.broken_issues.insert(issue.fnode.clone(), issue);
        }
    }

    pub fn clear_broken(&mut self, fnode: &str) {
        self.broken_issues.remove(fnode);
    }

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
