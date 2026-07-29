mod derived;
mod discovery;
mod queries;
mod refresh;
mod schema;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::{
    short_fnode, DependencyCandidates, DependencyItem, DependencyTraversalReport, GraphCheckReport,
    GraphIssue, GraphRootItem, NodeDegrees, NodeSummary,
};
use crate::mdocnode::{MdocHead, MdocIdentity, MdocNode};

#[derive(Debug, thiserror::Error)]
pub enum ResolveRefError {
    #[error("mdoc reference cannot be empty")]
    Empty,
    #[error("no mdoc matched reference: {0}")]
    NotFound(String),
    #[error("ambiguous mdoc reference '{reference}', matches: {matches}")]
    Ambiguous { reference: String, matches: String },
    #[error("invalid mdoc file: {0}")]
    Invalid(String),
}

/// SQLite-backed index of a MathDoc workspace.
pub struct IndCache {
    root: PathBuf,
    control_identity: (u64, u64),
    conn: Connection,
}

impl IndCache {
    /// Open (or create) the index database for the workspace rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        let _profile = crate::profile::scope("IndCache::open");
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
        Self::open_under_mutation_lock(&mutation_lock)
    }

    pub(crate) fn open_under_mutation_lock(
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<Self> {
        let root = mutation_lock.root()?.to_path_buf();
        let control_identity = mutation_lock.control_identity()?;
        let db_path = root.join(".mdc").join("index.db");
        let conn = schema::open_db(&db_path)?;
        let mut cache = IndCache {
            root,
            control_identity,
            conn,
        };
        cache.bootstrap_if_needed()?;
        Ok(cache)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn acquire_mutation_lock(&self) -> Result<crate::workspace::WorkspaceMutationLock> {
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&self.root)?;
        self.validate_mutation_lock(&mutation_lock)?;
        Ok(mutation_lock)
    }

    pub(crate) fn validate_mutation_lock(
        &self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<()> {
        mutation_lock.validate_identity(&self.root, self.control_identity)
    }

    // ── Bootstrap / refresh ──────────────────────────────────────────────────

    /// Bootstrap the index on first use; no-op if already bootstrapped.
    fn bootstrap_if_needed(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::bootstrap_if_needed");
        if !queries::is_bootstrapped(&self.conn)? {
            let tx = self.conn.transaction()?;
            refresh::refresh_search_index(&tx, &self.root)?;
            let _commit = crate::profile::scope("sqlite::bootstrap_commit");
            tx.commit()?;
        }
        Ok(())
    }

    /// Full workspace rescan; rebuilds the entire index.
    pub fn refresh_all(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::refresh_all");
        let tx = self.conn.transaction()?;
        refresh::refresh_search_index(&tx, &self.root)?;
        let _commit = crate::profile::scope("sqlite::refresh_all_commit");
        tx.commit()?;
        Ok(())
    }

    /// Discover additions, deletions, and metadata changes; marks bootstrapped.
    pub fn discover_workspace_changes(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::discover_workspace_changes");
        let tx = self.conn.transaction()?;
        let (changed_fnodes, has_deletion) =
            discovery::discover_workspace_changes(&tx, &self.root)?;
        if has_deletion {
            // Deletions can decrease ancestor depths; full backfill is needed.
            derived::backfill_all_topo_depths(&tx)?;
        } else {
            // Additions/updates: incremental upward BFS per changed fnode.
            for fnode in &changed_fnodes {
                derived::refresh_topo_depth_upward_from(&tx, fnode)?;
            }
        }
        tx.execute(
            "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Upsert a single file path and update its topo depths.
    pub fn upsert_path(&mut self, file_path: &Path) -> Result<()> {
        let file_path = crate::workspace::resolve_mdoc_path(&self.root, file_path)?;
        let tx = self.conn.transaction()?;
        let rel_path = crate::workspace::to_rel_path(&self.root, &file_path);

        // Capture pre-upsert identity for incremental topo updates.
        let old_fnode = queries::fnode_for_path(&tx, &rel_path)?;

        refresh::upsert_mdoc_row(&tx, &self.root, &file_path)?;

        // Post-upsert identity.
        let new_fnode = queries::fnode_for_path(&tx, &rel_path)?;

        // A rename affects both ancestors of the old token and the new node.
        match (old_fnode.as_deref(), new_fnode.as_deref()) {
            (Some(old), Some(new)) if old != new => {
                derived::refresh_topo_depth_upward_from(&tx, old)?;
                derived::refresh_topo_depth_upward_from(&tx, new)?;
            }
            (_, Some(new)) => derived::refresh_topo_depth_upward_from(&tx, new)?,
            (_, None) => derived::backfill_all_topo_depths(&tx)?,
        }

        tx.commit()?;
        Ok(())
    }

    /// Create a node and index it as one recoverable operation.
    pub(crate) fn create_node(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        node: &MdocNode,
    ) -> Result<crate::workspace::AppliedWrite> {
        self.validate_mutation_lock(mutation_lock)?;
        let path = self.validate_node_path(node)?;
        let payload = node.render()?;

        if let Some(parent) = path.parent() {
            crate::workspace::ensure_regular_directory_tree(&self.root, parent)
                .with_context(|| format!("creating parent dirs for {}", path.display()))?;
        }
        self.validate_mutation_lock(mutation_lock)?;
        let path = self.validate_node_path(node)?;
        self.write_and_index_node(
            &path,
            payload.as_bytes(),
            &crate::workspace::FileSnapshot::Missing,
        )
    }

    /// Replace a node and update its index entry as one recoverable operation.
    pub(crate) fn replace_node(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        node: &MdocNode,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<()> {
        self.validate_mutation_lock(mutation_lock)?;
        let path = self.validate_node_path(node)?;
        let payload = node.render()?;
        self.write_and_index_node(&path, payload.as_bytes(), snapshot)?;
        Ok(())
    }

    fn validate_node_path(&self, node: &MdocNode) -> Result<PathBuf> {
        crate::workspace::resolve_mdoc_path(&self.root, &node.path)
    }

    fn write_and_index_node(
        &mut self,
        path: &Path,
        payload: &[u8],
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<crate::workspace::AppliedWrite> {
        let created = matches!(snapshot, crate::workspace::FileSnapshot::Missing);
        let applied = snapshot.replace_beneath(&self.root, path, payload)?;
        let Err(index_error) = self.upsert_path(path) else {
            return Ok(applied);
        };

        let rollback_result = applied.rollback();
        let restore_index_result = self.upsert_path(path);
        let rollback_action = if created { "remove" } else { "restore" };
        Err(crate::workspace::PersistenceRecoveryError::from_attempts(
            index_error,
            rollback_result,
            restore_index_result,
            &format!("{rollback_action} {}", path.display()),
            &format!("repair the index for {}", path.display()),
        ))
    }

    /// Upsert all dependencies reachable from `root_path` up to `depth` hops (-1 = infinite).
    pub fn refresh_reachable_from_path(&mut self, root_path: &Path, depth: i32) -> Result<()> {
        let tx = self.conn.transaction()?;
        let upserted_fnodes =
            refresh::refresh_reachable_from_path(&tx, &self.root, root_path, depth)?;
        // Incremental topo update for each upserted fnode. Weak components are
        // rebuilt lazily from the dirty flag when roots are queried.
        for fnode in &upserted_fnodes {
            derived::refresh_topo_depth_upward_from(&tx, fnode)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ── Read queries ─────────────────────────────────────────────────────────

    pub fn count(&self) -> Result<u32> {
        queries::mdoc_count(&self.conn)
    }

    pub fn path_has_blocking_issue(&self, rel_path: &str) -> Result<bool> {
        queries::path_has_blocking_issue(&self.conn, rel_path)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<NodeSummary>> {
        queries::search(&self.conn, query, limit)
    }

    pub fn all_node_summaries(&self) -> Result<Vec<NodeSummary>> {
        queries::all_node_summaries(&self.conn)
    }

    pub fn dependency_candidates(
        &self,
        source_fnode: &str,
        query: &str,
        limit: usize,
    ) -> Result<DependencyCandidates> {
        queries::dependency_candidates(&self.conn, source_fnode, query, limit)
    }

    pub fn node_summary(&self, fnode: &str) -> Result<NodeSummary> {
        queries::node_summary(&self.conn, fnode)
    }

    pub fn node_degrees(&self, fnode: &str) -> Result<NodeDegrees> {
        queries::node_degrees(&self.conn, fnode)
    }

    #[cfg(test)]
    fn exact_fnode_rows(&self, fnode: &str) -> Result<Vec<(String, String, String)>> {
        queries::exact_fnode_rows(&self.conn, fnode)
    }

    pub fn reconcile_fnode_paths(&mut self, fnode: &str) -> Result<Vec<PathBuf>> {
        let tx = self.conn.transaction()?;
        let rows = queries::exact_fnode_rows(&tx, fnode)?;
        let mut paths = Vec::new();
        let mut stale = false;
        for (_, _, rel_path) in rows {
            if let Ok(path) = refresh::validate_cached_mdoc_path(&self.root, &rel_path) {
                paths.push(path);
            } else {
                refresh::delete_indexed_path(&tx, &rel_path)?;
                stale = true;
            }
        }
        if stale {
            derived::refresh_topo_depth_upward_from(&tx, fnode)?;
        }
        tx.commit()?;
        Ok(paths)
    }

    pub fn lookup_by_fnode(&self, fnodes: &[&str]) -> Result<HashMap<String, (String, String)>> {
        queries::lookup_by_fnode(&self.conn, fnodes)
    }

    pub fn issue_for_fnode(&self, fnode: &str) -> Result<Option<GraphIssue>> {
        queries::issue_for_fnode(&self.conn, fnode)
    }

    pub fn ref_item_for_fnode(&self, fnode: &str, depth: u32) -> Result<DependencyItem> {
        queries::ref_item_for_fnode(&self.conn, fnode, depth)
    }

    pub(crate) fn ref_items_for_fnodes(
        &self,
        fnodes: &[String],
        depth: u32,
    ) -> Result<Vec<DependencyItem>> {
        let fnodes: Vec<&str> = fnodes.iter().map(String::as_str).collect();
        queries::ref_items_for_fnodes(&self.conn, &fnodes, depth)
    }

    pub fn referrer_items(&self, target_fnode: &str, depth: i32) -> Result<Vec<DependencyItem>> {
        queries::referrer_items(&self.conn, target_fnode, depth)
    }

    pub fn direct_referrer_summaries(&self, fnode: &str) -> Result<Vec<NodeSummary>> {
        queries::direct_referrer_summaries(&self.conn, fnode)
    }

    pub fn direct_dependency_summaries(&self, fnode: &str) -> Result<Vec<NodeSummary>> {
        queries::direct_dependency_summaries(&self.conn, fnode)
    }

    /// All dependency edges whose source document has no blocking issue.
    pub fn all_valid_edges(&self) -> Result<Vec<(String, String)>> {
        queries::all_valid_edges(&self.conn)
    }

    pub fn is_reachable(&self, from_fnode: &str, to_fnode: &str) -> Result<bool> {
        queries::is_reachable(&self.conn, from_fnode, to_fnode)
    }

    pub fn dependency_report(
        &self,
        root_fnode: &str,
        depth: i32,
    ) -> Result<DependencyTraversalReport> {
        queries::dependency_report(&self.conn, root_fnode, depth)
    }

    pub fn leaf_dependency_report(&self, root_fnode: &str) -> Result<DependencyTraversalReport> {
        queries::leaf_dependency_report(&self.conn, root_fnode)
    }

    // ── Write-then-read (need &mut for transaction) ───────────────────────────

    pub fn global_root_items(&mut self) -> Result<Vec<GraphRootItem>> {
        let tx = self.conn.transaction()?;
        derived::ensure_weak_components(&tx)?;
        let result = queries::global_root_items(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn graph_check_report(&mut self) -> Result<GraphCheckReport> {
        let _profile = crate::profile::scope("IndCache::graph_check_report");
        let tx = self.conn.transaction()?;
        let cycles = derived::ensure_scc_cache(&tx)?;
        let result = queries::graph_check_report(&tx, cycles)?;
        tx.commit()?;
        Ok(result)
    }

    // ── Reference resolution ─────────────────────────────────────────────────

    /// Resolve a reference string to `(fnode, title, abs_path)`.
    ///
    /// The reference may be:
    /// - A path-like string (contains `/`, ends in `.mdoc`, or starts with `.`)
    /// - An fnode or fnode prefix
    pub fn resolve_ref(
        &self,
        raw_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(String, String, PathBuf)> {
        let raw_ref = raw_ref.trim();
        if raw_ref.is_empty() {
            return Err(ResolveRefError::Empty.into());
        }
        let base_cwd = cwd
            .map(|c| c.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let base_cwd = base_cwd.canonicalize().unwrap_or(base_cwd);

        if let Some((candidate, rel_path)) = self.resolve_existing_path(raw_ref, &base_cwd)? {
            if let Some((fnode, title)) = queries::resolve_ref_by_path(&self.conn, &rel_path)? {
                return Ok((fnode, title, candidate));
            }
            let snapshot = crate::workspace::FileSnapshot::capture(&candidate)?;
            let content = snapshot
                .content()
                .ok_or_else(|| ResolveRefError::Invalid(candidate.display().to_string()))?;
            match MdocHead::load_bytes(&candidate, content) {
                Ok(head) => return Ok((head.fnode, head.title, candidate)),
                Err(_) => {
                    let identity = MdocIdentity::from_bytes(content);
                    if let Some((fnode, title)) = identity.complete() {
                        return Ok((fnode.to_string(), title.to_string(), candidate));
                    }
                    return Err(ResolveRefError::Invalid(candidate.display().to_string()).into());
                }
            }
        }

        let untrusted_rows = queries::resolve_fnode_ref(&self.conn, raw_ref)?
            .ok_or_else(|| ResolveRefError::NotFound(raw_ref.to_string()))?;
        let mut rows = Vec::new();
        for (fnode, title, rel_path) in untrusted_rows {
            if let Some(path) = self.valid_cached_path(&rel_path) {
                rows.push((fnode, title, rel_path, path));
            }
        }
        if rows.is_empty() {
            return Err(ResolveRefError::NotFound(raw_ref.to_string()).into());
        }

        let query_lc = raw_ref.to_lowercase();
        let exact: Vec<_> = rows
            .iter()
            .filter(|(f, _, _, _)| f.to_lowercase() == query_lc)
            .collect();

        let chosen = if !exact.is_empty() {
            if exact.len() == 1 {
                exact[0]
            } else {
                return Err(ResolveRefError::Ambiguous {
                    reference: raw_ref.to_string(),
                    matches: format_ref_preview(&exact),
                }
                .into());
            }
        } else if rows.len() == 1 {
            &rows[0]
        } else {
            return Err(ResolveRefError::Ambiguous {
                reference: raw_ref.to_string(),
                matches: format_ref_preview(&rows.iter().collect::<Vec<_>>()),
            }
            .into());
        };
        Ok((chosen.0.clone(), chosen.1.clone(), chosen.3.clone()))
    }

    /// Resolve a browser start reference, additionally accepting a unique exact title.
    pub fn resolve_start_ref(
        &self,
        raw_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(String, String, PathBuf)> {
        let ref_error = match self.resolve_ref(raw_ref, cwd) {
            Ok(resolved) => return Ok(resolved),
            Err(error) => error,
        };
        let raw_ref = raw_ref.trim();
        if raw_ref.is_empty() {
            return Err(ref_error);
        }

        let mut rows = Vec::new();
        for (fnode, title, rel_path) in queries::exact_title_rows(&self.conn, raw_ref)? {
            if let Some(path) = self.valid_cached_path(&rel_path) {
                rows.push((fnode, title, rel_path, path));
            }
        }
        match rows.as_slice() {
            [(fnode, title, _, path)] => Ok((fnode.clone(), title.clone(), path.clone())),
            [] => Err(ref_error),
            _ => Err(ResolveRefError::Ambiguous {
                reference: raw_ref.to_string(),
                matches: format_ref_preview(&rows.iter().collect::<Vec<_>>()),
            }
            .into()),
        }
    }

    /// Like `resolve_ref` but returns only the path (also accepts refs that aren't indexed).
    pub fn resolve_edit_target_path(&self, raw_ref: &str, cwd: Option<&Path>) -> Result<PathBuf> {
        let raw_ref = raw_ref.trim();
        if raw_ref.is_empty() {
            return Err(ResolveRefError::Empty.into());
        }
        let base_cwd = cwd
            .map(|c| c.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let base_cwd = base_cwd.canonicalize().unwrap_or(base_cwd);
        if let Some((candidate, _)) = self.resolve_existing_path(raw_ref, &base_cwd)? {
            return Ok(candidate);
        }
        let (_, _, path) = self.resolve_ref(raw_ref, Some(&base_cwd))?;
        Ok(path)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn valid_cached_path(&self, rel_path: &str) -> Option<PathBuf> {
        refresh::validate_cached_mdoc_path(&self.root, rel_path).ok()
    }

    /// If `raw_ref` looks like a path, try to resolve it to an existing file.
    /// Returns `(abs_path, rel_path)` on success.
    fn resolve_existing_path(
        &self,
        raw_ref: &str,
        cwd: &Path,
    ) -> Result<Option<(PathBuf, String)>> {
        let mut raw_path = PathBuf::from(raw_ref);
        if !looks_like_path_ref(raw_ref) && raw_path.extension().is_some() {
            return Ok(None);
        }
        if raw_path.extension().is_none() {
            raw_path.set_extension("mdoc");
        }
        let candidates: Vec<PathBuf> = if raw_path.is_absolute() {
            vec![raw_path]
        } else {
            vec![cwd.join(&raw_path), self.root.join(&raw_path)]
        };
        for candidate in candidates {
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    let resolved = crate::workspace::resolve_mdoc_path(&self.root, &candidate)?;
                    let meta = std::fs::symlink_metadata(&resolved)?;
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        bail!("mdoc path is not a regular file: {}", candidate.display());
                    }
                    let rel_path = resolved
                        .strip_prefix(&self.root)
                        .expect("resolved mdoc path belongs to the cache root")
                        .to_string_lossy()
                        .into_owned();
                    return Ok(Some((resolved, rel_path)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting {}", candidate.display()))
                }
            }
        }
        if raw_ref.ends_with(".mdoc") {
            return Err(ResolveRefError::NotFound(raw_ref.to_string()).into());
        }
        Ok(None)
    }
}

fn looks_like_path_ref(raw_ref: &str) -> bool {
    raw_ref.contains('/') || raw_ref.ends_with(".mdoc") || raw_ref.starts_with('.')
}

fn format_ref_preview(rows: &[&(String, String, String, PathBuf)]) -> String {
    rows.iter()
        .map(|(f, _, p, _)| format!("{}:{}", short_fnode(f), p))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod mutation_boundary_tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".mdc")).unwrap();
        dir
    }

    fn write_node(node: &MdocNode) {
        std::fs::write(&node.path, node.render().unwrap()).unwrap();
    }

    #[test]
    fn create_rejects_a_mutation_lock_from_another_cache() {
        let first = workspace();
        let second = workspace();
        let mut cache = IndCache::open(first.path().to_path_buf()).unwrap();
        let other_cache = IndCache::open(second.path().to_path_buf()).unwrap();
        let other_lock = other_cache.acquire_mutation_lock().unwrap();
        let path = first.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&other_lock, &node).unwrap_err();

        assert!(error.to_string().contains("does not match cache root"));
        assert!(!path.exists());
    }

    #[test]
    fn create_rejects_a_node_from_another_workspace_before_writing() {
        let first = workspace();
        let second = workspace();
        let mut cache = IndCache::open(first.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = second.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("outside workspace"));
        assert!(!path.exists());
    }

    #[test]
    fn create_builds_parents_and_indexes_the_validated_path() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("notes/nested/node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();

        cache.create_node(&mutation_lock, &node).unwrap();

        assert!(path.is_file());
        assert_eq!(cache.exact_fnode_rows("created-node").unwrap().len(), 1);
    }

    #[test]
    fn create_rolls_back_file_and_recovers_index_after_index_failure() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_created_node
                 BEFORE INSERT ON mdocs
                 WHEN NEW.fnode = 'created-node'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected index failure');
                 END;",
            )
            .unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("created.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("injected index failure"));
        assert!(!path.exists());
        assert!(cache.exact_fnode_rows("created-node").unwrap().is_empty());
    }

    #[test]
    fn create_recovery_preserves_typed_rollback_conflict() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_created_node
                 BEFORE INSERT ON mdocs
                 WHEN NEW.fnode = 'created-node'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected index failure');
                 END;",
            )
            .unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("created.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();
        let edited_path = path.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::RollbackAfterInitialVerification,
            move || {
                std::fs::write(
                    edited_path,
                    "@fnode: external-node\n@title: External edit\n",
                )
                .unwrap();
            },
        );

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(crate::workspace::error_has_infrastructure_failure(&error));
        assert_eq!(MdocNode::load(&path).unwrap().title, "External edit");
        assert_eq!(
            cache.node_summary("external-node").unwrap().title,
            "External edit"
        );
    }

    #[test]
    fn replace_rejects_an_outside_path_before_writing() {
        let workspace = workspace();
        let outside = tempfile::TempDir::new().unwrap();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = outside.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache
            .replace_node(
                &mutation_lock,
                &node,
                &crate::workspace::FileSnapshot::Missing,
            )
            .unwrap_err();

        assert!(error.to_string().contains("outside workspace"));
        assert!(!path.exists());
    }

    #[test]
    fn stale_replace_preserves_external_edit() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "stale-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();

        let mut desired = node.clone();
        desired.title = "Desired".to_string();
        let mut external = node.clone();
        external.title = "External edit".to_string();
        write_node(&external);

        let error = cache
            .replace_node(&mutation_lock, &desired, &snapshot)
            .unwrap_err();

        assert!(error
            .downcast_ref::<crate::workspace::FileConflict>()
            .is_some());
        assert_eq!(MdocNode::load(&path).unwrap().title, "External edit");
    }

    #[cfg(unix)]
    #[test]
    fn replace_rejects_read_only_files() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("readonly.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "readonly-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();
        node.title = "Changed".to_string();

        let error = cache
            .replace_node(&mutation_lock, &node, &snapshot)
            .unwrap_err();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(error.to_string().contains("read-only"));
        assert_eq!(MdocNode::load(&path).unwrap().title, "Original");
    }

    #[cfg(unix)]
    #[test]
    fn replace_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("metadata.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "metadata-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();
        node.title = "Changed".to_string();

        cache
            .replace_node(&mutation_lock, &node, &snapshot)
            .unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(MdocNode::load(&path).unwrap().title, "Changed");
    }

    #[cfg(unix)]
    #[test]
    fn create_rejects_final_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let workspace = workspace();
        let outside = tempfile::TempDir::new().unwrap();
        let outside_path = outside.path().join("victim.mdoc");
        std::fs::write(&outside_path, "victim").unwrap();
        let path = workspace.path().join("link.mdoc");
        symlink(&outside_path, &path).unwrap();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let node = MdocNode::new_at_path(&path, "Node");

        assert!(cache.create_node(&mutation_lock, &node).is_err());
        assert_eq!(std::fs::read_to_string(outside_path).unwrap(), "victim");
        assert!(std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn ancestor_replacement_cannot_redirect_indexed_create_or_replace() {
        use std::os::unix::fs::symlink;

        for replacing in [false, true] {
            let workspace = workspace();
            let outside = tempfile::TempDir::new().unwrap();
            let ancestor = workspace.path().join("ancestor");
            let parent = ancestor.join("parent");
            std::fs::create_dir_all(&parent).unwrap();
            std::fs::create_dir(outside.path().join("parent")).unwrap();
            let path = parent.join("node.mdoc");
            let outside_path = outside.path().join("parent/node.mdoc");
            let displaced = workspace.path().join("displaced");
            let mut node = MdocNode::new_at_path(&path, "After");
            node.fnode = "ancestor-race-node".to_string();
            let snapshot = if replacing {
                let mut before = node.clone();
                before.title = "Before".to_string();
                write_node(&before);
                std::fs::write(&outside_path, b"outside victim").unwrap();
                crate::workspace::FileSnapshot::capture(&path).unwrap()
            } else {
                crate::workspace::FileSnapshot::Missing
            };
            let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
            let mutation_lock = cache.acquire_mutation_lock().unwrap();
            let hook_ancestor = ancestor.clone();
            let hook_outside = outside.path().to_path_buf();
            let hook_displaced = displaced.clone();
            crate::workspace::set_test_hook(
                crate::workspace::TestHookPoint::WriteBeforeDirectoryBinding,
                move || {
                    std::fs::rename(&hook_ancestor, &hook_displaced).unwrap();
                    symlink(&hook_outside, &hook_ancestor).unwrap();
                },
            );

            let error = if replacing {
                cache
                    .replace_node(&mutation_lock, &node, &snapshot)
                    .unwrap_err()
            } else {
                cache.create_node(&mutation_lock, &node).unwrap_err()
            };

            assert!(crate::workspace::error_has_file_conflict(&error));
            if replacing {
                assert_eq!(std::fs::read(&outside_path).unwrap(), b"outside victim");
                assert_eq!(
                    MdocNode::load(&displaced.join("parent/node.mdoc"))
                        .unwrap()
                        .title,
                    "Before"
                );
            } else {
                assert!(!outside_path.exists());
                assert!(!displaced.join("parent/node.mdoc").exists());
            }
        }
    }

    #[test]
    fn cache_rejects_a_replaced_control_directory() {
        let workspace = workspace();
        let cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        std::fs::rename(
            workspace.path().join(".mdc"),
            workspace.path().join("old-mdc"),
        )
        .unwrap();
        std::fs::create_dir(workspace.path().join(".mdc")).unwrap();

        let error = match cache.acquire_mutation_lock() {
            Err(error) => error,
            Ok(_) => panic!("expected replaced control directory to be rejected"),
        };

        assert!(error.to_string().contains("does not match the cache"));
    }
}
