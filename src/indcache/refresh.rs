use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::indcache::queries::{
    compute_all_topo_depths_from_edges, edge_targets_for_source_path, fnode_for_path,
    path_for_fnode_if_unique, path_has_blocking_issue, CHUNK_SIZE,
};
use crate::mdocnode::{MdocHead, MdocIdentity};
use crate::workspace::{find_nested_mdcroot, iter_mdoc_files, to_rel_path};

// ── Public write functions ────────────────────────────────────────────────────

/// Full workspace scan: reparse every file and delete stale paths.
pub fn refresh_search_index(conn: &Connection, root: &Path) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT path FROM mdoc_files
         UNION SELECT path FROM mdocs
         UNION SELECT path FROM mdoc_issues
         UNION SELECT src_path AS path FROM mdoc_edges",
    )?;
    let indexed_paths: HashSet<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut seen_paths: HashSet<String> = HashSet::new();
    for file_path in iter_mdoc_files(root) {
        let file_path = file_path?;
        let rel_path = to_rel_path(root, &file_path);
        seen_paths.insert(rel_path.clone());
        // A force refresh intentionally parses every discovered file. Metadata
        // equality is not a content guarantee.
        upsert_mdoc_row(conn, root, &file_path)?;
    }

    for stale_path in indexed_paths.difference(&seen_paths) {
        delete_indexed_path(conn, stale_path)?;
    }

    super::queries::refresh_all_derived_data(conn)?;
    conn.execute(
        "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
        [],
    )?;
    Ok(())
}

/// Upsert the root path and all reachable dependencies up to `depth` hops (-1 = infinite).
/// Returns the fnodes of all successfully upserted files (for incremental topo updates).
pub fn refresh_reachable_from_path(
    conn: &Connection,
    root: &Path,
    root_path: &Path,
    depth: i32,
) -> Result<HashSet<String>> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut affected_fnodes: HashSet<String> = HashSet::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, u32)> =
        std::collections::VecDeque::new();
    let canonical_root = crate::workspace::resolve_mdoc_path(root, root_path)?;
    queue.push_back((canonical_root, 0));

    while let Some((file_path, item_depth)) = queue.pop_front() {
        let file_path = crate::workspace::resolve_mdoc_path(root, &file_path)?;
        let rel_path = to_rel_path(root, &file_path);
        if !seen.insert(rel_path.clone()) {
            continue;
        }
        if let Some(old_fnode) = fnode_for_path(conn, &rel_path)? {
            affected_fnodes.insert(old_fnode);
        }
        upsert_mdoc_row(conn, root, &file_path)?;
        if let Some(fnode) = fnode_for_path(conn, &rel_path)? {
            affected_fnodes.insert(fnode);
        }
        if !file_path.exists() {
            continue;
        }
        if depth != -1 && item_depth as i32 >= depth {
            continue;
        }
        if path_has_blocking_issue(conn, &rel_path)? {
            continue;
        }
        for dep_fnode in edge_targets_for_source_path(conn, &rel_path)? {
            if let Some(dep_rel) = path_for_fnode_if_unique(conn, &dep_fnode)? {
                queue.push_back((root.join(&dep_rel), item_depth + 1));
            }
        }
    }
    Ok(affected_fnodes)
}

pub(crate) fn validate_cached_mdoc_path(root: &Path, rel_path: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel_path);
    if rel_path.is_absolute() {
        bail!("cached mdoc path must be relative: {}", rel_path.display());
    }
    let resolved = crate::workspace::resolve_mdoc_path(root, rel_path)?;
    let meta = std::fs::symlink_metadata(&resolved)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        bail!(
            "cached mdoc path is not a regular file: {}",
            resolved.display()
        );
    }
    Ok(resolved)
}

