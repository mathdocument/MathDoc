use anyhow::{anyhow, bail, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::core::{short_fnode, DependencyCandidates, DependencyItem};
use crate::indcache::WorkspaceStore;
use crate::mdocnode::MdocNode;
use crate::workspace::to_rel_path;

// ── DepGraph ──────────────────────────────────────────────────────────────────

/// Dependency mutation session for one root node.
pub struct DepGraph<'cache> {
    cache: &'cache mut WorkspaceStore,
    root: MdocNode,
}

impl<'cache> DepGraph<'cache> {
    // ── Constructors ─────────────────────────────────────────────────────────

    /// Create a fresh `.mdoc` file and return a DepGraph rooted at it.
    ///
    /// `file_path`: relative path (without `.mdoc`) or `"."` for `{fnode}.mdoc` in root.
    pub fn create_root(
        cache: &'cache mut WorkspaceStore,
        file_path: &str,
        title: &str,
        fnode: Option<&str>,
    ) -> Result<Self> {
        let mutation_lock = cache.acquire_mutation_lock()?;
        Self::create_root_under_lock(cache, &mutation_lock, file_path, title, fnode)
    }

    pub(crate) fn create_root_under_lock(
        cache: &'cache mut WorkspaceStore,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        file_path: &str,
        title: &str,
        fnode: Option<&str>,
    ) -> Result<Self> {
        cache.validate_mutation_lock(mutation_lock)?;
        let root = cache.root().to_path_buf();
        let node = prepare_new_node(&root, file_path, title, fnode)?;

        cache.refresh_all()?;
        if !cache.reconcile_fnode_paths(&node.fnode)?.is_empty() {
            bail!(
                "fnode {} is already used by another file in this workspace",
                short_fnode(&node.fnode)
            );
        }

        let _receipt = cache.create_node(mutation_lock, &node)?;
        Ok(DepGraph { cache, root: node })
    }

    /// Load an existing `.mdoc` via `ref` (fnode, path, or fnode prefix) and build a DepGraph.
    pub fn from_ref(
        cache: &'cache mut WorkspaceStore,
        ref_str: &str,
        cwd: Option<&Path>,
    ) -> Result<Self> {
        let mutation_lock = cache.acquire_mutation_lock()?;
        Self::from_ref_under_lock(cache, &mutation_lock, ref_str, cwd)
    }

    pub(crate) fn from_ref_under_lock(
        cache: &'cache mut WorkspaceStore,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        ref_str: &str,
        cwd: Option<&Path>,
    ) -> Result<Self> {
        cache.validate_mutation_lock(mutation_lock)?;
        let root = cache.root().to_path_buf();
        let base_cwd = cwd
            .map(|c| c.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| root.clone()));
        let (resolved_fnode, _, src_path) = cache.resolve_ref(ref_str, Some(&base_cwd))?;
        // Ensure the resolved file is indexed before checking for duplicates.
        // Without this, a file resolved via filesystem fallback (not yet in the index)
        // would be invisible to reconcile_fnode_paths, allowing a silent bypass.
        cache.upsert_path(&src_path)?;
        let node = MdocNode::load(&src_path)?;
        if node.fnode != resolved_fnode {
            bail!(
                "mdoc identity changed while resolving '{}': expected {}, found {}",
                ref_str,
                resolved_fnode,
                node.fnode
            );
        }

        let dup_paths = cache.reconcile_fnode_paths(&node.fnode)?;
        if dup_paths.len() > 1 {
            bail!("{}", duplicate_fnode_error(&root, &node.fnode, &dup_paths));
        }

