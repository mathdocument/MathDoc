mod query;
mod state;
pub mod workback;

use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::core::{find_cycle, topo_dependencies_first, DependencyItem, GraphIssue};
use crate::indcache::IndCache;
use crate::mdocnode::{read_mdoc_head, MdocNode};
use crate::workspace::{find_nested_mdcroot, iter_mdoc_files, to_rel_path};
use query::dependency_items_from_graph;
use state::{make_invalid_issue, GraphState};

// ── DepGraph ──────────────────────────────────────────────────────────────────

/// In-memory dependency graph for a workspace rooted at `mdcroot`.
/// Wraps an `IndCache` for path resolution and bootstrapping.
pub struct DepGraph {
    pub mdcroot: PathBuf,
    pub cache: IndCache,
    pub state: GraphState,
}

impl DepGraph {
    // ── Constructors ─────────────────────────────────────────────────────────

    /// Convenience constructor: open a cache and load root by fnode (for tests and CLI).
    pub fn new(mdcroot: PathBuf, root_fnode: &str) -> Result<Self> {
        let mut cache = IndCache::open(mdcroot)?;
        cache.bootstrap_if_needed()?;
        let (graph, _) = DepGraph::from_ref(cache, root_fnode, None)?;
        Ok(graph)
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
    ) -> Result<(Self, String)> {
        let root = mdcroot.canonicalize()?;
        let mut node = MdocNode::new_at_path(&root, &root, title);
        if let Some(f) = fnode {
            node.fnode = f.to_string();
        }
        node.path = resolve_new_node_path(&root, file_path, &node.fnode)?;

        // Open (or receive) the cache before any I/O so we can pre-validate.
        let mut cache = match cache {
            Some(c) => c,
            None => IndCache::open(root.clone())?,
        };
        cache.bootstrap_if_needed()?;
        if !cache.duplicate_fnode_paths(&node.fnode)?.is_empty() {
            bail!(
                "fnode {} is already used by another file in this workspace",
                &node.fnode[..node.fnode.len().min(8)]
            );
        }

        node.save()?;
        let node_path = node.path.clone();
        let rel_path = to_rel_path(&root, &node.path);
        let mut graph = DepGraph {
            mdcroot: root,
            cache,
            state: GraphState::default(),
        };
        graph.set_root_node(node)?;
        graph.cache.upsert_path(&node_path)?;
        Ok((graph, rel_path))
    }

    /// Load an existing `.mdoc` via `ref` (fnode, path, or fnode prefix) and build a DepGraph.
    pub fn from_ref(
        mut cache: IndCache,
        ref_str: &str,
        cwd: Option<&Path>,
    ) -> Result<(Self, String)> {
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

        let rel_path = to_rel_path(&cache.root, &src_path);
        let mdcroot = cache.root.clone();
        let mut graph = DepGraph {
            mdcroot,
            cache,
            state: GraphState::default(),
        };
        graph.set_root_node(node)?;
        Ok((graph, rel_path))
    }

    // ── Root management ───────────────────────────────────────────────────────

    pub fn root_fnode(&self) -> &str {
        &self.state.root_fnode
    }