/// Upsert a single .mdoc file: update metadata, parse, rebuild edges and issues.
pub fn upsert_mdoc_row(conn: &Connection, root: &Path, file_path: &Path) -> Result<()> {
    let file_path = crate::workspace::resolve_mdoc_path(root, file_path)?;
    // Guard: file must not be inside a nested workspace
    let parent = file_path.parent().unwrap_or(&file_path);
    let root_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let parent_resolved = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    if let Some(nested) = find_nested_mdcroot(&root_resolved, &parent_resolved) {
        bail!("mdoc path is inside nested mdoc root: {}", nested.display());
    }

    let rel_path = to_rel_path(&root_resolved, &file_path);
    let old_fnode = fnode_for_path(conn, &rel_path)?;
    let old_had_blocking_issue = path_has_blocking_issue(conn, &rel_path)?;

    let meta = match std::fs::metadata(&file_path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => bail!("mdoc path is not a regular file: {}", file_path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            delete_indexed_path(conn, &rel_path)?;
            return Ok(());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading metadata for {}", file_path.display()))
        }
    };
    let snapshot = crate::workspace::FileSnapshot::capture(&file_path)?;
    let Some(content) = snapshot.content() else {
        delete_indexed_path(conn, &rel_path)?;
        return Ok(());
    };

    let (mtime_ns, size) = metadata_state(&meta)?;

    conn.execute(
        "INSERT INTO mdoc_files (path, mtime_ns, size)
         VALUES (?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             mtime_ns = excluded.mtime_ns,
             size = excluded.size",
        rusqlite::params![rel_path, mtime_ns, size],
    )?;

    // Snapshot old edge targets before clearing
    let old_dst_fnodes: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_path = ?")?;
        let rows: HashSet<String> = stmt
            .query_map([&rel_path], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", [&rel_path])?;
    conn.execute("DELETE FROM mdoc_issues WHERE path = ?", [&rel_path])?;

    // Strict structural parse and tolerant identity fallback both use this one
    // captured byte generation.
    let parse_result = MdocHead::load_bytes(&file_path, content);
    let new_fnode: Option<String>;
    let mut new_dst_fnodes: HashSet<String> = HashSet::new();

    match parse_result {
        Ok(head) => {
            new_fnode = Some(head.fnode.clone());

            // Before inserting, remove any stale rows that share this fnode
            // at a different path (file was renamed/moved). This must happen
            // BEFORE upsert_search_row to avoid UNIQUE constraint violations
            // if the DB schema enforces fnode uniqueness.
            cleanup_stale_fnode_paths(conn, &root_resolved, &head.fnode, &rel_path)?;

            upsert_search_row(conn, &rel_path, &head.fnode, &head.title)?;
            for (order, dep_fnode) in head.depens.iter().enumerate() {
                conn.execute(
                    "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                     VALUES (?, ?, ?, ?)",
                    rusqlite::params![rel_path, head.fnode, dep_fnode, order as i64],
                )?;
                new_dst_fnodes.insert(dep_fnode.clone());
            }
        }
        Err(e) => {
            let identity = MdocIdentity::from_bytes(content);
            let ref_fnode = identity.fnode.as_deref().unwrap_or("<unknown>");
            new_fnode = identity.fnode.clone();
            match identity.complete() {
                Some((fnode, title)) => {
                    cleanup_stale_fnode_paths(conn, &root_resolved, fnode, &rel_path)?;
                    upsert_search_row(conn, &rel_path, fnode, title)?;
                }
                None => {
                    conn.execute("DELETE FROM mdocs WHERE path = ?", [&rel_path])?;
                }
            }
            insert_issue(conn, &rel_path, "invalid", ref_fnode, &e.to_string())?;
        }
    }

    refresh_duplicate_issues_for_fnode(conn, old_fnode.as_deref())?;
    refresh_duplicate_issues_for_fnode(conn, new_fnode.as_deref())?;
    refresh_missing_issues_for_source(conn, &rel_path)?;
    refresh_missing_issues_for_target(conn, old_fnode.as_deref())?;
    refresh_missing_issues_for_target(conn, new_fnode.as_deref())?;
    let new_has_blocking_issue = path_has_blocking_issue(conn, &rel_path)?;

    // Collect all fnodes whose in_degree may have changed
    let mut affected: HashSet<String> = old_dst_fnodes.union(&new_dst_fnodes).cloned().collect();
    for fnode in [old_fnode.as_deref(), new_fnode.as_deref()]
        .into_iter()
        .flatten()
    {
        let mut stmt = conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_fnode = ?")?;
        let targets: HashSet<String> = stmt
            .query_map([fnode], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        affected.extend(targets);
    }
    refresh_in_degree_for_fnodes(conn, &affected)?;

    // Blocking issues filter edges and nodes without changing their stored
    // identities. In particular, a duplicate claimant becoming a malformed
    // partial identity can make another claimant valid while this path remains
    // blocking and reports the same fallback fnode. Conservatively invalidate
    // graph-derived caches whenever a touched path is or was blocking.
    if old_fnode != new_fnode
        || old_dst_fnodes != new_dst_fnodes
        || old_had_blocking_issue
        || new_has_blocking_issue
    {
        bump_graph_epoch(conn)?;
    }
    Ok(())
}

/// Remove all index entries for a path (file deleted or moved).
pub fn delete_indexed_path(conn: &Connection, stale_path: &str) -> Result<()> {
    let old_fnode = fnode_for_path(conn, stale_path)?;
    let old_had_blocking_issue = path_has_blocking_issue(conn, stale_path)?;
    let old_dst_fnodes: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_path = ?")?;
        let rows: HashSet<String> = stmt
            .query_map([stale_path], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    conn.execute("DELETE FROM mdoc_files WHERE path = ?", [stale_path])?;
    conn.execute("DELETE FROM mdocs WHERE path = ?", [stale_path])?;
    conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", [stale_path])?;
    conn.execute("DELETE FROM mdoc_issues WHERE path = ?", [stale_path])?;

    refresh_duplicate_issues_for_fnode(conn, old_fnode.as_deref())?;
    refresh_missing_issues_for_target(conn, old_fnode.as_deref())?;

    let mut affected = old_dst_fnodes;
    if let Some(ref fnode) = old_fnode {
        let mut stmt = conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_fnode = ?")?;
        let targets: HashSet<String> = stmt
            .query_map([fnode.as_str()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        affected.extend(targets);
    }
    refresh_in_degree_for_fnodes(conn, &affected)?;

    if old_fnode.is_some() || !affected.is_empty() || old_had_blocking_issue {
        bump_graph_epoch(conn)?;
    }
    Ok(())
}

// ── Topo depth helpers ────────────────────────────────────────────────────────

/// Compute topo_depth for a single fnode: max(dep topo_depths) + 1, or 0 if no deps.
fn compute_node_topo_depth(conn: &Connection, fnode: &str) -> Result<u32> {
    let max_dep: Option<u32> = conn.query_row(
        "SELECT MAX(m.topo_depth)
         FROM mdoc_valid_edges e
         LEFT JOIN mdocs m ON m.fnode = e.dst_fnode
         WHERE e.src_fnode = ?",
        [fnode],
        |r| r.get::<_, Option<u32>>(0),
    )?;
    let has_deps: bool = conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM mdoc_valid_edges e
             WHERE e.src_fnode = ?
         )",
        [fnode],
        |row| row.get(0),
    )?;
    Ok(if has_deps {
        max_dep.unwrap_or(0) + 1
    } else {
        0
    })
}

/// Recompute `start_fnode` and its reverse-reachable ancestors in dependency-first order.
pub(crate) fn refresh_topo_depth_upward_from(conn: &Connection, start_fnode: &str) -> Result<()> {
    use std::collections::VecDeque;

    let mut affected: HashSet<String> = HashSet::from([start_fnode.to_string()]);
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::from([start_fnode.to_string()]);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT e.src_fnode
         FROM mdoc_valid_edges e
         WHERE e.dst_fnode = ?",
    )?;
    while let Some(fnode) = queue.pop_front() {
        let parents: Vec<String> = stmt
            .query_map([&fnode], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for parent in &parents {
            if affected.insert(parent.clone()) {
                queue.push_back(parent.clone());
            }
        }
        reverse.insert(fnode, parents);
    }
    drop(stmt);

    // An ancestor is ready only after all of its affected dependencies have been
    // refreshed. This allows nodes reached by both short and long paths to settle once.
    let mut remaining: HashMap<String, usize> = affected.iter().map(|f| (f.clone(), 0)).collect();
    for parents in reverse.values() {
        for parent in parents {
            *remaining.entry(parent.clone()).or_default() += 1;
        }
    }
    let mut ready: VecDeque<String> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(fnode, _)| fnode.clone())
        .collect();
    let mut processed = 0;
    while let Some(fnode) = ready.pop_front() {
        let new_depth = compute_node_topo_depth(conn, &fnode)?;
        conn.execute(
            "UPDATE mdocs SET topo_depth = ? WHERE fnode = ?",
            rusqlite::params![new_depth, &fnode],
        )?;
        processed += 1;
        for parent in reverse.get(&fnode).into_iter().flatten() {
            if let Some(count) = remaining.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    ready.push_back(parent.clone());
                }
            }
        }
    }

    // Cycles have no dependency-first ordering and repeated local relaxation would
    // increase forever. Match the established full-graph cycle behavior instead.
    if processed != affected.len() {
        backfill_all_topo_depths(conn)?;
    }
    Ok(())
}

/// Recompute topo_depth for all nodes from scratch and persist to DB.
/// Used after bulk scans where incremental updates would be incorrect or too expensive.
pub(crate) fn backfill_all_topo_depths(conn: &Connection) -> Result<()> {
    let depths = compute_all_topo_depths_from_edges(conn)?;
    for chunk in depths.iter().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
        for (fnode, depth) in chunk {
            conn.execute(
                "UPDATE mdocs SET topo_depth = ? WHERE fnode = ?",
                rusqlite::params![depth, fnode],
            )?;
        }
    }
    Ok(())
}