        Ok(DepGraph { cache, root: node })
    }

    // ── Root management ───────────────────────────────────────────────────────

    pub(crate) fn root_node(&self) -> &MdocNode {
        &self.root
    }

    pub fn root_item(&self) -> Result<DependencyItem> {
        Ok(DependencyItem {
            depth: 0,
            fnode: self.root.fnode.clone(),
            title: self.root.title.clone(),
            rel_path: to_rel_path(self.cache.root(), &self.root.path),
        })
    }

    // ── Issue queries ─────────────────────────────────────────────────────────

    pub fn is_broken_fnode(&self, fnode: &str) -> Result<bool> {
        if fnode == self.root.fnode {
            return Ok(false);
        }
        Ok(self.cache.issue_for_fnode(fnode)?.is_some())
    }

    pub fn ref_item_for_fnode(&self, fnode: &str, depth: u32) -> Result<DependencyItem> {
        if fnode == self.root.fnode {
            let mut item = self.root_item()?;
            item.depth = depth;
            return Ok(item);
        }
        self.cache.ref_item_for_fnode(fnode, depth)
    }

    // ── Dependency queries ────────────────────────────────────────────────────

    pub fn direct_dependency_items(&mut self) -> Result<Vec<DependencyItem>> {
        let fnodes = dedupe_keep_order(&self.root.depens);
        let fnode_refs: Vec<_> = fnodes.iter().map(String::as_str).collect();
        let paths_by_fnode = self.cache.reconcile_fnode_paths_many(&fnode_refs)?;
        let mut paths_to_upsert = Vec::with_capacity(fnodes.len());
        for fnode in &fnodes {
            if let Some([path]) = paths_by_fnode.get(fnode).map(Vec::as_slice) {
                paths_to_upsert.push(path.clone());
            }
        }
        self.cache.upsert_paths(&paths_to_upsert)?;
        self.cache.ref_items_for_fnodes(&fnodes, 1)
    }

    pub fn dependency_candidates(&self, query: &str, limit: usize) -> Result<DependencyCandidates> {
        self.cache
            .dependency_candidates(&self.root.fnode, query, limit)
    }

    /// Construct a new dependency node using the same path rules as standalone
    /// node creation. A preallocated fnode may be supplied by interactive clients.
    pub fn prepare_new_dependency_node(
        &self,
        file_path: &str,
        title: &str,
        fnode: Option<&str>,
    ) -> Result<MdocNode> {
        prepare_new_node(self.cache.root(), file_path, title, fnode)
    }

    pub fn resolve_direct_dependency_ref(
        &mut self,
        target_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<String> {
        let target_ref = target_ref.trim();
        if target_ref.is_empty() {
            bail!("target reference cannot be empty");
        }

        let direct_fnodes = dedupe_keep_order(&self.root.depens);
        if let Some(exact) = direct_fnodes
            .iter()
            .find(|fnode| fnode.as_str() == target_ref)
        {
            return Ok(exact.clone());
        }

        let target_lc = target_ref.to_lowercase();
        let exact_matches: Vec<&String> = direct_fnodes
            .iter()
            .filter(|fnode| fnode.to_lowercase() == target_lc)
            .collect();
        if let [matched] = exact_matches.as_slice() {
            return Ok((*matched).clone());
        }
        let prefix_matches: Vec<&String> = direct_fnodes
            .iter()
            .filter(|fnode| fnode.to_lowercase().starts_with(&target_lc))
            .collect();
        match prefix_matches.as_slice() {
            [matched] => return Ok((*matched).clone()),
            matches if matches.len() > 1 => {
                let preview = matches
                    .iter()
                    .map(|fnode| fnode.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("ambiguous direct dependency target '{target_ref}', matches: {preview}");
            }
            _ => {}
        }

        let base = cwd.unwrap_or_else(|| self.cache.root());
        let (target_fnode, _, target_path) = self.cache.resolve_ref(target_ref, Some(base))?;
        if let Some(matched) = direct_fnodes
            .iter()
            .find(|fnode| fnode.eq_ignore_ascii_case(&target_fnode))
        {
            return Ok(matched.clone());
        }
        bail!(
            "target {}:{} is not a direct dependency of this node",
            short_fnode(&target_fnode),
            to_rel_path(self.cache.root(), &target_path)
        )
    }

    // ── Graph mutations ───────────────────────────────────────────────────────

    /// Add `dep_fnodes` as direct dependencies of the root node.
    /// Returns `(added, skipped_existing, skipped_self)`.
    pub fn add_direct_dependencies(
        &mut self,
        dep_fnodes: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let mutation_lock = self.cache.acquire_mutation_lock()?;
        self.add_direct_dependencies_under_lock(&mutation_lock, dep_fnodes)
    }

    fn add_direct_dependencies_under_lock(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        dep_fnodes: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let root = self.root.fnode.clone();
        let snapshot = self.snapshot_root_under_lock(mutation_lock, &root, None)?;
        self.add_direct_dependencies_locked(mutation_lock, &root, dep_fnodes, &snapshot)
    }

    /// Resolve one user-facing dependency reference while holding the workspace
    /// mutation lock, then persist only its exact, valid fnode.
    pub fn add_direct_dependency_ref(
        &mut self,
        target_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let mutation_lock = self.cache.acquire_mutation_lock()?;
        self.add_direct_dependency_ref_under_lock(&mutation_lock, target_ref, cwd, None)
    }

    pub(crate) fn add_direct_dependency_ref_under_lock(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        target_ref: &str,
        cwd: Option<&Path>,
        expected_revision: Option<&str>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let target_ref = target_ref.trim();
        if target_ref.is_empty() {
            bail!("dependency reference cannot be empty");
        }

        let root = self.root.fnode.clone();
        let snapshot = self.snapshot_root_under_lock(mutation_lock, &root, expected_revision)?;
        let base = cwd.unwrap_or_else(|| self.cache.root());
        let (target_fnode, _, _) = self.cache.resolve_ref(target_ref, Some(base))?;

        self.add_direct_dependencies_locked(mutation_lock, &root, vec![target_fnode], &snapshot)
    }

    fn add_direct_dependencies_locked(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        root: &str,
        dep_fnodes: Vec<String>,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let existing: HashSet<String> =
            { dedupe_keep_order(&self.root.depens).into_iter().collect() };
        let root_fnode_for_compare = root.to_string();

        let mut added: Vec<String> = Vec::new();
        let mut skipped_existing: Vec<String> = Vec::new();
        let mut skipped_self: Vec<String> = Vec::new();
        let mut seen_new: HashSet<String> = existing.clone();

        let dep_fnodes = dedupe_keep_order(&dep_fnodes);
        for dep_fnode in &dep_fnodes {
            self.validate_dependency_target(dep_fnode)?;
        }

        for dep_fnode in dep_fnodes {
            if dep_fnode == root_fnode_for_compare {
                skipped_self.push(dep_fnode);
                continue;
            }
            if existing.contains(&dep_fnode) {
                skipped_existing.push(dep_fnode);
                continue;
            }
            if seen_new.insert(dep_fnode.clone()) {
                added.push(dep_fnode);
            }
        }

        // Reject any dep that would create a cycle: adding root → dep_fnode creates
        // a cycle if dep_fnode can already reach root in the indexed graph.
        if let Some(dep_fnode) = self.first_reaching_root(&added, root)? {
            bail!(
                "adding {} as a dependency of {} would create a cycle",
                short_fnode(&dep_fnode),
                short_fnode(root)
            );
        }

        if !added.is_empty() {
            let mut updated = self.root.clone();
            for dep_fnode in &added {
                updated.add_dependency(dep_fnode);
            }
            self.save_root_update(mutation_lock, root, updated, snapshot)?;
        }

        Ok((added, skipped_existing, skipped_self))
    }

    fn validate_dependency_target(&mut self, exact_fnode: &str) -> Result<()> {
        let paths = self.cache.reconcile_fnode_paths(exact_fnode)?;
        let path = match paths.as_slice() {
            [] => bail!("dependency target is missing: {exact_fnode}"),
            [path] => path,
            _ => bail!(
                "{}",
                duplicate_fnode_error(self.cache.root(), exact_fnode, &paths)
            ),
        };
        let rel_path = crate::workspace::to_indexed_rel_path(self.cache.root(), path)?;
        if self.cache.path_has_blocking_issue(&rel_path)? {
            bail!("dependency target must be valid: {exact_fnode}");
        }
        let node = MdocNode::load(path)?;
        if node.fnode != exact_fnode {
            bail!(
                "dependency target must be an exact fnode: expected {exact_fnode}, found {}",
                node.fnode
            );
        }
        Ok(())
    }

    /// Remove `dep_fnodes` from the root node's direct dependencies. Returns removed fnodes.
    pub fn remove_direct_dependencies(&mut self, dep_fnodes: Vec<String>) -> Result<Vec<String>> {
        let mutation_lock = self.cache.acquire_mutation_lock()?;
        self.remove_direct_dependencies_under_lock(&mutation_lock, dep_fnodes, None)
    }

    pub(crate) fn remove_direct_dependencies_under_lock(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        dep_fnodes: Vec<String>,
        expected_revision: Option<&str>,
    ) -> Result<Vec<String>> {
        let root = self.root.fnode.clone();
        let snapshot = self.snapshot_root_under_lock(mutation_lock, &root, expected_revision)?;

        let mut updated = self.root.clone();
        let mut removed: Vec<String> = Vec::new();
        for dep_fnode in dedupe_keep_order(&dep_fnodes) {
            if updated.depens.contains(&dep_fnode) {
                updated.remove_dependency(&dep_fnode);
                removed.push(dep_fnode);
            }
        }

        if !removed.is_empty() {
            self.save_root_update(mutation_lock, &root, updated, &snapshot)?;
        }
        Ok(removed)
    }

    /// Save `new_node` to disk, index it, load it into the in-memory graph, and
    /// add it as a direct dependency of the root. Returns `true` if it was added.
    ///
    /// All cycle validation is done before any I/O so failure leaves no files on
    /// disk and no index entries. Two sources of cycles are checked up-front:
    ///  - `new_node.fnode` already exists in the index with a path to root
    ///    (fnode collision with an existing node that can reach root).
    ///  - `new_node.depens` contains a fnode that can reach root in the current
    ///    index (root → new_node → declared_dep → … → root).
    pub fn create_and_add_dependency(&mut self, new_node: MdocNode) -> Result<bool> {
        let mutation_lock = self.cache.acquire_mutation_lock()?;
        self.create_and_add_dependency_under_lock(&mutation_lock, new_node, None)
    }

    pub(crate) fn create_and_add_dependency_under_lock(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        mut new_node: MdocNode,
        expected_revision: Option<&str>,
    ) -> Result<bool> {
        let root = self.root.fnode.clone();
        let snapshot = self.snapshot_root_under_lock(mutation_lock, &root, expected_revision)?;

        // Reject duplicate fnode before touching disk.
        if !self
            .cache
            .reconcile_fnode_paths(&new_node.fnode)?
            .is_empty()
        {
            bail!(
                "fnode {} is already used by another file in this workspace",
                short_fnode(&new_node.fnode)
            );
        }

        // Enforce path contract before touching disk: must be inside mdcroot, not in
        // a nested root, not already existing. Returns the resolved canonical path —
        // we update new_node.path to it so the write never uses a raw symlink-bearing path.
        new_node.path =
            crate::workspace::validate_new_mdoc_path(self.cache.root(), &new_node.path)?;

        let declared_dependencies = dedupe_keep_order(&new_node.depens);
        for dep_fnode in &declared_dependencies {
            if dep_fnode == &new_node.fnode {
                bail!("new node cannot depend on itself");
            }
            self.validate_dependency_target(dep_fnode)?;
        }

        // Check both cycle sources before touching disk.
        let mut cycle_candidates = Vec::with_capacity(declared_dependencies.len() + 1);
        cycle_candidates.push(new_node.fnode.clone());
        cycle_candidates.extend(declared_dependencies.iter().cloned());
        let cycle_source = self.first_reaching_root(&cycle_candidates, &root)?;
        if cycle_source.as_deref() == Some(new_node.fnode.as_str()) {
            bail!(
                "adding {} as a dependency of {} would create a cycle",
                short_fnode(&new_node.fnode),
                short_fnode(&root)
            );
        }
        for dep_fnode in &declared_dependencies {
            if cycle_source.as_deref() == Some(dep_fnode.as_str()) {
                bail!(
                    "adding {} as a dependency of {} would create a cycle \
                     (new node's dep {} already reaches root)",
                    short_fnode(&new_node.fnode),
                    short_fnode(&root),
                    short_fnode(dep_fnode)
                );
            }
        }

        let receipt = self.cache.create_node(mutation_lock, &new_node)?;
        let fnode = new_node.fnode.clone();
        let new_path = new_node.path.clone();
        let root_path = self.root.path.clone();
        let (added, _, _) = match self.add_direct_dependencies_locked(
            mutation_lock,
            &root,
            vec![fnode.clone()],
            &snapshot,
        ) {
            Ok(result) => result,
            Err(link_error) => {
                return Err(
                    self.recover_failed_link(&root_path, &snapshot, &new_path, receipt, link_error)
                );
            }
        };
        Ok(!added.is_empty())
    }

    fn first_reaching_root(&self, candidates: &[String], root: &str) -> Result<Option<String>> {
        match candidates {
            [] => Ok(None),
            [candidate] => self
                .cache
                .is_reachable(candidate, root)
                .map(|reaches| reaches.then(|| candidate.clone())),
            _ => {
                let reaches_root = self.cache.reverse_reachable_fnodes(root)?;
                Ok(candidates
                    .iter()
                    .find(|candidate| reaches_root.contains(*candidate))
                    .cloned())
            }
        }
    }

    fn recover_failed_link(
        &mut self,
        root_path: &Path,
        root_snapshot: &crate::workspace::FileSnapshot,
        new_path: &Path,
        receipt: crate::workspace::AppliedWrite,
        link_error: anyhow::Error,
    ) -> anyhow::Error {
        let root_still_original = root_snapshot
            .unchanged_beneath(self.cache.root(), root_path)
            .and_then(|unchanged| {
                if unchanged {
                    Ok(true)
                } else {
                    crate::workspace::FileSnapshot::capture_beneath(self.cache.root(), root_path)
                        .map(|current| current.content() == root_snapshot.content())
                }
            });
        let rollback_result = match root_still_original {
            Ok(true) => receipt.rollback(),
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        };
        let index_result = self.cache.upsert_path(new_path);
        crate::workspace::PersistenceRecoveryError::from_attempts(
            link_error,
            rollback_result,
            index_result,
            &format!("roll back {}", new_path.display()),
            &format!("repair the index for {}", new_path.display()),
        )
    }

    #[cfg(test)]
    fn snapshot_root_for_mutation(&mut self, root: &str) -> Result<crate::workspace::FileSnapshot> {
        let path = self.root.path.clone();
        let snapshot = crate::workspace::FileSnapshot::capture(&path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow!("mdoc file disappeared: {}", path.display()))?;
        let node = MdocNode::load_bytes(&path, content)?;
        if node.fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        self.root = node;
        Ok(snapshot)
    }

    fn snapshot_root_under_lock(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        root: &str,
        expected_revision: Option<&str>,
    ) -> Result<crate::workspace::FileSnapshot> {
        self.cache.validate_mutation_lock(mutation_lock)?;
        self.cache.refresh_all()?;
        let (resolved_fnode, _, path) = self.cache.resolve_ref(root, Some(self.cache.root()))?;
        if resolved_fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        let snapshot = crate::workspace::FileSnapshot::capture(&path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow!("mdoc file disappeared: {}", path.display()))?;
        if expected_revision
            .is_some_and(|expected| expected != crate::mdocnode::content_revision(content))
        {
            return Err(crate::mdocnode::RevisionMismatch.into());
        }
        let node = MdocNode::load_bytes(&path, content)?;
        if node.fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        self.root = node;
        Ok(snapshot)
    }

    fn save_root_update(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        root: &str,
        updated: MdocNode,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<()> {
        self.cache.replace_node(mutation_lock, &updated, snapshot)?;
        debug_assert_eq!(root, updated.fnode);
        self.root = updated;
        Ok(())
    }
}

/// Deduplicate while preserving first-occurrence order.
fn dedupe_keep_order(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter(|s| seen.insert(s.as_str()))
        .cloned()
        .collect()
}

fn prepare_new_node(
    root: &Path,
    file_path: &str,
    title: &str,
    fnode: Option<&str>,
) -> Result<MdocNode> {
    let mut node = MdocNode::new_at_path(root, title);
    if let Some(fnode) = fnode {
        node.fnode = fnode.to_string();
    }
    node.path = crate::workspace::resolve_new_mdoc_path(root, file_path, &node.fnode)?;
    Ok(node)
}

fn duplicate_fnode_error(mdcroot: &Path, fnode: &str, paths: &[PathBuf]) -> String {
    let rel_paths: Vec<String> = paths.iter().map(|p| to_rel_path(mdcroot, p)).collect();
    format!(
        "duplicate fnode '{fnode}' found in: {}",
        rel_paths.join(", ")
    )
}

#[cfg(test)]
mod mutation_conflict_tests {
    use super::*;

    fn setup_graph(with_dependency: bool) -> (tempfile::TempDir, WorkspaceStore, String, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();

        let target_path = root.join("target.mdoc");
        let target = MdocNode::new_at_path(&target_path, "Target");
        let target_fnode = target.fnode.clone();
        std::fs::write(&target_path, target.render().unwrap()).unwrap();

        let source_path = root.join("source.mdoc");
        let mut source = MdocNode::new_at_path(&source_path, "Source");
        if with_dependency {
            source.add_dependency(&target_fnode);
        }
        let source_fnode = source.fnode.clone();
        std::fs::write(&source_path, source.render().unwrap()).unwrap();

        let cache = WorkspaceStore::open(root.to_path_buf()).unwrap();
        (dir, cache, source_fnode, target_fnode)
    }

    #[test]
    fn add_and_remove_dependency_conflicts_preserve_external_edit_and_index() {
        for removing in [false, true] {
            let (_dir, mut cache, source_fnode, target_fnode) = setup_graph(removing);
            let mut graph = DepGraph::from_ref(&mut cache, &source_fnode, None).unwrap();
            let snapshot = graph.snapshot_root_for_mutation(&source_fnode).unwrap();
            let mut desired = graph.root.clone();
            if removing {
                desired.remove_dependency(&target_fnode);
            } else {
                desired.add_dependency(&target_fnode);
            }

            // Deterministic failpoint between snapshot/parse and replacement.
            let mut external = MdocNode::load(&desired.path).unwrap();
            external.title = "External edit".to_string();
            std::fs::write(&external.path, external.render().unwrap()).unwrap();
            let external_bytes = std::fs::read(&external.path).unwrap();
            let mutation_lock = graph.cache.acquire_mutation_lock().unwrap();

            let error = graph
                .save_root_update(&mutation_lock, &source_fnode, desired, &snapshot)
                .unwrap_err();
            assert!(error
                .downcast_ref::<crate::workspace::FileConflict>()
                .is_some());
            assert_eq!(std::fs::read(&external.path).unwrap(), external_bytes);

            let summary = graph.cache.node_summary(&source_fnode).unwrap();
            assert_eq!(summary.title, "External edit");
        }
    }

    #[test]
    fn failed_link_rollback_preserves_external_edit_and_repairs_index() {
        let (_dir, mut cache, source_fnode, _target_fnode) = setup_graph(false);
        let mut graph = DepGraph::from_ref(&mut cache, &source_fnode, None).unwrap();
        let path = graph.cache.root().join("created.mdoc");
        let mut created = MdocNode::new_at_path(&path, "Created");
        created.fnode = "created-node".to_string();
        let mutation_lock = graph.cache.acquire_mutation_lock().unwrap();
        let receipt = graph.cache.create_node(&mutation_lock, &created).unwrap();
        let root_path = graph.root.path.clone();
        let root_snapshot = crate::workspace::FileSnapshot::capture(&root_path).unwrap();

        created.title = "External edit".to_string();
        std::fs::write(&path, created.render().unwrap()).unwrap();

        let error = graph.recover_failed_link(
            &root_path,
            &root_snapshot,
            &path,
            receipt,
            anyhow!("injected link failure"),
        );

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert_eq!(MdocNode::load(&path).unwrap().title, "External edit");
        let summary = graph.cache.node_summary("created-node").unwrap();
        assert_eq!(summary.title, "External edit");
    }

    #[test]
    fn committed_parent_link_keeps_created_child_after_lock_replacement() {
        let (dir, mut cache, source_fnode, _target_fnode) = setup_graph(false);
        let mut graph = DepGraph::from_ref(&mut cache, &source_fnode, None).unwrap();
        let child_path = graph.cache.root().join("created.mdoc");
        let mut child = MdocNode::new_at_path(&child_path, "Created");
        child.fnode = "created-node".to_string();
        let mutation_lock = graph.cache.acquire_mutation_lock().unwrap();
        let lock_path = graph.cache.root().join(".mdc/mutation.lock");
        let displaced_lock = dir.path().join("displaced-mutation.lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::IndexAfterNodeUpsert,
            move || {
                crate::workspace::set_test_hook(
                    crate::workspace::TestHookPoint::IndexAfterNodeUpsert,
                    move || {
                        std::fs::rename(&lock_path, displaced_lock).unwrap();
                        std::fs::write(&lock_path, []).unwrap();
                    },
                );
            },
        );

        let error = graph
            .create_and_add_dependency_under_lock(&mutation_lock, child, None)
            .unwrap_err();

        assert!(error.to_string().contains("uncertain lock"));
        let parent = MdocNode::load(&graph.root.path).unwrap();
        assert!(parent.depens.contains(&"created-node".to_string()));
        assert_eq!(MdocNode::load(&child_path).unwrap().fnode, "created-node");
        assert_eq!(
            graph.cache.node_summary("created-node").unwrap().title,
            "Created"
        );
    }
}