    pub fn set_root_node(&mut self, node: MdocNode) -> Result<()> {
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

    pub fn set_root_fnode(&mut self, fnode: &str) -> Result<()> {
        let value = fnode.trim();
        if value.is_empty() {
            bail!("root fnode cannot be empty");
        }
        if !self.state.root_fnode.is_empty() && self.state.root_fnode != value {
            bail!(
                "root fnode mismatch: {} != {}",
                self.state.root_fnode,
                value
            );
        }
        self.state.root_fnode = value.to_string();
        Ok(())
    }

    /// Ensure the root node is loaded and return its path.
    pub fn root_path(&mut self) -> Result<PathBuf> {
        let root = self.bind_root(None, None)?;
        self.ensure_node_loaded(&root)?;
        Ok(self.state.nodes_by_fnode[&root].path.clone())
    }

    pub fn root_has_blocks(&mut self) -> Result<bool> {
        let root = self.bind_root(None, None)?;
        self.ensure_node_loaded(&root)?;
        Ok(!self.state.nodes_by_fnode[&root].blocks.is_empty())
    }

    pub fn root_item(&mut self) -> Result<DependencyItem> {
        let root = self.bind_root(None, None)?;
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

    pub fn is_broken_fnode(&self, fnode: &str) -> bool {
        if self.has_local_state(fnode) {
            return self.state.is_broken(fnode);
        }
        self.cache.issue_for_fnode(fnode).ok().flatten().is_some()
    }

    pub fn issue_for_fnode(&self, fnode: &str) -> Result<Option<GraphIssue>> {
        if self.has_local_state(fnode) {
            return Ok(self.state.broken_issues.get(fnode).cloned());
        }
        self.cache.issue_for_fnode(fnode)
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
        let root = self.bind_root(None, None)?;
        self.ensure_node_loaded(&root)?;
        let depens = self.state.nodes_by_fnode[&root].depens.clone();
        Ok(dedupe_keep_order(&depens))
    }

    pub fn direct_dependency_items(&mut self) -> Result<Vec<DependencyItem>> {
        let root = self.bind_root(None, None)?;
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

    pub fn dependency_items(&mut self, depth: i32) -> Result<Vec<DependencyItem>> {
        let root = self.dependency_context(depth, None, None)?;
        let mdcroot = self.mdcroot.clone();
        Ok(dependency_items_from_graph(
            &self.state,
            &root,
            &mdcroot,
            false,
        ))
    }

    pub fn leaf_dependency_items(&mut self) -> Result<Vec<DependencyItem>> {
        let root = self.dependency_context(-1, None, None)?;
        let mdcroot = self.mdcroot.clone();
        Ok(dependency_items_from_graph(
            &self.state,
            &root,
            &mdcroot,
            true,
        ))
    }

    /// Topologically-ordered nodes (dependencies first), ready for block evaluation.
    pub fn ordered_nodes(&mut self, depth: i32) -> Result<Vec<MdocNode>> {
        let root = self.dependency_context(depth, None, None)?;
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
        let root = self.bind_root(None, None)?;
        self.ensure_node_loaded(&root)?;

        let existing: HashSet<String> = {
            let node = &self.state.nodes_by_fnode[&root];
            dedupe_keep_order(&node.depens).into_iter().collect()
        };
        let root_fnode_for_compare = root.clone();

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
            if self.cache.is_reachable(dep_fnode, &root)? {
                bail!(
                    "adding {} as a dependency of {} would create a cycle",
                    &dep_fnode[..dep_fnode.len().min(8)],
                    &root[..root.len().min(8)]
                );
            }
        }

        if !added.is_empty() {
            // Commit: mutate the node, save to disk, and sync the index.
            {
                let node = self
                    .state
                    .nodes_by_fnode
                    .get_mut(&root)
                    .expect("root node was loaded via ensure_node_loaded");
                for dep_fnode in &added {
                    node.add_dependency(dep_fnode);
                }
            }
            self.state.nodes_by_fnode[&root].save()?;
            let root_path = self.state.nodes_by_fnode[&root].path.clone();
            self.cache.upsert_path(&root_path)?;

            let new_depens = dedupe_keep_order(&self.state.nodes_by_fnode[&root].depens.clone());
            self.state.dep_graph.insert(root.clone(), new_depens);
            for dep_fnode in &added {
                self.state.dep_graph.entry(dep_fnode.clone()).or_default();
            }
        }

        Ok((added, skipped_existing, skipped_self))
    }

    /// Remove `dep_fnodes` from the root node's direct dependencies. Returns removed fnodes.
    pub fn remove_direct_dependencies(&mut self, dep_fnodes: Vec<String>) -> Result<Vec<String>> {
        let root = self.bind_root(None, None)?;
        self.ensure_node_loaded(&root)?;

        let mut removed: Vec<String> = Vec::new();
        {
            let node = self
                .state
                .nodes_by_fnode
                .get_mut(&root)
                .expect("root node was loaded via ensure_node_loaded");
            for dep_fnode in dedupe_keep_order(&dep_fnodes) {
                if node.depens.contains(&dep_fnode) {
                    node.remove_dependency(&dep_fnode);
                    removed.push(dep_fnode);
                }
            }
        }

        if !removed.is_empty() {
            self.state.nodes_by_fnode[&root].save()?;
            let root_path = self.state.nodes_by_fnode[&root].path.clone();
            self.cache.upsert_path(&root_path)?;
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
        let root = self.bind_root(None, None)?;

        // Reject duplicate fnode before touching disk.
        if !self
            .cache
            .duplicate_fnode_paths(&new_node.fnode)?
            .is_empty()
        {
            bail!(
                "fnode {} is already used by another file in this workspace",
                &new_node.fnode[..new_node.fnode.len().min(8)]
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
                &new_node.fnode[..new_node.fnode.len().min(8)],
                &root[..root.len().min(8)]
            );
        }
        for dep_fnode in &new_node.depens {
            if self.cache.is_reachable(dep_fnode, &root)? {
                bail!(
                    "adding {} as a dependency of {} would create a cycle \
                     (new node's dep {} already reaches root)",
                    &new_node.fnode[..new_node.fnode.len().min(8)],
                    &root[..root.len().min(8)],
                    &dep_fnode[..dep_fnode.len().min(8)]
                );
            }
        }

        new_node.save()?;
        self.cache.upsert_path(&new_node.path)?;
        let fnode = new_node.fnode.clone();
        self.state.nodes_by_fnode.insert(fnode.clone(), new_node);
        self.state.dep_graph.entry(fnode.clone()).or_default();
        let (added, _, _) = self.add_direct_dependencies(vec![fnode])?;
        Ok(!added.is_empty())
    }

    // ── Full workspace scan ───────────────────────────────────────────────────

    /// Scan all `.mdoc` files in the workspace, loading them into state.
    pub fn scan_all(&mut self) -> Result<()> {
        self.ensure_ready()?;
        self.state.reset();

        let files: Vec<PathBuf> = iter_mdoc_files(&self.mdcroot).collect();
        let mut loaded: Vec<MdocNode> = Vec::new();

        for file_path in files {
            self.state.scanned_file_count += 1;
            match MdocNode::load(&self.mdcroot, &file_path) {
                Ok(node) => loaded.push(node),
                Err(e) => {
                    let head = read_mdoc_head(&file_path);
                    let fnode = head
                        .as_ref()
                        .map(|(f, _)| f.as_str())
                        .unwrap_or("<unknown>");
                    let issue =
                        make_invalid_issue(&self.mdcroot, &file_path, &e.to_string(), fnode);
                    self.state.record_invalid(issue);
                }
            }
        }

        // Group by fnode, detect duplicates
        let mut by_fnode: HashMap<String, Vec<MdocNode>> = HashMap::new();
        for node in loaded {
            by_fnode.entry(node.fnode.clone()).or_default().push(node);
        }
        let mut sorted_fnodes: Vec<String> = by_fnode.keys().cloned().collect();
        sorted_fnodes.sort();

        for fnode in sorted_fnodes {
            let nodes = by_fnode.remove(&fnode).unwrap();
            if nodes.len() == 1 {
                self.state
                    .nodes_by_fnode
                    .insert(fnode, nodes.into_iter().next().unwrap());
            } else {
                let paths: Vec<PathBuf> = nodes.iter().map(|n| n.path.clone()).collect();
                self.record_duplicate_fnode(&fnode, &paths)?;
            }
        }

        // Build dep_graph from loaded nodes
        let node_fnodes: Vec<String> = self.state.nodes_by_fnode.keys().cloned().collect();
        for fnode in node_fnodes {
            let depens = dedupe_keep_order(&self.state.nodes_by_fnode[&fnode].depens.clone());
            self.state.dep_graph.insert(fnode.clone(), Vec::new());
            for dep_fnode in &depens {
                if !self.state.nodes_by_fnode.contains_key(dep_fnode)
                    && !self.state.invalid_fnodes.contains(dep_fnode)
                {
                    if let Some(node) = self.load_node(dep_fnode, true, true)? {
                        self.state.nodes_by_fnode.insert(dep_fnode.clone(), node);
                    }
                }
                self.state
                    .dep_graph
                    .entry(fnode.clone())
                    .or_default()
                    .push(dep_fnode.clone());
                self.state.dep_graph.entry(dep_fnode.clone()).or_default();
            }
        }
        let loaded_fnodes: Vec<String> = self.state.nodes_by_fnode.keys().cloned().collect();
        for fnode in loaded_fnodes {
            self.state.dep_graph.entry(fnode).or_default();
        }
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
                    let rows = self.cache.exact_fnode_rows(fnode)?;
                    if rows.len() > 1 {
                        return Ok(Some(self.mdcroot.join(&rows[0].2)));
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

    fn bind_root(
        &mut self,
        root_node: Option<MdocNode>,
        root_fnode: Option<&str>,
    ) -> Result<String> {
        if let Some(node) = root_node {
            self.set_root_node(node)?;
        }
        if let Some(fnode) = root_fnode {
            self.set_root_fnode(fnode)?;
        }
        if self.state.root_fnode.is_empty() {
            bail!("root fnode is required");
        }
        Ok(self.state.root_fnode.clone())
    }

    fn dependency_context(
        &mut self,
        depth: i32,
        root_node: Option<MdocNode>,
        root_fnode: Option<&str>,
    ) -> Result<String> {
        if depth < -1 {
            bail!("depth must be -1 (infinite) or >= 0");
        }
        let root = self.bind_root(root_node, root_fnode)?;
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
                let s = &fnode[..fnode.len().min(8)];
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

/// Deduplicate while preserving first-occurrence order.
fn dedupe_keep_order(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter(|s| seen.insert(s.as_str()))
        .cloned()
        .collect()
}

/// Validate that `path` is an acceptable location for a new `.mdoc` file:
/// Validate that `path` is a safe, canonical location for a new `.mdoc` file and
/// return the fully-resolved path to use for writing.
///
/// Two classes of path-escape are blocked:
/// - **Lexical `..` through non-existent dirs**: e.g. `root/nope/../../outside.mdoc`
///   where `nope` does not exist. A naive `parent().canonicalize()` silently falls
///   back to the raw path here; this function handles it by evaluating `..` against
///   the resolved prefix built so far (which is already correctly bounded by root).
/// - **Symlink-assisted `..`**: e.g. `root/link/../outside.mdoc` where
///   `link → /external/`. Lexical normalization would collapse `link/..` to `root/`
///   (wrongly inside the workspace), but this function resolves the symlink first
///   via `canonicalize`, so `..` is evaluated relative to `/external/` — correctly
///   landing outside the workspace.
///
/// Algorithm: walk path components left-to-right; after each existing intermediate
/// directory is appended, call `canonicalize` to resolve any symlink at that level.
/// `..` and `.` are then applied to the already-resolved prefix.
/// The final (filename) component is appended verbatim — it must not yet exist.
///
/// Returns the resolved absolute path that callers must use for all subsequent I/O.
fn validate_new_node_path(mdcroot: &Path, path: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let comps: Vec<_> = path.components().collect();
    let mut out = PathBuf::new();
    for (i, comp) in comps.iter().enumerate() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                out.push(name);
                // Resolve symlinks in intermediate components only (not the filename
                // to be created). This makes `..` evaluate against the real on-disk
                // parent rather than the lexical one, blocking symlink-based escapes.
                if i < comps.len() - 1 {
                    if let Ok(canonical) = out.canonicalize() {
                        out = canonical;
                    }
                }
            }
        }
    }
    if out.extension().and_then(|e| e.to_str()) != Some("mdoc") {
        bail!("path must have a .mdoc extension: {}", out.display());
    }
    if out.strip_prefix(mdcroot).is_err() {
        bail!("target path must be under mdoc root {}", mdcroot.display());
    }
    let parent = out.parent().unwrap_or(mdcroot);
    if let Some(nested) = find_nested_mdcroot(mdcroot, parent) {
        bail!(
            "target path is inside nested mdoc root: {}",
            nested.display()
        );
    }
    if out.exists() {
        bail!("mdoc file already exists: {}", out.display());
    }
    Ok(out)
}

/// Resolve the path for a new `.mdoc` file given a relative target (no extension).
/// Returns the absolute path with `.mdoc` appended to the last component.
fn resolve_new_node_path(mdcroot: &Path, raw_target: &str, fnode: &str) -> Result<PathBuf> {
    let target = raw_target.trim();
    if target.is_empty() || target == "." {
        return validate_new_node_path(mdcroot, &mdcroot.join(format!("{fnode}.mdoc")));
    }
    let rel = Path::new(target);
    if rel.is_absolute() {
        bail!("target path must be relative to the mdoc root");
    }
    let joined = mdcroot.join(rel);
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
