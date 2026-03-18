//! Incremental workspace change detection using directory mtime tracking.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use rusqlite::Connection;

use super::queries::fnode_for_path;
use super::refresh::{delete_indexed_path, upsert_mdoc_row};

// ── Public API ────────────────────────────────────────────────────────────────

/// Efficiently detect and index workspace changes using directory mtime comparison.
///
/// Returns `(changed_fnodes, has_deletion)`:
/// - `changed_fnodes`: fnodes of added/updated files (use for incremental topo).
/// - `has_deletion`: true if any files were deleted (requires full topo backfill).
pub fn discover_workspace_changes(conn: &Connection, root: &Path) -> Result<(Vec<String>, bool)> {
    let mut state = DiscoveryState {
        known_dirs: dir_mtimes(conn)?,
        known_file_states: file_states(conn)?,
        child_dirs_by_parent: HashMap::new(),
        files_by_parent: HashMap::new(),
        seen_dirs: HashSet::new(),
        changed_fnodes: Vec::new(),
        has_deletion: false,
    };
    state.child_dirs_by_parent = group_dirs_by_parent(&state.known_dirs);
    state.files_by_parent = group_files_by_parent(&state.known_file_states);

    // Iterative DFS over workspace directories
    let mut stack: Vec<String> = vec![String::new()];
    while let Some(rel_dir) = stack.pop() {
        scan_dir_step(conn, root, &rel_dir, &mut state, &mut stack)?;
    }

    // Purge directories that were never visited (deleted or became nested roots)
    let mut stale_dirs: Vec<String> = state
        .known_dirs
        .keys()
        .filter(|d| !state.seen_dirs.contains(*d))
        .cloned()
        .collect();
    stale_dirs.sort();
    stale_dirs.reverse(); // deepest first
    for stale_dir in stale_dirs {
        purge_subtree(conn, &stale_dir, &mut state)?;
    }
    Ok((state.changed_fnodes, state.has_deletion))
}

/// Rebuild the mdoc_dirs table from scratch by walking the workspace.
pub fn rebuild_directory_index(conn: &Connection, root: &Path) -> Result<()> {
    conn.execute("DELETE FROM mdoc_dirs", [])?;
    for (rel_dir, mtime_ns) in scan_workspace_dirs(root) {
        conn.execute(
            "INSERT INTO mdoc_dirs (path, mtime_ns)
             VALUES (?, ?)
             ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns",
            rusqlite::params![rel_dir, mtime_ns],
        )?;
    }
    Ok(())
}

// ── Directory scan state ──────────────────────────────────────────────────────

struct DiscoveryState {
    known_dirs: HashMap<String, i64>,
    known_file_states: HashMap<String, (i64, i64)>,
    child_dirs_by_parent: HashMap<String, HashSet<String>>,
    files_by_parent: HashMap<String, HashSet<String>>,
    seen_dirs: HashSet<String>,
    /// Fnodes of files that were added or updated this scan (for incremental topo).
    changed_fnodes: Vec<String>,
    /// True if any file was deleted; deletions require a full topo backfill because
    /// ancestor depths may decrease and incremental propagation is not monotone-safe.
    has_deletion: bool,
}

fn scan_dir_step(
    conn: &Connection,
    root: &Path,
    rel_dir: &str,
    state: &mut DiscoveryState,
    stack: &mut Vec<String>,
) -> Result<()> {
    let dir_path = if rel_dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel_dir)
    };

    // Nested workspace root — purge and stop descending
    if !rel_dir.is_empty() && dir_path.join(".mdc").is_dir() {
        purge_subtree(conn, rel_dir, state)?;
        return Ok(());
    }

    // Gone — purge
    if !dir_path.is_dir() {
        purge_subtree(conn, rel_dir, state)?;
        return Ok(());
    }

    let meta = match std::fs::metadata(&dir_path) {
        Ok(m) => m,
        Err(_) => {
            purge_subtree(conn, rel_dir, state)?;
            return Ok(());
        }
    };

    state.seen_dirs.insert(rel_dir.to_string());
    let current_mtime_ns = mtime_ns_from_meta(&meta);
    let known_mtime_ns = state.known_dirs.get(rel_dir).copied();

    // Unchanged known directory: just push known children
    if known_mtime_ns == Some(current_mtime_ns) {
        let mut children: Vec<String> = state
            .child_dirs_by_parent
            .get(rel_dir)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        children.sort();
        children.reverse(); // push in reverse so sorted order is processed first
        for child in children {
            stack.push(child);
        }
        return Ok(());
    }

    // Directory is new or changed: scan its entries
    let entries = match std::fs::read_dir(&dir_path) {
        Ok(e) => e,
        Err(_) => {
            purge_subtree(conn, rel_dir, state)?;
            return Ok(());
        }
    };

    let mut entry_list: Vec<std::fs::DirEntry> = entries.filter_map(|e| e.ok()).collect();
    entry_list.sort_by_key(|e| e.file_name());

    let mut discovered_child_dirs: HashSet<String> = HashSet::new();
    let mut seen_files: HashSet<String> = HashSet::new();

    for entry in &entry_list {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".mdc" {
            continue;
        }
        let child_rel = join_rel_dir(rel_dir, &name);
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            discovered_child_dirs.insert(child_rel);
            continue;
        }
        if !ft.is_file() || !name.ends_with(".mdoc") {
            continue;
        }
        seen_files.insert(child_rel.clone());

        let entry_meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let current_state = (mtime_ns_from_meta(&entry_meta), entry_meta.len() as i64);
        if state.known_file_states.get(&child_rel).copied() == Some(current_state) {
            continue;
        }
        upsert_mdoc_row(conn, root, &dir_path.join(&name))?;
        if let Some(fnode) = fnode_for_path(conn, &child_rel)? {
            state.changed_fnodes.push(fnode);
        }
        state.known_file_states.insert(child_rel, current_state);
    }

    // Purge stale files in this directory
    let stale_files: Vec<String> = state
        .files_by_parent
        .get(rel_dir)
        .into_iter()
        .flatten()
        .filter(|p| !seen_files.contains(*p))
        .cloned()
        .collect();
    for path in stale_files {
        delete_indexed_path(conn, &path)?;
        state.known_file_states.remove(&path);
        state.has_deletion = true;
    }

    // Purge stale child directories
    let stale_dirs: Vec<String> = state
        .child_dirs_by_parent
        .get(rel_dir)
        .into_iter()
        .flatten()
        .filter(|d| !discovered_child_dirs.contains(*d))
        .cloned()
        .collect();
    for stale_dir in stale_dirs {
        purge_subtree(conn, &stale_dir, state)?;
    }

    // Record updated state
    conn.execute(
        "INSERT INTO mdoc_dirs (path, mtime_ns)
         VALUES (?, ?)
         ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns",
        rusqlite::params![rel_dir, current_mtime_ns],
    )?;
    state
        .known_dirs
        .insert(rel_dir.to_string(), current_mtime_ns);
    state
        .child_dirs_by_parent
        .insert(rel_dir.to_string(), discovered_child_dirs.clone());
    state
        .files_by_parent
        .insert(rel_dir.to_string(), seen_files);

    // Push children for processing (reverse sorted for deterministic order)
    let mut children: Vec<String> = discovered_child_dirs.into_iter().collect();
    children.sort();
    children.reverse();
    for child in children {
        stack.push(child);
    }
    Ok(())
}

