mod discovery;
mod queries;
mod refresh;
mod schema;

pub(crate) use refresh::resolve_workspace_path;

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::{
    short_fnode, DependencyItem, DependencyTraversalReport, GraphCheckReport, GraphIssue,
    GraphRootItem,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub broken: bool,
    pub depth: u32,
}

/// SQLite-backed index of a MathDoc workspace.
pub struct IndCache {
    pub root: PathBuf,
    conn: Connection,
}

impl IndCache {
    /// Open (or create) the index database for the workspace rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        let root = crate::workspace::validate_mdcroot(&root)?;
        let _mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
        Self::open_under_mutation_lock(root)
    }

    pub(crate) fn open_under_mutation_lock(root: PathBuf) -> Result<Self> {
        let root = crate::workspace::validate_mdcroot(&root)?;
        let db_path = root.join(".mdc").join("index.db");
        let (mut conn, needs_topo_backfill) = schema::open_db(&db_path)?;
        if needs_topo_backfill {
            // topo_depth values are all-zero (column newly added or prior crash).
            // Backfill real depths and mark complete in the same transaction so a
            // crash here leaves topo_depth_backfilled = 0 and triggers recovery on
            // the next open.
            let tx = conn.transaction()?;
            refresh::backfill_all_topo_depths(&tx)?;
            tx.execute(
                "UPDATE mdoc_index_state SET topo_depth_backfilled = 1 WHERE id = 1",
                [],
            )?;
            tx.commit()?;
        }
        let mut cache = IndCache { root, conn };
        cache.bootstrap_if_needed()?;
        Ok(cache)
    }

    /// Absolute path to the SQLite database file.
    pub fn db_path(&self) -> PathBuf {
        self.root.join(".mdc").join("index.db")
    }

    // ── Bootstrap / refresh ──────────────────────────────────────────────────

    /// Bootstrap the index on first use; no-op if already bootstrapped.
    pub fn bootstrap_if_needed(&mut self) -> Result<()> {
        if !queries::is_bootstrapped(&self.conn)? {
            let tx = self.conn.transaction()?;
            refresh::refresh_search_index(&tx, &self.root)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Full workspace rescan; rebuilds the entire index.
    pub fn refresh_all(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        refresh::refresh_search_index(&tx, &self.root)?;
        tx.commit()?;
        Ok(())
    }

    /// Incremental discovery using directory mtime tracking; marks bootstrapped.
    pub fn discover_workspace_changes(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        let (changed_fnodes, has_deletion) =
            discovery::discover_workspace_changes(&tx, &self.root)?;
        if has_deletion {
            // Deletions can decrease ancestor depths; full backfill is needed.
            refresh::backfill_all_topo_depths(&tx)?;
        } else {
            // Additions/updates: incremental upward BFS per changed fnode.
            for fnode in &changed_fnodes {
                refresh::refresh_topo_depth_upward_from(&tx, fnode)?;
            }
        }
        tx.execute(
            "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Incremental discovery + re-stat all indexed paths.
    pub fn refresh_workspace_index(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        discovery::discover_workspace_changes(&tx, &self.root)?;
        refresh::refresh_indexed_paths(&tx, &self.root)?;
        tx.execute(
            "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Upsert a single file path with incremental topo and weak component updates.
    pub fn upsert_path(&mut self, file_path: &Path) -> Result<()> {
        let file_path = resolve_workspace_path(&self.root, file_path)?;
        let tx = self.conn.transaction()?;
        let rel_path = crate::workspace::to_rel_path(&self.root, &file_path);

        // Capture pre-upsert state for incremental updates.
        let old_fnode = queries::fnode_for_path(&tx, &rel_path)?;
        let old_dsts: std::collections::HashSet<String> =
            queries::edge_targets_for_source_path(&tx, &rel_path)?
                .into_iter()
                .collect();

        refresh::upsert_mdoc_row(&tx, &self.root, &file_path)?;

        // Post-upsert state.
        let new_fnode = queries::fnode_for_path(&tx, &rel_path)?;
        let new_dsts: std::collections::HashSet<String> =
            queries::edge_targets_for_source_path(&tx, &rel_path)?
                .into_iter()
                .collect();

        // A rename affects both ancestors of the old token and the new node.
        match (old_fnode.as_deref(), new_fnode.as_deref()) {
            (Some(old), Some(new)) if old != new => {
                refresh::refresh_topo_depth_upward_from(&tx, old)?;
                refresh::refresh_topo_depth_upward_from(&tx, new)?;
            }
            (_, Some(new)) => refresh::refresh_topo_depth_upward_from(&tx, new)?,
            (_, None) => refresh::backfill_all_topo_depths(&tx)?,
        }

        // Incremental weak component update (also clears weak_component_dirty).
        refresh::update_weak_component_incremental(
            &tx,
            old_fnode.as_deref(),
            new_fnode.as_deref(),
            &old_dsts,
            &new_dsts,
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Upsert all dependencies reachable from `root_path` up to `depth` hops (-1 = infinite).
    pub fn refresh_reachable_from_path(&mut self, root_path: &Path, depth: i32) -> Result<()> {
        let tx = self.conn.transaction()?;
        let upserted_fnodes =
            refresh::refresh_reachable_from_path(&tx, &self.root, root_path, depth)?;
        // Incremental topo update for each upserted fnode; weak components are handled
        // lazily via the weak_component_dirty flag already set by bump_graph_epoch.
        for fnode in &upserted_fnodes {
            refresh::refresh_topo_depth_upward_from(&tx, fnode)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ── Read queries ─────────────────────────────────────────────────────────

    pub fn count(&self) -> Result<u32> {
        queries::mdoc_count(&self.conn)
    }

    pub fn indexed_file_count(&self) -> Result<u32> {
        queries::indexed_file_count(&self.conn)
    }

    pub fn fnode_for_path(&self, rel_path: &str) -> Result<Option<String>> {
        queries::fnode_for_path(&self.conn, rel_path)
    }

    pub fn path_has_blocking_issue(&self, rel_path: &str) -> Result<bool> {
        queries::path_has_blocking_issue(&self.conn, rel_path)
    }

    pub fn knows_fnode(&self, fnode: &str) -> Result<bool> {
        queries::knows_fnode(&self.conn, fnode)
    }

    pub fn search(&self, query: &str) -> Result<Vec<(String, String, String)>> {
        queries::search(&self.conn, query)
    }

    pub fn search_with_metadata(&self, query: &str, limit: usize) -> Result<Vec<SearchMatch>> {
        queries::search_with_metadata(&self.conn, query, limit)
    }

    pub fn exact_fnode_rows(&self, fnode: &str) -> Result<Vec<(String, String, String)>> {
        queries::exact_fnode_rows(&self.conn, fnode)
    }

    pub fn duplicate_fnode_paths(&self, fnode: &str) -> Result<Vec<PathBuf>> {
        let rows = self.exact_fnode_rows(fnode)?;
        let mut paths = Vec::new();
        for (_, _, rel_path) in rows {
            if let Some(path) = self.valid_cached_path(&rel_path)? {
                paths.push(path);
            }
        }
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

    pub fn referrer_items(&self, target_fnode: &str, depth: i32) -> Result<Vec<DependencyItem>> {
        queries::referrer_items(&self.conn, target_fnode, depth)
    }

    pub fn direct_referrers_for_fnode(&self, fnode: &str) -> Result<Vec<(String, String, String)>> {
        queries::direct_referrers_for_fnode(&self.conn, fnode)
    }

    pub fn all_topo_depths(&self) -> Result<HashMap<String, u32>> {
        queries::all_topo_depths(&self.conn)
    }

    /// All (src_fnode, dst_fnode) edges between valid nodes.
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

    pub fn has_issues(&self, fnode: &str) -> Result<bool> {
        Ok(self.issue_for_fnode(fnode)?.is_some())
    }

    // ── Write-then-read (need &mut for transaction) ───────────────────────────

    pub fn global_root_items(&mut self) -> Result<Vec<GraphRootItem>> {
        let tx = self.conn.transaction()?;
        let result = queries::global_root_items(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn graph_check_report(&mut self) -> Result<GraphCheckReport> {
        let tx = self.conn.transaction()?;
        let result = queries::graph_check_report(&tx)?;
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
            match crate::mdocnode::read_mdoc_head(&candidate) {
                Some((fnode, title)) if !fnode.is_empty() => return Ok((fnode, title, candidate)),
                _ => return Err(ResolveRefError::Invalid(candidate.display().to_string()).into()),
            }
        }

        let untrusted_rows = queries::resolve_fnode_ref(&self.conn, raw_ref)?
            .ok_or_else(|| ResolveRefError::NotFound(raw_ref.to_string()))?;
        let mut rows = Vec::new();
        for (fnode, title, rel_path) in untrusted_rows {
            if let Some(path) = self.valid_cached_path(&rel_path)? {
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

    fn valid_cached_path(&self, rel_path: &str) -> Result<Option<PathBuf>> {
        match refresh::validate_cached_mdoc_path(&self.root, rel_path) {
            Ok(path) => Ok(Some(path)),
            Err(_) => {
                refresh::delete_indexed_path(&self.conn, rel_path)?;
                Ok(None)
            }
        }
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
            if std::fs::symlink_metadata(&candidate).is_ok() {
                let resolved = resolve_workspace_path(&self.root, &candidate)?;
                let meta = std::fs::symlink_metadata(&resolved)?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    bail!("mdoc path is not a regular file: {}", candidate.display());
                }
                let rel_path = self.workspace_rel_path(&resolved)?;
                return Ok(Some((resolved, rel_path)));
            }
        }
        if raw_ref.ends_with(".mdoc") {
            return Err(ResolveRefError::NotFound(raw_ref.to_string()).into());
        }
        Ok(None)
    }

    fn workspace_rel_path(&self, candidate: &Path) -> Result<String> {
        let parent = candidate.parent().unwrap_or(candidate);
        if let Some(nested) = crate::workspace::find_nested_mdcroot(&self.root, parent) {
            bail!("mdoc path is inside nested mdoc root: {}", nested.display());
        }
        candidate
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|_| {
                anyhow::anyhow!("mdoc path must be under mdoc root: {}", self.root.display())
            })
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