// ── Semi-public helpers used by discovery ────────────────────────────────────

pub(crate) fn refresh_in_degree_for_fnodes(
    conn: &Connection,
    fnodes: &HashSet<String>,
) -> Result<()> {
    if fnodes.is_empty() {
        return Ok(());
    }
    let fnode_vec: Vec<&str> = fnodes.iter().map(|s| s.as_str()).collect();
    for chunk in fnode_vec.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!("DELETE FROM mdoc_in_degree WHERE fnode IN ({placeholders})"),
            rusqlite::params_from_iter(chunk.iter().copied()),
        )?;
        conn.execute(
            &format!(
                "INSERT INTO mdoc_in_degree (fnode, in_degree)
                  SELECT dst_fnode, COUNT(*)
                  FROM mdoc_valid_edges
                  WHERE dst_fnode IN ({placeholders})
                  GROUP BY dst_fnode
                 HAVING COUNT(*) > 0"
            ),
            rusqlite::params_from_iter(chunk.iter().copied()),
        )?;
    }
    Ok(())
}

pub(crate) fn bump_graph_epoch(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE mdoc_index_state
         SET graph_epoch = graph_epoch + 1, weak_component_dirty = 1
         WHERE id = 1",
        [],
    )?;
    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

pub(crate) fn metadata_state(meta: &std::fs::Metadata) -> Result<(i64, i64)> {
    let modified = meta
        .modified()
        .context("reading file modification time")?
        .duration_since(UNIX_EPOCH)
        .context("file modification time predates the Unix epoch")?;
    let mtime_ns = modified.as_secs() as i64 * 1_000_000_000 + modified.subsec_nanos() as i64;
    let size = meta.len() as i64;
    Ok((mtime_ns, size))
}

