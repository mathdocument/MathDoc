//! BFS traversal over in-memory graph state to produce dependency item lists.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use crate::core::DependencyItem;

use super::state::GraphState;

/// BFS from `root_fnode` over `state.dep_graph`.
/// If `leaf_only`, only nodes with no outgoing edges in the graph are returned.
pub fn dependency_items_from_graph(
    state: &GraphState,
    root_fnode: &str,
    mdcroot: &Path,
    leaf_only: bool,
) -> Vec<DependencyItem> {
    let mut items: Vec<DependencyItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, u32)> = state
        .dep_graph
        .get(root_fnode)
        .into_iter()
        .flatten()
        .map(|f| (f.clone(), 1u32))
        .collect();

    while let Some((fnode, depth)) = queue.pop_front() {
        if !seen.insert(fnode.clone()) {
            continue;
        }
        let dep_fnodes = state.dep_graph.get(&fnode);
        let has_deps = dep_fnodes.map(|d| !d.is_empty()).unwrap_or(false);

        if leaf_only && has_deps {
            for dep in dep_fnodes.into_iter().flatten() {
                queue.push_back((dep.clone(), depth + 1));
            }
            continue;
        }

        items.push(state.dependency_item(&fnode, depth, mdcroot));

        if !leaf_only {
            for dep in dep_fnodes.into_iter().flatten() {
                queue.push_back((dep.clone(), depth + 1));
            }
        }
    }
    items
}
