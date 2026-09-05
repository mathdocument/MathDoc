//! Batched document use cases. One lock, strong refresh, and index update per batch.

use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::indcache::WorkspaceStore;
use crate::mdocnode::MdocNode;
use crate::workspace::FileSnapshot;

pub struct NewNode {
    pub file: String,
    pub title: String,
    pub fnode: Option<String>,
}

pub enum NodeChange {
    SetTitle(String),
    UpsertBlock {
        srctype: String,
        content: String,
    },
    RemoveBlock(String),
    /// Targets are workspace references (paths, fnodes, or unique prefixes).
    AddDependencies(Vec<String>),
    /// Targets are exact fnodes; dangling dependencies can also be removed.
    RemoveDependencies(Vec<String>),
}

pub struct NodeEdit {
    pub reference: String,
    /// Exact content revision observed by the caller. Omit for CLI-style edits.
    pub expected_revision: Option<String>,
    pub changes: Vec<NodeChange>,
}

pub fn create_nodes(store: &mut WorkspaceStore, requests: &[NewNode]) -> Result<Vec<MdocNode>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let lock = store.acquire_mutation_lock()?;
    store.refresh_all()?;
    let mut fnodes = HashSet::new();
    let mut nodes = Vec::with_capacity(requests.len());
    for request in requests {
        let node = crate::depgraph::prepare_new_node(
            store.root(),
            &request.file,
            &request.title,
            request.fnode.as_deref(),
        )?;
        if !fnodes.insert(node.fnode.clone())
            || !store.reconcile_fnode_paths(&node.fnode)?.is_empty()
        {
            bail!(
                "fnode {} is already used by another file in this workspace",
                node.fnode
            );
        }
        nodes.push((node, FileSnapshot::Missing));
    }
    store.persist_node_batch(&lock, &nodes)?;
    Ok(nodes.into_iter().map(|(node, _)| node).collect())
}

pub fn edit_nodes(
    store: &mut WorkspaceStore,
    requests: &[NodeEdit],
    cwd: Option<&Path>,
) -> Result<Vec<MdocNode>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let lock = store.acquire_mutation_lock()?;
    store.refresh_all()?;
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for (source, target) in store.all_valid_edges()? {
        graph.entry(source).or_default().push(target);
    }
    crate::workspace::run_test_hook(crate::workspace::TestHookPoint::BatchAfterRefresh);
    let mut indexes = HashMap::new();
    let mut original_render = HashMap::new();
    let mut nodes: Vec<(MdocNode, FileSnapshot)> = Vec::new();
    for request in requests {
        let (fnode, _, path) = store.resolve_ref(&request.reference, cwd)?;
        if store.issue_for_fnode(&fnode)?.is_some() {
            bail!("node must be valid and unique: {fnode}");
        }
        let index = if let Some(index) = indexes.get(&fnode).copied() {
            index
        } else {
            let snapshot = FileSnapshot::capture_beneath(store.root(), &path)?;
            let bytes = snapshot
                .content()
                .ok_or_else(|| anyhow::anyhow!("node disappeared: {fnode}"))?;
            let node = MdocNode::load_bytes(&path, bytes)?;
            if node.fnode != fnode {
                bail!("node identity changed: {fnode}");
            }
            graph.insert(fnode.clone(), node.depens.clone());
            original_render.insert(fnode.clone(), node.render()?);
            let index = nodes.len();
            nodes.push((node, snapshot));
            indexes.insert(fnode.clone(), index);
            index
        };
        let (node, snapshot) = &mut nodes[index];
        if request.expected_revision.as_ref().is_some_and(|expected| {
            expected != &crate::mdocnode::content_revision(snapshot.content().unwrap_or_default())
        }) {
            return Err(crate::mdocnode::RevisionMismatch.into());
        }
        for change in &request.changes {
            match change {
                NodeChange::AddDependencies(references) => {
                    for reference in references {
                        let (target, _, path) = store.resolve_ref(reference, cwd)?;
                        let target_node = MdocNode::load(&path)?;
                        if store.issue_for_fnode(&target)?.is_some() || target_node.fnode != target
                        {
                            bail!("dependency target must be valid and unique: {target}");
                        }
                        if !indexes.contains_key(&target) {
                            graph.insert(target.clone(), target_node.depens);
                        }
                        if node.depens.contains(&target) {
                            continue;
                        }
                        if target == fnode || reaches(&graph, &target, &fnode) {
                            bail!(
                                "adding {target} as a dependency of {fnode} would create a cycle"
                            );
                        }
                        node.add_dependency(&target);
                        graph.entry(fnode.clone()).or_default().push(target);
                    }
                }
                NodeChange::RemoveDependencies(targets) => {
                    for target in targets {
                        node.remove_dependency(target);
                    }
                    graph.insert(fnode.clone(), node.depens.clone());
                }
                _ => apply_content_change(node, change)?,
            }
        }
    }
    // Preserve exact bytes, inode generations, and revisions for no-op requests.
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    for (node, snapshot) in nodes {
        if original_render[&node.fnode] == node.render()? {
            if !snapshot.unchanged_beneath(store.root(), &node.path)? {
                bail!("node changed during batch validation: {}", node.fnode);
            }
            unchanged.push(node);
        } else {
            changed.push((node, snapshot));
        }
    }
    store.persist_node_batch(&lock, &changed)?;
    let mut result = changed
        .into_iter()
        .map(|(node, _)| node)
        .chain(unchanged)
        .collect::<Vec<_>>();
    result.sort_by_key(|node| indexes[&node.fnode]);
    Ok(result)
}

