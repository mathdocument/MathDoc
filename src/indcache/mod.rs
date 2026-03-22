mod discovery;
mod queries;
mod refresh;
mod schema;

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::{
    DependencyItem, DependencyTraversalReport, GraphCheckReport, GraphIssue, GraphRootItem,
};

/// SQLite-backed index of a MathDoc workspace.
pub struct IndCache {
    pub root: PathBuf,
    conn: Connection,
}

impl IndCache {
    /// Open (or create) the index database for the workspace rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        let root = root.canonicalize()?;
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
        Ok(IndCache { root, conn })
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
        let tx = self.conn.transaction()?;
        let rel_path = crate::workspace::to_rel_path(&self.root, file_path);

        // Capture pre-upsert state for incremental updates.
        let old_fnode = queries::fnode_for_path(&tx, &rel_path)?;
        let old_dsts: std::collections::HashSet<String> =
            queries::edge_targets_for_source_path(&tx, &rel_path)?
                .into_iter()
                .collect();

        refresh::upsert_mdoc_row(&tx, &self.root, file_path)?;

        // Post-upsert state.
        let new_fnode = queries::fnode_for_path(&tx, &rel_path)?;
        let new_dsts: std::collections::HashSet<String> =
            queries::edge_targets_for_source_path(&tx, &rel_path)?
                .into_iter()
                .collect();

        // Incremental topo refresh.
        if let Some(ref fnode) = new_fnode {
            refresh::refresh_topo_depth_upward_from(&tx, fnode)?;
        } else {
            refresh::backfill_all_topo_depths(&tx)?;
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

    pub fn exact_fnode_rows(&self, fnode: &str) -> Result<Vec<(String, String, String)>> {
        queries::exact_fnode_rows(&self.conn, fnode)
    }

    pub fn duplicate_fnode_paths(&self, fnode: &str) -> Result<Vec<PathBuf>> {
        let rows = self.exact_fnode_rows(fnode)?;
        Ok(rows
            .into_iter()
            .map(|(_, _, p)| self.root.join(p))
            .collect())
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
            bail!("mdoc reference cannot be empty");
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
                _ => bail!("invalid mdoc file: {}", candidate.display()),
            }
        }

        let rows = queries::resolve_fnode_ref(&self.conn, raw_ref)?
            .ok_or_else(|| anyhow::anyhow!("no mdoc matched reference: {raw_ref}"))?;

        let query_lc = raw_ref.to_lowercase();
        let exact: Vec<_> = rows
            .iter()
            .filter(|(f, _, _)| f.to_lowercase() == query_lc)
            .collect();

        let chosen = if !exact.is_empty() {
            if exact.len() == 1 {
                exact[0]
            } else {
                bail!(
                    "ambiguous mdoc reference '{}', matches: {}",
                    raw_ref,
                    format_ref_preview(&exact)
                );
            }
        } else if rows.len() == 1 {
            &rows[0]
        } else {
            bail!(
                "ambiguous mdoc reference '{}', matches: {}",
                raw_ref,
                format_ref_preview(&rows.iter().collect::<Vec<_>>())
            );
        };
        Ok((
            chosen.0.clone(),
            chosen.1.clone(),
            self.root.join(&chosen.2),
        ))
    }

    /// Like `resolve_ref` but returns only the path (also accepts refs that aren't indexed).
    pub fn resolve_edit_target_path(&self, raw_ref: &str, cwd: Option<&Path>) -> Result<PathBuf> {
        let raw_ref = raw_ref.trim();
        if raw_ref.is_empty() {
            bail!("mdoc reference cannot be empty");
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

    /// If `raw_ref` looks like a path, try to resolve it to an existing file.
    /// Returns `(abs_path, rel_path)` on success.
    fn resolve_existing_path(
        &self,
        raw_ref: &str,
        cwd: &Path,
    ) -> Result<Option<(PathBuf, String)>> {
        if !looks_like_path_ref(raw_ref) {
            return Ok(None);
        }
        let raw_path = PathBuf::from(raw_ref);
        let candidates: Vec<PathBuf> = if raw_path.is_absolute() {
            vec![raw_path.canonicalize().unwrap_or(raw_path)]
        } else {
            vec![
                cwd.join(&raw_path)
                    .canonicalize()
                    .unwrap_or_else(|_| cwd.join(&raw_path)),
                self.root
                    .join(&raw_path)
                    .canonicalize()
                    .unwrap_or_else(|_| self.root.join(&raw_path)),
            ]
        };
        for candidate in candidates {
            if candidate.is_file() {
                let rel_path = self.workspace_rel_path(&candidate)?;
                return Ok(Some((candidate, rel_path)));
            }
        }
        if raw_ref.ends_with(".mdoc") {
            bail!("mdoc file not found: {raw_ref}");
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

fn format_ref_preview(rows: &[&(String, String, String)]) -> String {
    rows.iter()
        .map(|(f, _, p)| format!("{}:{}", &f[..f.len().min(8)], p))
        .collect::<Vec<_>>()
        .join(", ")
}
