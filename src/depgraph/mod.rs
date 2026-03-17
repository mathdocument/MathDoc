mod query;
mod state;

pub use state::{make_invalid_issue, GraphState};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::core::{find_cycle, topo_dependencies_first, DependencyItem, GraphIssue};
use crate::indcache::IndCache;
use crate::mdoc::{read_mdoc_head, MdocNode};
use crate::workspace::{find_nested_mdcroot, iter_mdoc_files, to_rel_path};
use query::dependency_items_from_graph;

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
        node.save()?;
        let node_path = node.path.clone();
        let rel_path = to_rel_path(&root, &node.path);
        let cache = match cache {
            Some(c) => c,
            None => IndCache::open(root.clone())?,
        };
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

        if !added.is_empty() {
            // Commit: mutate the node and save
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
            let new_depens = dedupe_keep_order(&self.state.nodes_by_fnode[&root].depens.clone());
            self.state.dep_graph.insert(root, new_depens);
        }
        Ok(removed)
    }

    // ── Full workspace scan ───────────────────────────────────────────────────

    /// Evaluate blocks for the root node, merging dependency blocks when configured.
    ///
    /// For each block in the root node:
    /// - If `depens=true` for its srctype: concatenates same-srctype blocks from reachable deps.
    ///   With `reverse_depens=true` (default): root content first, then deps nearest-first.
    ///   With `reverse_depens=false`: deps deepest-first, then root content last.
    /// - If `depens=false`: compiles the root block content alone.
    ///
    /// Returns one `BlockResult` per root block.
    pub fn eval_blocks(
        &mut self,
        depth: i32,
        registry: &crate::compiler::CompilerRegistry,
        config: &crate::config::Config,
        progress: Option<fn(&str)>,
    ) -> Result<Vec<crate::compiler::BlockResult>> {
        // ordered_nodes returns topo order: [dep_deepest, ..., dep_direct, root]
        let nodes = self.ordered_nodes(depth)?;
        let root_fnode = self.state.root_fnode.clone();

        // dep_nodes = all nodes except root, in topo order (deepest first)
        let dep_nodes: Vec<&crate::mdoc::MdocNode> =
            nodes.iter().filter(|n| n.fnode != root_fnode).collect();
        let root_node = match nodes.iter().find(|n| n.fnode == root_fnode) {
            Some(n) => n,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for block in &root_node.blocks {
            let src_cfg = config.src_config(&block.srctype);
            let content = if src_cfg.effective_depens(&block.srctype) && !dep_nodes.is_empty() {
                // Collect same-srctype blocks from dep nodes (in topo order: deepest first)
                let dep_parts: Vec<&str> = dep_nodes
                    .iter()
                    .flat_map(|n| {
                        n.blocks
                            .iter()
                            .filter(|b| b.srctype == block.srctype)
                            .map(|b| b.content.trim_end_matches('\n'))
                    })
                    .collect();

                let root_part = block.content.trim_end_matches('\n');

                // reverse_depens=true (default): root first, then deps nearest-first (reversed topo)
                // reverse_depens=false: deps deepest-first (topo order), then root last
                let mut parts: Vec<&str> = if src_cfg.effective_reverse_depens() {
                    let mut p = vec![root_part];
                    p.extend(dep_parts.iter().rev().copied()); // reversed topo = nearest first
                    p
                } else {
                    let mut p: Vec<&str> = dep_parts; // topo order = deepest first
                    p.push(root_part);
                    p
                };
                parts.retain(|p| !p.is_empty());
                parts.join("\n\n")
            } else {
                block.content.clone()
            };

            let compcfg = src_cfg.to_compiler_cfg();
            let req = crate::compiler::CompilerReq {
                mdcroot: self.mdcroot.clone(),
                srctype: block.srctype.clone(),
                content,
                compcfg,
                progress: progress.map(|p| -> Box<dyn Fn(&str)> { Box::new(p) }),
            };
            let res = match registry.resolve(&block.srctype) {
                Some(compiler) => compiler.compile(&req),
                None => {
                    crate::compiler::CompilerRes::err(format!("unknown srctype: {}", block.srctype))
                }
            };
            results.push(crate::compiler::BlockResult {
                node_fnode: root_node.fnode.clone(),
                srctype: block.srctype.clone(),
                res,
            });
        }
        Ok(results)
    }

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
                        self.state.nodes_by_fnode.insert(dep_fnode.clone(), node);
                        if self.state.nodes_by_fnode.contains_key(dep_fnode) {
                            let sub_depens = dedupe_keep_order(
                                &self.state.nodes_by_fnode[dep_fnode].depens.clone(),
                            );
                            self.state
                                .dep_graph
                                .entry(dep_fnode.clone())
                                .or_insert_with(|| sub_depens);
                        }
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
            bail!("dependency cycle detected: {}", cycle.join(" → "));
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

/// Resolve the path for a new `.mdoc` file given a relative target (no extension).
/// Returns the absolute path with `.mdoc` appended to the last component.
fn resolve_new_node_path(mdcroot: &Path, raw_target: &str, fnode: &str) -> Result<PathBuf> {
    let target = raw_target.trim();
    if target.is_empty() || target == "." {
        return Ok(mdcroot.join(format!("{fnode}.mdoc")));
    }
    let rel = Path::new(target);
    if rel.is_absolute() {
        bail!("target path must be relative to the mdoc root");
    }
    let joined = mdcroot.join(rel);
    // Verify path stays under root (without requiring existence)
    if joined.strip_prefix(mdcroot).is_err() {
        bail!("target path must be under mdoc root {}", mdcroot.display());
    }
    let stem = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid target path"))?
        .to_string_lossy();
    let final_path = joined.with_file_name(format!("{stem}.mdoc"));

    let parent = final_path.parent().unwrap_or(mdcroot);
    if let Some(nested) = find_nested_mdcroot(mdcroot, parent) {
        bail!(
            "target path is inside nested mdoc root: {}",
            nested.display()
        );
    }
    if final_path.exists() {
        bail!("mdoc file already exists: {}", final_path.display());
    }
    Ok(final_path)
}

fn duplicate_fnode_error(mdcroot: &Path, fnode: &str, paths: &[PathBuf]) -> String {
    let rel_paths: Vec<String> = paths.iter().map(|p| to_rel_path(mdcroot, p)).collect();
    format!(
        "duplicate fnode '{fnode}' found in: {}",
        rel_paths.join(", ")
    )
}