pub(crate) fn apply_content_change(node: &mut MdocNode, change: &NodeChange) -> Result<()> {
    match change {
        NodeChange::SetTitle(title) => {
            let title = title.trim();
            if title.is_empty() {
                bail!("@title must be non-empty");
            }
            node.set_title(title.to_string());
        }
        NodeChange::UpsertBlock { srctype, content } => {
            node.upsert_source_block(srctype, content.clone())?
        }
        NodeChange::RemoveBlock(srctype) => {
            let srctype = crate::config::builtin_srctype(srctype)?;
            if !node.remove_source_block(srctype) {
                bail!("no '@src: {srctype}' block on this node");
            }
        }
        _ => bail!("dependency changes require graph validation"),
    }
    Ok(())
}

fn reaches(graph: &HashMap<String, Vec<String>>, from: &str, target: &str) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if seen.insert(node) {
            if let Some(dependencies) = graph.get(node) {
                stack.extend(dependencies.iter().map(String::as_str));
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_edges_changed_after_refresh_are_included_in_cycle_validation() {
        let dir = tempfile::tempdir().unwrap();
        crate::workspace::initialize(dir.path()).unwrap();
        let mut store = WorkspaceStore::open(dir.path().to_path_buf()).unwrap();
        let seeds = ["a", "b"].map(|name| NewNode {
            file: name.into(),
            title: name.into(),
            fnode: Some(name.into()),
        });
        create_nodes(&mut store, &seeds).unwrap();
        let path = dir.path().join("b.mdoc");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::BatchAfterRefresh,
            move || {
                let mut external = MdocNode::load(&path).unwrap();
                external.add_dependency("a");
                std::fs::write(&path, external.render().unwrap()).unwrap();
            },
        );
        let error = edit_nodes(
            &mut store,
            &[NodeEdit {
                reference: "a".into(),
                expected_revision: None,
                changes: vec![NodeChange::AddDependencies(vec!["b".into()])],
            }],
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cycle"));
        assert!(MdocNode::load(&dir.path().join("a.mdoc"))
            .unwrap()
            .depens
            .is_empty());
    }

    #[test]
    fn committed_batch_survives_a_replaced_mutation_lock() {
        let dir = tempfile::tempdir().unwrap();
        crate::workspace::initialize(dir.path()).unwrap();
        let mut store = WorkspaceStore::open(dir.path().to_path_buf()).unwrap();
        let lock_path = dir.path().join(".mdc/mutation.lock");
        let displaced = dir.path().join("old-lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::IndexAfterNodeUpsert,
            move || {
                std::fs::rename(&lock_path, displaced).unwrap();
                std::fs::write(lock_path, []).unwrap();
            },
        );
        let seeds = ["a", "b"].map(|name| NewNode {
            file: name.into(),
            title: name.into(),
            fnode: Some(name.into()),
        });
        assert!(create_nodes(&mut store, &seeds).is_err());
        assert_eq!(store.count().unwrap(), 2);
        for name in ["a", "b"] {
            assert_eq!(
                MdocNode::load(&dir.path().join(format!("{name}.mdoc")))
                    .unwrap()
                    .fnode,
                name
            );
        }
        assert!(store.index_is_dirty().unwrap());
        drop(store);
        let recovered = WorkspaceStore::open(dir.path().to_path_buf()).unwrap();
        assert!(!recovered.index_is_dirty().unwrap());
        assert_eq!(recovered.count().unwrap(), 2);
    }

    #[test]
    fn failed_later_write_rolls_back_earlier_files_and_preserves_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        crate::workspace::initialize(dir.path()).unwrap();
        let mut store = WorkspaceStore::open(dir.path().to_path_buf()).unwrap();
        let seeds = ["a", "b"].map(|name| NewNode {
            file: name.into(),
            title: name.into(),
            fnode: Some(name.into()),
        });
        create_nodes(&mut store, &seeds).unwrap();
        let b = dir.path().join("b.mdoc");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteAfterPersistence,
            move || {
                std::fs::write(b, "@fnode: b\n@title: External B\n").unwrap();
            },
        );
        let edits = ["a", "b"].map(|name| NodeEdit {
            reference: name.into(),
            expected_revision: None,
            changes: vec![NodeChange::SetTitle(format!("Changed {name}"))],
        });
        assert!(edit_nodes(&mut store, &edits, None).is_err());
        assert_eq!(
            MdocNode::load(&dir.path().join("a.mdoc")).unwrap().title,
            "a"
        );
        assert_eq!(store.node_summary("a").unwrap().title, "a");
        assert_eq!(store.node_summary("b").unwrap().title, "External B");
        assert!(!store.index_is_dirty().unwrap());
    }
}