fn upsert_search_row(conn: &Connection, rel_path: &str, fnode: &str, title: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO mdocs (path, fnode, title, title_lc)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             fnode = excluded.fnode,
             title = excluded.title,
             title_lc = excluded.title_lc",
        rusqlite::params![rel_path, fnode, title, title.to_lowercase()],
    )?;
    Ok(())
}

fn insert_issue(
    conn: &Connection,
    path: &str,
    kind: &str,
    ref_fnode: &str,
    error: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(path, kind, ref_fnode) DO UPDATE SET error = excluded.error",
        rusqlite::params![path, kind, ref_fnode, error],
    )?;
    Ok(())
}

/// Remove any stale index entries that share `fnode` but have a different path.
/// Used when a file was renamed/moved: the new path gets the fnode, and the old
/// path (which no longer exists on disk) must be cleaned up BEFORE the new row
/// is inserted to avoid UNIQUE constraint violations.
fn cleanup_stale_fnode_paths(
    conn: &Connection,
    root: &Path,
    fnode: &str,
    keep_path: &str,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? AND path != ?")?;
    let stale_paths: Vec<String> = stmt
        .query_map(rusqlite::params![fnode, keep_path], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<_>>()?;
    for stale_rel in stale_paths {
        if validate_cached_mdoc_path(root, &stale_rel).is_err() {
            delete_indexed_path(conn, &stale_rel)?;
        }
    }
    Ok(())
}

fn refresh_duplicate_issues_for_fnode(conn: &Connection, fnode: Option<&str>) -> Result<()> {
    let fnode = match fnode {
        Some(f) if !(f.is_empty() || f.starts_with('<') && f.ends_with('>')) => f,
        _ => return Ok(()),
    };
    conn.execute(
        "DELETE FROM mdoc_issues WHERE kind = 'duplicate' AND ref_fnode = ?",
        [fnode],
    )?;
    let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? ORDER BY path")?;
    let paths: Vec<String> = stmt
        .query_map([fnode], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if paths.len() < 2 {
        return Ok(());
    }
    let error = format!("duplicate fnode '{}' across: {}", fnode, paths.join(", "));
    for path in &paths {
        insert_issue(conn, path, "duplicate", fnode, &error)?;
    }
    Ok(())
}

fn refresh_missing_issues_for_source(conn: &Connection, src_path: &str) -> Result<()> {
    let has_blocking: bool = conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE path = ? AND kind IN ('invalid', 'duplicate')
         )",
        [src_path],
        |row| row.get(0),
    )?;
    if has_blocking {
        return Ok(());
    }
    let mut stmt =
        conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_path = ? ORDER BY ord")?;
    let dep_fnodes: Vec<String> = stmt
        .query_map([src_path], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    for dep_fnode in dep_fnodes {
        refresh_missing_issues_for_target(conn, Some(&dep_fnode))?;
    }
    Ok(())
}

fn refresh_missing_issues_for_target(conn: &Connection, target_fnode: Option<&str>) -> Result<()> {
    let target = match target_fnode {
        Some(f) if !(f.is_empty() || f.starts_with('<') && f.ends_with('>')) => f,
        _ => return Ok(()),
    };
    conn.execute(
        "DELETE FROM mdoc_issues WHERE kind = 'missing' AND ref_fnode = ?",
        [target],
    )?;
    // Any claimant means the target is present. Multiple claimants are
    // reported separately as a duplicate/ambiguous target.
    let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? ORDER BY path LIMIT 2")?;
    let node_paths: Vec<String> = stmt
        .query_map([target], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !node_paths.is_empty() {
        return Ok(());
    }
    let error = format!("missing dependency target: {target}");
    let mut stmt2 = conn.prepare(
        "SELECT DISTINCT src_path FROM mdoc_edges WHERE dst_fnode = ? ORDER BY src_path",
    )?;
    let src_paths: Vec<String> = stmt2
        .query_map([target], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    for src_path in src_paths {
        let src_has_blocking: bool = conn.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE path = ? AND kind IN ('invalid', 'duplicate')
             )",
            [&src_path],
            |row| row.get(0),
        )?;
        if src_has_blocking {
            continue;
        }
        insert_issue(conn, &src_path, "missing", target, &error)?;
    }
    Ok(())
}
