mod state;
pub mod workback;

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::core::{find_cycle, short_fnode, topo_dependencies_first, DependencyItem};
use crate::indcache::IndCache;
use crate::mdocnode::MdocNode;
use crate::workspace::to_rel_path;
use state::{make_invalid_issue, GraphState};

// ── DepGraph ──────────────────────────────────────────────────────────────────

/// In-memory dependency graph for a workspace rooted at `mdcroot`.
/// Wraps an `IndCache` for path resolution and bootstrapping.
pub struct DepGraph {
    mdcroot: PathBuf,
    cache: IndCache,
    state: GraphState,
}

impl DepGraph {
    // ── Constructors ─────────────────────────────────────────────────────────

    /// Convenience constructor: open a cache and load root by fnode (for tests and CLI).
    pub fn new(mdcroot: PathBuf, root_fnode: &str) -> Result<Self> {
        let mut cache = IndCache::open(mdcroot).context("opening workspace index")?;
        cache.bootstrap_if_needed()?;
        DepGraph::from_ref(cache, root_fnode, None)
            .context("loading graph root from workspace index")
    }

    /// Create a fresh `.mdoc` file and return a DepGraph rooted at it.
    ///
    /// `file_path`: relative path (without `.mdoc`) or `"."` for `{fnode}.mdoc` in root.
    pub fn create_root(
        mdcroot: PathBuf,
        file_path: &str,
        title: &str,
        fnode: Option<&str>,
        cache: Option<IndCache>,
    ) -> Result<Self> {
        let root = crate::workspace::validate_mdcroot(&mdcroot)?;
        let _mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
        let mut node = MdocNode::new_at_path(&root, &root, title);
        if let Some(f) = fnode {
            node.fnode = f.to_string();
        }
        node.path = resolve_new_node_path(&root, file_path, &node.fnode)?;

        // Open (or receive) the cache before any I/O so we can pre-validate.
        let mut cache = match cache {
            Some(c) => c,
            None => IndCache::open_under_mutation_lock(root.clone())?,
        };
        cache.refresh_all()?;
        if !cache.duplicate_fnode_paths(&node.fnode)?.is_empty() {
            bail!(
                "fnode {} is already used by another file in this workspace",
                short_fnode(&node.fnode)
            );
        }

        let mut graph = DepGraph {
            mdcroot: root,
            cache,
            state: GraphState::default(),
        };
        graph.set_root_node(node.clone())?;
        create_indexed_node(&mut graph.cache, &node)?;
        Ok(graph)
    }