fn purge_subtree(conn: &Connection, rel_dir: &str, state: &mut DiscoveryState) -> Result<()> {
    let prefix = if rel_dir.is_empty() {
        String::new()
    } else {
        format!("{rel_dir}/")
    };

    let stale_files: Vec<String> = state
        .known_file_states
        .keys()
        .filter(|p| {
            let p = p.as_str();
            p == rel_dir || (!prefix.is_empty() && p.starts_with(&prefix))
        })
        .cloned()
        .collect();
    for path in stale_files {
        delete_indexed_path(conn, &path)?;
        state.known_file_states.remove(&path);
        state.has_deletion = true;
    }

    let stale_dirs: Vec<String> = if rel_dir.is_empty() {
        state.known_dirs.keys().cloned().collect()
    } else {
        state
            .known_dirs
            .keys()
            .filter(|d| {
                let d = d.as_str();
                d == rel_dir || d.starts_with(&prefix)
            })
            .cloned()
            .collect()
    };
    for dir in stale_dirs {
        conn.execute("DELETE FROM mdoc_dirs WHERE path = ?", [&dir])?;
        state.known_dirs.remove(&dir);
        state.child_dirs_by_parent.remove(&dir);
        state.files_by_parent.remove(&dir);
    }
    Ok(())
}

// ── Workspace directory walk ──────────────────────────────────────────────────

fn scan_workspace_dirs(root: &Path) -> Vec<(String, i64)> {
    let mut results: Vec<(String, i64)> = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, String)> = vec![(
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
        String::new(),
    )];

    while let Some((dir_path, rel_dir)) = stack.pop() {
        if !rel_dir.is_empty() && dir_path.join(".mdc").is_dir() {
            continue;
        }
        let mtime_ns = std::fs::metadata(&dir_path)
            .ok()
            .as_ref()
            .map(mtime_ns_from_meta)
            .unwrap_or(0);
        results.push((rel_dir.clone(), mtime_ns));

        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .filter(|e| e.file_name() != ".mdc")
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries.reverse(); // push in reverse so sorted order is processed first
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            stack.push((entry.path(), join_rel_dir(&rel_dir, &name)));
        }
    }
    results
}

// ── DB snapshot helpers ───────────────────────────────────────────────────────

fn dir_mtimes(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns FROM mdoc_dirs")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn file_states(conn: &Connection) -> Result<HashMap<String, (i64, i64)>> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let rows: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|(p, ns, sz)| (p, (ns, sz))).collect())
}

fn group_dirs_by_parent(dirs: &HashMap<String, i64>) -> HashMap<String, HashSet<String>> {
    let mut grouped: HashMap<String, HashSet<String>> = HashMap::new();
    grouped.entry(String::new()).or_default();
    for rel_dir in dirs.keys() {
        grouped.entry(rel_dir.clone()).or_default();
        if !rel_dir.is_empty() {
            grouped
                .entry(parent_dir(rel_dir))
                .or_default()
                .insert(rel_dir.clone());
        }
    }
    grouped
}

fn group_files_by_parent(
    file_states: &HashMap<String, (i64, i64)>,
) -> HashMap<String, HashSet<String>> {
    let mut grouped: HashMap<String, HashSet<String>> = HashMap::new();
    grouped.entry(String::new()).or_default();
    for rel_path in file_states.keys() {
        grouped
            .entry(parent_dir(rel_path))
            .or_default()
            .insert(rel_path.clone());
    }
    grouped
}

// ── Path utilities ────────────────────────────────────────────────────────────

fn join_rel_dir(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn parent_dir(rel_path: &str) -> String {
    match rel_path.rfind('/') {
        Some(idx) => rel_path[..idx].to_string(),
        None => String::new(),
    }
}

fn mtime_ns_from_meta(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64 * 1_000_000_000 + d.subsec_nanos() as i64)
        .unwrap_or(0)
}