    /// Load an existing `.mdoc` via `ref` (fnode, path, or fnode prefix) and build a DepGraph.
    pub fn from_ref(cache: IndCache, ref_str: &str, cwd: Option<&Path>) -> Result<Self> {
        let _mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&cache.root)?;
        Self::from_ref_locked(cache, ref_str, cwd)
    }

    fn from_ref_locked(mut cache: IndCache, ref_str: &str, cwd: Option<&Path>) -> Result<Self> {
        let base_cwd = cwd
            .map(|c| c.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| cache.root.clone()));
        cache.bootstrap_if_needed()?;
        let (_, _, src_path) = cache.resolve_ref(ref_str, Some(&base_cwd))?;
        // Ensure the resolved file is indexed before checking for duplicates.
        // Without this, a file resolved via filesystem fallback (not yet in the index)
        // would be invisible to duplicate_fnode_paths, allowing a silent bypass.
        cache.upsert_path(&src_path)?;
        let node = MdocNode::load(&cache.root, &src_path)?;

        let dup_paths = cache.duplicate_fnode_paths(&node.fnode)?;
        if dup_paths.len() > 1 {
            bail!(
                "{}",
                duplicate_fnode_error(&cache.root, &node.fnode, &dup_paths)
            );
        }

        let mdcroot = cache.root.clone();
        let mut graph = DepGraph {
            mdcroot,
            cache,
            state: GraphState::default(),
        };
        graph.set_root_node(node)?;
        Ok(graph)
    }

    // ── Root management ───────────────────────────────────────────────────────

    pub fn root_fnode(&self) -> &str {
        &self.state.root_fnode
    }

    pub(crate) fn mdcroot(&self) -> &Path {
        &self.mdcroot
    }

    pub(crate) fn cache_mut(&mut self) -> &mut IndCache {
        &mut self.cache
    }

    pub(crate) fn into_cache(self) -> IndCache {
        self.cache
    }

    pub(crate) fn root_node(&self) -> &MdocNode {
        self.state
            .nodes_by_fnode
            .get(self.root_fnode())
            .expect("constructed graph keeps its root loaded")
    }

    fn set_root_node(&mut self, node: MdocNode) -> Result<()> {
        if node
            .mdcroot
            .canonicalize()
            .unwrap_or_else(|_| node.mdcroot.clone())
            != self.mdcroot
        {
            bail!(
                "mdoc node root mismatch: {} != {}",
                node.mdcroot.display(),
                self.mdcroot.display()
            );
        }
        if !self.state.root_fnode.is_empty() && self.state.root_fnode != node.fnode {
            bail!(
                "root fnode mismatch: {} != {}",
                self.state.root_fnode,
                node.fnode
            );
        }
        self.state.root_fnode = node.fnode.clone();
        self.state.dep_graph.entry(node.fnode.clone()).or_default();
        self.state.nodes_by_fnode.insert(node.fnode.clone(), node);
        Ok(())
    }

    /// Ensure the root node is loaded and return its path.
    pub fn root_path(&mut self) -> Result<PathBuf> {
        let root = self.state.root_fnode.clone();
        self.ensure_node_loaded(&root)?;
        Ok(self.state.nodes_by_fnode[&root].path.clone())
    }

    pub fn root_item(&mut self) -> Result<DependencyItem> {
        let root = self.state.root_fnode.clone();
        if let Some(issue) = self.state.broken_issues.get(&root) {
            return Ok(DependencyItem {
                depth: 0,
                fnode: issue.fnode.clone(),
                title: issue.title.clone(),
                rel_path: issue.rel_path.clone(),
            });
        }
        self.ensure_node_loaded(&root)?;
        let node = &self.state.nodes_by_fnode[&root];
        Ok(DependencyItem {
            depth: 0,
            fnode: node.fnode.clone(),
            title: node.title.clone(),
            rel_path: to_rel_path(&self.mdcroot, &node.path),
        })
    }

    // ── Issue queries ─────────────────────────────────────────────────────────

    pub fn is_broken_fnode(&self, fnode: &str) -> Result<bool> {
        if self.has_local_state(fnode) {
            return Ok(self.state.is_broken(fnode));
        }
        Ok(self.cache.issue_for_fnode(fnode)?.is_some())
    }

    pub fn ref_item_for_fnode(&self, fnode: &str, depth: u32) -> Result<DependencyItem> {
        if self.state.nodes_by_fnode.contains_key(fnode)
            || self.state.broken_issues.contains_key(fnode)
        {
            return Ok(self.state.dependency_item(fnode, depth, &self.mdcroot));
        }
        self.cache.ref_item_for_fnode(fnode, depth)
    }

    // ── Dependency queries ────────────────────────────────────────────────────

    pub fn direct_dependency_fnodes(&mut self) -> Result<Vec<String>> {
        let root = self.state.root_fnode.clone();
        self.ensure_node_loaded(&root)?;
        let depens = self.state.nodes_by_fnode[&root].depens.clone();
        Ok(dedupe_keep_order(&depens))
    }

    pub fn direct_dependency_items(&mut self) -> Result<Vec<DependencyItem>> {
        let root = self.state.root_fnode.clone();
        self.ensure_node_loaded(&root)?;
        let depens = dedupe_keep_order(&self.state.nodes_by_fnode[&root].depens.clone());

        self.state.dep_graph.entry(root.clone()).or_default();
        let mut items = Vec::new();
        for dep_fnode in &depens {
            if !self.state.nodes_by_fnode.contains_key(dep_fnode) {
                if let Some(node) = self.load_node(dep_fnode, true, true)? {
                    self.state.nodes_by_fnode.insert(dep_fnode.clone(), node);
                }
            }
            self.state.dep_graph.entry(dep_fnode.clone()).or_default();
            items.push(self.ref_item_for_fnode(dep_fnode, 1)?);
        }
        self.state.dep_graph.insert(root, depens);
        Ok(items)
    }

    /// Topologically-ordered nodes (dependencies first), ready for block evaluation.
    pub fn ordered_nodes(&mut self, depth: i32) -> Result<Vec<MdocNode>> {
        let root = self.dependency_context(depth)?;
        let topo = topo_dependencies_first(&self.state.dep_graph, &root);
        Ok(topo
            .into_iter()
            .filter_map(|f| self.state.nodes_by_fnode.get(&f).cloned())
            .collect())
    }

    // ── Graph mutations ───────────────────────────────────────────────────────

    /// Add `dep_fnodes` as direct dependencies of the root node.
    /// Returns `(added, skipped_existing, skipped_self)`.
    pub fn add_direct_dependencies(
        &mut self,
        dep_fnodes: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let root = self.state.root_fnode.clone();
        let (_mutation_lock, snapshot) = self.lock_and_snapshot_root(&root)?;
        self.add_direct_dependencies_locked(&root, dep_fnodes, &snapshot)
    }

    /// Resolve one user-facing dependency reference while holding the workspace
    /// mutation lock, then persist only its exact, valid fnode.
    pub fn add_direct_dependency_ref(
        &mut self,
        target_ref: &str,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let target_ref = target_ref.trim();
        if target_ref.is_empty() {
            bail!("dependency reference cannot be empty");
        }

        let root = self.state.root_fnode.clone();
        let (_mutation_lock, snapshot) = self.lock_and_snapshot_root(&root)?;
        self.cache.discover_workspace_changes()?;
        let (_, _, target_path) = self.cache.resolve_ref(target_ref, Some(&self.mdcroot))?;
        self.cache.refresh_reachable_from_path(&target_path, -1)?;
        let (target_fnode, _, _) = self.cache.resolve_ref(target_ref, Some(&self.mdcroot))?;

        let paths = self.cache.duplicate_fnode_paths(&target_fnode)?;
        if paths.len() > 1 {
            bail!(
                "{}",
                duplicate_fnode_error(&self.mdcroot, &target_fnode, &paths)
            );
        }
        if paths.is_empty() {
            bail!(
                "dependency reference must resolve to exactly one node: {}",
                target_ref
            );
        }
        if self.cache.has_issues(&target_fnode)? {
            bail!("dependency target must be valid: {target_fnode}");
        }

        self.add_direct_dependencies_locked(&root, vec![target_fnode], &snapshot)
    }

    fn add_direct_dependencies_locked(
        &mut self,
        root: &str,
        dep_fnodes: Vec<String>,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let existing: HashSet<String> = {
            let node = &self.state.nodes_by_fnode[root];
            dedupe_keep_order(&node.depens).into_iter().collect()
        };
        let root_fnode_for_compare = root.to_string();

        let mut added: Vec<String> = Vec::new();
        let mut skipped_existing: Vec<String> = Vec::new();
        let mut skipped_self: Vec<String> = Vec::new();
        let mut seen_new: HashSet<String> = existing.clone();

        for dep_fnode in dedupe_keep_order(&dep_fnodes) {
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
        for dep_fnode in &added {
            if self.cache.is_reachable(dep_fnode, root)? {
                bail!(
                    "adding {} as a dependency of {} would create a cycle",
                    short_fnode(dep_fnode),
                    short_fnode(root)
                );
            }
        }

        if !added.is_empty() {
            let mut updated = self.state.nodes_by_fnode[root].clone();
            for dep_fnode in &added {
                updated.add_dependency(dep_fnode);
            }
            self.save_root_update(root, updated, snapshot)?;

            let new_depens = dedupe_keep_order(&self.state.nodes_by_fnode[root].depens.clone());
            self.state.dep_graph.insert(root.to_string(), new_depens);
            for dep_fnode in &added {
                self.state.dep_graph.entry(dep_fnode.clone()).or_default();
            }
        }

        Ok((added, skipped_existing, skipped_self))
    }

    /// Remove `dep_fnodes` from the root node's direct dependencies. Returns removed fnodes.
    pub fn remove_direct_dependencies(&mut self, dep_fnodes: Vec<String>) -> Result<Vec<String>> {
        let root = self.state.root_fnode.clone();
        let (_mutation_lock, snapshot) = self.lock_and_snapshot_root(&root)?;

        let mut updated = self.state.nodes_by_fnode[&root].clone();
        let mut removed: Vec<String> = Vec::new();
        for dep_fnode in dedupe_keep_order(&dep_fnodes) {
            if updated.depens.contains(&dep_fnode) {
                updated.remove_dependency(&dep_fnode);
                removed.push(dep_fnode);
            }
        }

        if !removed.is_empty() {
            self.save_root_update(&root, updated, &snapshot)?;
            let new_depens = dedupe_keep_order(&self.state.nodes_by_fnode[&root].depens.clone());
            self.state.dep_graph.insert(root, new_depens);
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
    pub fn create_and_add_dependency(&mut self, mut new_node: MdocNode) -> Result<bool> {
        let root = self.state.root_fnode.clone();
        let (_mutation_lock, snapshot) = self.lock_and_snapshot_root(&root)?;

        // Reject duplicate fnode before touching disk.
        if !self
            .cache
            .duplicate_fnode_paths(&new_node.fnode)?
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
        new_node.path = validate_new_node_path(&self.mdcroot, &new_node.path)?;

        // Check both cycle sources before touching disk.
        if self.cache.is_reachable(&new_node.fnode, &root)? {
            bail!(
                "adding {} as a dependency of {} would create a cycle",
                short_fnode(&new_node.fnode),
                short_fnode(&root)
            );
        }
        for dep_fnode in &new_node.depens {
            if dep_fnode == &new_node.fnode {
                bail!("new node cannot depend on itself");
            }
            if self.cache.is_reachable(dep_fnode, &root)? {
                bail!(
                    "adding {} as a dependency of {} would create a cycle \
                     (new node's dep {} already reaches root)",
                    short_fnode(&new_node.fnode),
                    short_fnode(&root),
                    short_fnode(dep_fnode)
                );
            }
        }

        create_indexed_node(&mut self.cache, &new_node)?;
        let fnode = new_node.fnode.clone();
        let new_path = new_node.path.clone();
        self.state.nodes_by_fnode.insert(fnode.clone(), new_node);
        self.state.dep_graph.entry(fnode.clone()).or_default();
        let (added, _, _) =
            match self.add_direct_dependencies_locked(&root, vec![fnode.clone()], &snapshot) {
                Ok(result) => result,
                Err(link_error) => {
                    self.state.nodes_by_fnode.remove(&fnode);
                    self.state.dep_graph.remove(&fnode);
                    let remove_result = std::fs::remove_file(&new_path);
                    let index_result = self.cache.upsert_path(&new_path);
                    let cleanup_error = remove_result
                        .err()
                        .map(|e| e.to_string())
                        .or_else(|| index_result.err().map(|e| e.to_string()));
                    if let Some(cleanup_error) = cleanup_error {
                        return Err(anyhow!(
                            "{link_error}; additionally failed to roll back {}: {cleanup_error}",
                            new_path.display()
                        ));
                    }
                    return Err(link_error);
                }
            };
        Ok(!added.is_empty())
    }

    #[cfg(test)]
    fn snapshot_root_for_mutation(&mut self, root: &str) -> Result<crate::workspace::FileSnapshot> {
        let path = self.state.nodes_by_fnode[root].path.clone();
        let snapshot = crate::workspace::FileSnapshot::capture(&path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow!("mdoc file disappeared: {}", path.display()))?;
        let node = MdocNode::load_bytes(&self.mdcroot, &path, content)?;
        if node.fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        self.state.nodes_by_fnode.insert(root.to_string(), node);
        Ok(snapshot)
    }

    fn lock_and_snapshot_root(
        &mut self,
        root: &str,
    ) -> Result<(
        crate::workspace::WorkspaceMutationLock,
        crate::workspace::FileSnapshot,
    )> {
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&self.mdcroot)?;
        self.cache.refresh_all()?;
        let (resolved_fnode, _, path) = self.cache.resolve_ref(root, Some(&self.mdcroot))?;
        if resolved_fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        let snapshot = crate::workspace::FileSnapshot::capture(&path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow!("mdoc file disappeared: {}", path.display()))?;
        let node = MdocNode::load_bytes(&self.mdcroot, &path, content)?;
        if node.fnode != root {
            bail!("fnode changed while preparing mutation: {root}");
        }
        self.state.nodes_by_fnode.insert(root.to_string(), node);
        Ok((mutation_lock, snapshot))
    }

    fn save_root_update(
        &mut self,
        root: &str,
        updated: MdocNode,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<()> {
        replace_indexed_node(&mut self.cache, &updated, snapshot)?;
        self.state.nodes_by_fnode.insert(root.to_string(), updated);
        Ok(())
    }

    // ── Private: loader helpers ───────────────────────────────────────────────

    fn ensure_ready(&mut self) -> Result<()> {
        let mdc = self.mdcroot.join(".mdc");
        if !mdc.is_dir() {
            bail!("invalid mdoc root (missing .mdc): {}", mdc.display());
        }
        self.cache.bootstrap_if_needed()
    }

    fn ensure_node_loaded(&mut self, fnode: &str) -> Result<()> {
        if self.state.nodes_by_fnode.contains_key(fnode) {
            return Ok(());
        }
        self.ensure_ready()?;
        match self.load_node(fnode, false, false)? {
            Some(node) => {
                self.state.nodes_by_fnode.insert(fnode.to_string(), node);
                self.state.dep_graph.entry(fnode.to_string()).or_default();
                Ok(())
            }
            None => bail!("no mdoc matched reference: {fnode}"),
        }
    }

    fn expand_from_root(&mut self, root_fnode: &str, depth: i32) -> Result<()> {
        self.ensure_node_loaded(root_fnode)?;

        let mut seen: HashSet<String> = HashSet::from([root_fnode.to_string()]);
        let mut queue: VecDeque<(String, u32)> = VecDeque::from([(root_fnode.to_string(), 0u32)]);

        while let Some((fnode, node_depth)) = queue.pop_front() {
            let depens = self
                .state
                .nodes_by_fnode
                .get(&fnode)
                .map(|n| dedupe_keep_order(&n.depens))
                .unwrap_or_default();

            self.state.dep_graph.insert(fnode.clone(), Vec::new());

            for dep_fnode in &depens {
                // Skip if depth limit reached and this dep is not yet seen
                if depth != -1 && node_depth as i32 >= depth && !seen.contains(dep_fnode) {
                    continue;
                }
                if !self.state.nodes_by_fnode.contains_key(dep_fnode) {
                    if let Some(node) = self.load_node(dep_fnode, true, true)? {
                        let sub_depens = dedupe_keep_order(&node.depens.clone());
                        self.state.nodes_by_fnode.insert(dep_fnode.clone(), node);
                        self.state
                            .dep_graph
                            .entry(dep_fnode.clone())
                            .or_insert(sub_depens);
                    }
                }
                self.state
                    .dep_graph
                    .entry(fnode.clone())
                    .or_default()
                    .push(dep_fnode.clone());
                self.state.dep_graph.entry(dep_fnode.clone()).or_default();

                if !self.state.nodes_by_fnode.contains_key(dep_fnode) {
                    continue;
                }
                if seen.insert(dep_fnode.clone()) {
                    queue.push_back((dep_fnode.clone(), node_depth + 1));
                }
            }
        }

        let loaded_fnodes: Vec<String> = self.state.nodes_by_fnode.keys().cloned().collect();
        for fnode in loaded_fnodes {
            self.state.dep_graph.entry(fnode).or_default();
        }
        Ok(())
    }

    fn load_node(
        &mut self,
        fnode: &str,
        tolerate_missing: bool,
        tolerate_invalid: bool,
    ) -> Result<Option<MdocNode>> {
        let path = match self.resolve_fnode_path(fnode, tolerate_missing)? {
            Some(p) => p,
            None => {
                if tolerate_missing {
                    self.state.mark_missing(fnode);
                    return Ok(None);
                }
                bail!("no mdoc matched reference: {fnode}");
            }
        };

        let node = match MdocNode::load(&self.mdcroot, &path) {
            Ok(n) => n,
            Err(e) => {
                let is_not_found = e
                    .downcast_ref::<std::io::Error>()
                    .map(|e| e.kind() == std::io::ErrorKind::NotFound)
                    .unwrap_or(false);
                if is_not_found && tolerate_missing {
                    self.state.mark_missing(fnode);
                    return Ok(None);
                }
                if !is_not_found && tolerate_invalid {
                    let issue = make_invalid_issue(&self.mdcroot, &path, &e.to_string(), fnode);
                    self.state.record_invalid(issue);
                    return Ok(None);
                }
                return Err(e);
            }
        };

        let dup_paths = self.cache.duplicate_fnode_paths(&node.fnode)?;
        if dup_paths.len() > 1 {
            if tolerate_invalid {
                self.record_duplicate_fnode(&node.fnode, &dup_paths)?;
                return Ok(None);
            }
            bail!(
                "{}",
                duplicate_fnode_error(&self.mdcroot, &node.fnode, &dup_paths)
            );
        }

        self.state.clear_broken(fnode);
        Ok(Some(node))
    }

    fn resolve_fnode_path(
        &mut self,
        fnode: &str,
        tolerate_missing: bool,
    ) -> Result<Option<PathBuf>> {
        let cwd = self.mdcroot.clone();
        match self.cache.resolve_ref(fnode, Some(&cwd)) {
            Ok((_, _, path)) => Ok(Some(path)),
            Err(e) => {
                let msg = e.to_string();
                if tolerate_missing && msg.starts_with("no mdoc matched reference:") {
                    return Ok(None);
                }
                // Ambiguous = duplicate fnode: return first path so load_node's dup check runs.
                if msg.contains("ambiguous mdoc reference") {
                    let paths = self.cache.duplicate_fnode_paths(fnode)?;
                    if paths.len() > 1 {
                        return Ok(paths.into_iter().next());
                    }
                }
                Err(e)
            }
        }
    }

    fn record_duplicate_fnode(&mut self, fnode: &str, paths: &[PathBuf]) -> Result<()> {
        let mut sorted = paths.to_vec();
        sorted.sort();
        let error = duplicate_fnode_error(&self.mdcroot, fnode, &sorted);
        for path in &sorted {
            let issue = make_invalid_issue(&self.mdcroot, path, &error, fnode);
            self.state.record_invalid(issue);
        }
        let first_issue = make_invalid_issue(&self.mdcroot, &sorted[0], &error, fnode);
        self.state
            .broken_issues
            .insert(fnode.to_string(), first_issue);
        Ok(())
    }

    fn dependency_context(&mut self, depth: i32) -> Result<String> {
        if depth < -1 {
            bail!("depth must be -1 (infinite) or >= 0");
        }
        let root = self.state.root_fnode.clone();
        self.expand_from_root(&root.clone(), depth)
            .map_err(|e| anyhow::anyhow!("failed to build dependency graph: {e}"))?;
        if let Some(cycle) = find_cycle(&self.state.dep_graph, Some(&root)) {
            let nodes = if cycle.len() > 1 && cycle.first() == cycle.last() {
                &cycle[..cycle.len() - 1]
            } else {
                &cycle[..]
            };
            let mut msg = String::from("dependency cycle detected:");
            for (i, fnode) in nodes.iter().enumerate() {
                let s = short_fnode(fnode);
                if nodes.len() == 1 {
                    msg.push_str(&format!("\n  ↺  {s}"));
                } else if i == 0 {
                    msg.push_str(&format!("\n  ┌➤  {s}"));
                } else if i == nodes.len() - 1 {
                    msg.push_str(&format!("\n  └─  {s}"));
                } else {
                    msg.push_str(&format!("\n  │   {s}"));
                }
            }
            bail!("{msg}");
        }
        Ok(root)
    }

    fn has_local_state(&self, fnode: &str) -> bool {
        self.state.nodes_by_fnode.contains_key(fnode)
            || self.state.broken_issues.contains_key(fnode)
            || self.state.dep_graph.contains_key(fnode)
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

fn create_indexed_node(cache: &mut IndCache, node: &MdocNode) -> Result<()> {
    node.save_new()?;
    if let Err(index_error) = cache.upsert_path(&node.path) {
        let cleanup_error = std::fs::remove_file(&node.path)
            .err()
            .map(anyhow::Error::from);
        let restore_index_error = cache.upsert_path(&node.path).err();
        let cleanup_error = cleanup_error.or(restore_index_error);
        return match cleanup_error {
            Some(cleanup_error) => Err(anyhow!(
                "{index_error}; additionally failed to remove or reindex {}: {cleanup_error}",
                node.path.display()
            )),
            None => Err(index_error),
        };
    }
    Ok(())
}

/// Replace a node file and update its index entry as one recoverable operation.
/// The caller must hold the workspace mutation lock.
pub(crate) fn replace_indexed_node(
    cache: &mut IndCache,
    node: &MdocNode,
    snapshot: &crate::workspace::FileSnapshot,
) -> Result<()> {
    let payload = node.render_payload()?;
    let applied = crate::workspace::atomic_replace(&node.path, snapshot, payload.as_bytes())?;
    if let Err(index_error) = cache.upsert_path(&node.path) {
        if let Err(rollback_error) = applied.rollback() {
            return Err(anyhow!(
                "{index_error}; additionally failed to restore {}: {rollback_error}",
                node.path.display()
            ));
        }
        if let Err(restore_index_error) = cache.upsert_path(&node.path) {
            return Err(anyhow!(
                "{index_error}; file was restored but its index could not be restored: {restore_index_error}"
            ));
        }
        return Err(index_error);
    }
    Ok(())
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

fn validate_new_node_path(mdcroot: &Path, path: &Path) -> Result<PathBuf> {
    let resolved = crate::workspace::resolve_mdoc_path(mdcroot, path)?;
    if std::fs::symlink_metadata(&resolved).is_ok() {
        bail!("mdoc file already exists: {}", resolved.display());
    }
    Ok(resolved)
}

/// Resolve the path for a new `.mdoc` file given a relative target (no extension).
/// Returns the absolute path with `.mdoc` appended to the last component.
pub(crate) fn resolve_new_node_path(
    mdcroot: &Path,
    raw_target: &str,
    fnode: &str,
) -> Result<PathBuf> {
    let target = raw_target.trim();
    if target.is_empty() || target == "." {
        return validate_new_node_path(mdcroot, &mdcroot.join(format!("{fnode}.mdoc")));
    }
    let rel = Path::new(target);
    if rel.is_absolute() {
        bail!("target path must be relative to the mdoc root");
    }
    let joined = mdcroot.join(rel);
    if joined.extension().and_then(|ext| ext.to_str()) == Some("mdoc") {
        return validate_new_node_path(mdcroot, &joined);
    }
    let stem = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid target path"))?
        .to_string_lossy();
    let final_path = joined.with_file_name(format!("{stem}.mdoc"));
    validate_new_node_path(mdcroot, &final_path)
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

    fn setup_graph(with_dependency: bool) -> (tempfile::TempDir, DepGraph, String, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();

        let target_path = root.join("target.mdoc");
        let target = MdocNode::new_at_path(root, &target_path, "Target");
        let target_fnode = target.fnode.clone();
        target.save_new().unwrap();

        let source_path = root.join("source.mdoc");
        let mut source = MdocNode::new_at_path(root, &source_path, "Source");
        if with_dependency {
            source.add_dependency(&target_fnode);
        }
        let source_fnode = source.fnode.clone();
        source.save_new().unwrap();

        let graph = DepGraph::new(root.to_path_buf(), &source_fnode).unwrap();
        (dir, graph, source_fnode, target_fnode)
    }

    #[test]
    fn add_and_remove_dependency_conflicts_preserve_external_edit_and_index() {
        for removing in [false, true] {
            let (_dir, mut graph, source_fnode, target_fnode) = setup_graph(removing);
            let snapshot = graph.snapshot_root_for_mutation(&source_fnode).unwrap();
            let mut desired = graph.state.nodes_by_fnode[&source_fnode].clone();
            if removing {
                desired.remove_dependency(&target_fnode);
            } else {
                desired.add_dependency(&target_fnode);
            }

            // Deterministic failpoint between snapshot/parse and replacement.
            let mut external = MdocNode::load(&graph.mdcroot, &desired.path).unwrap();
            external.title = "External edit".to_string();
            external.save().unwrap();
            let external_bytes = std::fs::read(&external.path).unwrap();

            let error = graph
                .save_root_update(&source_fnode, desired, &snapshot)
                .unwrap_err();
            assert!(error
                .downcast_ref::<crate::workspace::FileConflict>()
                .is_some());
            assert_eq!(std::fs::read(&external.path).unwrap(), external_bytes);

            let rows = graph.cache.exact_fnode_rows(&source_fnode).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, "Source");
        }
    }
}
