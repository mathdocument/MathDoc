use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::indcache::queries::{
    compute_all_topo_depths_from_edges, edge_targets_for_source_path, fnode_for_path,
    path_for_fnode_if_unique, path_has_blocking_issue, CHUNK_SIZE,
};
use crate::mdoc::{read_mdoc_head, MdocNode};
use crate::workspace::{find_nested_mdcroot, iter_mdoc_files, to_rel_path};

// ── Public write functions ────────────────────────────────────────────────────

/// Full workspace scan: upsert changed files, delete stale paths, rebuild dir index.
pub fn refresh_search_index(conn: &Connection, root: &Path) -> Result<()> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let cached_by_path: std::collections::HashMap<String, (i64, i64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(p, ns, sz)| (p, (ns, sz)))
        .collect();

    let mut stmt2 = conn.prepare(
        "SELECT path FROM mdoc_files
         UNION SELECT path FROM mdocs
         UNION SELECT path FROM mdoc_issues
         UNION SELECT src_path AS path FROM mdoc_edges",
    )?;
    let indexed_paths: HashSet<String> = stmt2
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut seen_paths: HashSet<String> = HashSet::new();
    for file_path in iter_mdoc_files(root) {
        let rel_path = to_rel_path(root, &file_path);
        seen_paths.insert(rel_path.clone());
        let meta = match std::fs::metadata(&file_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let (mtime_ns, size) = metadata_state(&meta);
        if cached_by_path.get(&rel_path) == Some(&(mtime_ns, size)) {
            continue;
        }
        upsert_mdoc_row(conn, root, &file_path)?;
    }

    for stale_path in indexed_paths.difference(&seen_paths) {
        delete_indexed_path(conn, stale_path)?;
    }

    super::queries::refresh_all_derived_data(conn)?;
    super::discovery::rebuild_directory_index(conn, root)?;
    conn.execute(
        "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
        [],
    )?;
    Ok(())
}

/// Re-check all already-indexed paths; delete those that have vanished, re-upsert changed ones.
pub fn refresh_indexed_paths(conn: &Connection, root: &Path) -> Result<()> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let rows: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (rel_path, cached_ns, cached_sz) in rows {
        let file_path = root.join(&rel_path);
        match std::fs::metadata(&file_path) {
            Ok(meta) => {
                let (mtime_ns, size) = metadata_state(&meta);
                if (mtime_ns, size) != (cached_ns, cached_sz) {
                    upsert_mdoc_row(conn, root, &file_path)?;
                }
            }
            Err(_) => delete_indexed_path(conn, &rel_path)?,
        }
    }
    super::queries::refresh_all_derived_data(conn)?;
    Ok(())
}

/// Upsert the root path and all reachable dependencies up to `depth` hops (-1 = infinite).
/// Returns the fnodes of all successfully upserted files (for incremental topo updates).
pub fn refresh_reachable_from_path(
    conn: &Connection,
    root: &Path,
    root_path: &Path,
    depth: i32,
) -> Result<Vec<String>> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut upserted_fnodes: Vec<String> = Vec::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, u32)> =
        std::collections::VecDeque::new();
    let canonical_root = root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf());
    queue.push_back((canonical_root, 0));

    while let Some((file_path, item_depth)) = queue.pop_front() {
        let rel_path = to_rel_path(root, &file_path);
        if !seen.insert(rel_path.clone()) {
            continue;
        }
        // Skip files that no longer exist at the cached path — they may have been
        // renamed. Cleaning up stale paths is sync's job, not a targeted refresh.
        if !file_path.exists() {
            continue;
        }
        upsert_mdoc_row(conn, root, &file_path)?;
        if let Some(fnode) = fnode_for_path(conn, &rel_path)? {
            upserted_fnodes.push(fnode);
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
    Ok(upserted_fnodes)
}

/// Upsert a single .mdoc file: update metadata, parse, rebuild edges and issues.
pub fn upsert_mdoc_row(conn: &Connection, root: &Path, file_path: &Path) -> Result<()> {
    // Guard: file must not be inside a nested workspace
    let parent = file_path.parent().unwrap_or(file_path);
    let root_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let parent_resolved = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    if let Some(nested) = find_nested_mdcroot(&root_resolved, &parent_resolved) {
        bail!("mdoc path is inside nested mdoc root: {}", nested.display());
    }

    let file_resolved = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_path_buf());
    let rel_path = to_rel_path(&root_resolved, &file_resolved);
    let old_fnode = fnode_for_path(conn, &rel_path)?;

    let meta = match std::fs::metadata(file_path) {
        Ok(m) if m.is_file() => m,
        _ => {
            delete_indexed_path(conn, &rel_path)?;
            return Ok(());
        }
    };

    let (mtime_ns, size) = metadata_state(&meta);
    let mtime_sec = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO mdoc_files (path, mtime_sec, mtime_ns, size)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             mtime_sec = excluded.mtime_sec,
             mtime_ns = excluded.mtime_ns,
             size = excluded.size",
        rusqlite::params![rel_path, mtime_sec, mtime_ns, size],
    )?;

    // Quick head read (fallback for mdocs row if full parse fails)
    let head = read_mdoc_head(file_path);

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

    // Full parse (headers + deps, no block content)
    let parse_result = MdocNode::load_head(&root_resolved, file_path);
    let new_fnode: Option<String>;
    let mut new_dst_fnodes: HashSet<String> = HashSet::new();

    match parse_result {
        Ok(node) => {
            new_fnode = Some(node.fnode.clone());
            upsert_search_row(
                conn,
                &rel_path,
                &node.fnode,
                &node.title,
                mtime_sec,
                mtime_ns,
                size,
            )?;
            for (order, dep_fnode) in node.depens.iter().enumerate() {
                conn.execute(
                    "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                     VALUES (?, ?, ?, ?)",
                    rusqlite::params![rel_path, node.fnode, dep_fnode, order as i64],
                )?;
                new_dst_fnodes.insert(dep_fnode.clone());
            }
        }
        Err(e) => {
            let ref_fnode = head
                .as_ref()
                .map(|(f, _)| f.as_str())
                .unwrap_or("<unknown>");
            new_fnode = head.as_ref().map(|(f, _)| f.clone());
            match &head {
                Some((fnode, title)) => {
                    upsert_search_row(conn, &rel_path, fnode, title, mtime_sec, mtime_ns, size)?;
                }
                None => {
                    conn.execute("DELETE FROM mdocs WHERE path = ?", [&rel_path])?;
                }
            }
            insert_issue(conn, &rel_path, "invalid", ref_fnode, &e.to_string())?;
        }
    }

    // If the file was renamed, the old path for this fnode is still in the cache.
    // Clean it up now so that refresh_duplicate_issues_for_fnode doesn't flag a
    // spurious duplicate.
    if let Some(ref nf) = new_fnode {
        let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? AND path != ?")?;
        let stale_paths: Vec<String> = stmt
            .query_map(rusqlite::params![nf, rel_path], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        for stale_rel in stale_paths {
            if !root_resolved.join(&stale_rel).exists() {
                delete_indexed_path(conn, &stale_rel)?;
            }
        }
    }

    refresh_duplicate_issues_for_fnode(conn, old_fnode.as_deref())?;
    refresh_duplicate_issues_for_fnode(conn, new_fnode.as_deref())?;
    refresh_missing_issues_for_source(conn, &rel_path)?;
    refresh_missing_issues_for_target(conn, old_fnode.as_deref())?;
    refresh_missing_issues_for_target(conn, new_fnode.as_deref())?;

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

    if old_fnode != new_fnode || old_dst_fnodes != new_dst_fnodes {
        bump_graph_epoch(conn)?;
    }
    Ok(())
}

/// Remove all index entries for a path (file deleted or moved).
pub fn delete_indexed_path(conn: &Connection, stale_path: &str) -> Result<()> {
    let old_fnode = fnode_for_path(conn, stale_path)?;
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

    if old_fnode.is_some() {
        bump_graph_epoch(conn)?;
    }
    Ok(())
}

// ── Topo depth helpers ────────────────────────────────────────────────────────

/// Compute topo_depth for a single fnode: max(dep topo_depths) + 1, or 0 if no deps.
fn compute_node_topo_depth(conn: &Connection, fnode: &str) -> Result<u32> {
    let max_dep: Option<u32> = conn.query_row(
        "SELECT MAX(m.topo_depth)
         FROM mdoc_edges e
         JOIN mdocs m ON m.fnode = e.dst_fnode
         WHERE e.src_fnode = ?
           AND NOT EXISTS (
             SELECT 1 FROM mdoc_issues i
             WHERE i.path = e.src_path AND i.kind IN ('invalid', 'duplicate')
           )",
        [fnode],
        |r| r.get::<_, Option<u32>>(0),
    )?;
    Ok(max_dep.map(|d| d + 1).unwrap_or(0))
}

/// BFS upward from `start_fnode` through reverse edges, recomputing and persisting
/// `topo_depth` for each ancestor whose depth changes.
pub(crate) fn refresh_topo_depth_upward_from(conn: &Connection, start_fnode: &str) -> Result<()> {
    use std::collections::{HashSet, VecDeque};
    let mut queue: VecDeque<String> = VecDeque::from([start_fnode.to_string()]);
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(fnode) = queue.pop_front() {
        if !visited.insert(fnode.clone()) {
            continue;
        }
        let new_depth = compute_node_topo_depth(conn, &fnode)?;
        let old_depth: Option<u32> = conn
            .query_row(
                "SELECT topo_depth FROM mdocs WHERE fnode = ?",
                [&fnode],
                |r| r.get(0),
            )
            .ok();
        if old_depth != Some(new_depth) {
            conn.execute(
                "UPDATE mdocs SET topo_depth = ? WHERE fnode = ?",
                rusqlite::params![new_depth, &fnode],
            )?;
            // Propagate to nodes that have this fnode as a dependency.
            let mut stmt =
                conn.prepare("SELECT DISTINCT src_fnode FROM mdoc_edges WHERE dst_fnode = ?")?;
            let parents: Vec<String> = stmt
                .query_map([&fnode], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for parent in parents {
                if !visited.contains(&parent) {
                    queue.push_back(parent);
                }
            }
        }
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

// ── Incremental weak component helpers ───────────────────────────────────────

/// Insert a node as its own isolated component (no-op if already present).
fn ensure_component_entry(conn: &Connection, fnode: &str) -> Result<()> {
    if fnode.starts_with('<') && fnode.ends_with('>') {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO mdoc_weak_component (fnode, component_id, component_size)
         VALUES (?, ?, 1)",
        rusqlite::params![fnode, fnode],
    )?;
    Ok(())
}

/// Union the components of `u` and `v`. Merges smaller into larger by size.
fn union_components(conn: &Connection, u: &str, v: &str) -> Result<()> {
    if (u.starts_with('<') && u.ends_with('>')) || (v.starts_with('<') && v.ends_with('>')) {
        return Ok(());
    }
    ensure_component_entry(conn, u)?;
    ensure_component_entry(conn, v)?;

    let row_u: Option<(String, u32)> = conn
        .query_row(
            "SELECT component_id, component_size FROM mdoc_weak_component WHERE fnode = ?",
            [u],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let row_v: Option<(String, u32)> = conn
        .query_row(
            "SELECT component_id, component_size FROM mdoc_weak_component WHERE fnode = ?",
            [v],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (cid_u, size_u) = match row_u {
        Some(r) => r,
        None => return Ok(()),
    };
    let (cid_v, size_v) = match row_v {
        Some(r) => r,
        None => return Ok(()),
    };
    if cid_u == cid_v {
        return Ok(());
    }
    let (keep, replace, new_size) = if size_u >= size_v {
        (cid_u, cid_v, size_u + size_v)
    } else {
        (cid_v, cid_u, size_u + size_v)
    };
    conn.execute(
        "UPDATE mdoc_weak_component SET component_id = ?, component_size = ?
         WHERE component_id = ?",
        rusqlite::params![keep, new_size, replace],
    )?;
    Ok(())
}

/// Build undirected adjacency (restricted to `members`) from the current `mdoc_edges` table.
fn build_undirected_adj_for_members(
    conn: &Connection,
    members: &HashSet<String>,
) -> Result<HashMap<String, HashSet<String>>> {
    let mut adj: HashMap<String, HashSet<String>> = members
        .iter()
        .map(|m| (m.clone(), HashSet::new()))
        .collect();

    let member_vec: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
    for chunk in member_vec.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT src_fnode, dst_fnode FROM mdoc_edges
             WHERE src_fnode IN ({placeholders})
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE mdoc_issues.path = mdoc_edges.src_path
                   AND mdoc_issues.kind IN ('invalid', 'duplicate')
               )"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|f| f as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (src, dst) = row?;
            if members.contains(&dst) {
                adj.entry(src.clone()).or_default().insert(dst.clone());
                adj.entry(dst).or_default().insert(src);
            }
        }
    }
    Ok(adj)
}

/// BFS from `start` using undirected `adj`. Returns the set of reachable nodes.
fn bfs_reachable(adj: &HashMap<String, HashSet<String>>, start: &str) -> HashSet<String> {
    use std::collections::VecDeque;
    let mut visited: HashSet<String> = HashSet::new();
    if !adj.contains_key(start) {
        return visited;
    }
    let mut queue: VecDeque<String> = VecDeque::from([start.to_string()]);
    while let Some(node) = queue.pop_front() {
        if !visited.insert(node.clone()) {
            continue;
        }
        for nb in adj.get(&node).into_iter().flatten() {
            if !visited.contains(nb.as_str()) {
                queue.push_back(nb.clone());
            }
        }
    }
    visited
}

/// After edge `u → v` removal, check if `u` and `v` are still connected; split if not.
fn check_and_split_component(conn: &Connection, u: &str, v: &str) -> Result<()> {
    if (u.starts_with('<') && u.ends_with('>')) || (v.starts_with('<') && v.ends_with('>')) {
        return Ok(());
    }
    let cid_u: Option<String> = conn
        .query_row(
            "SELECT component_id FROM mdoc_weak_component WHERE fnode = ?",
            [u],
            |r| r.get(0),
        )
        .ok();
    let cid_v: Option<String> = conn
        .query_row(
            "SELECT component_id FROM mdoc_weak_component WHERE fnode = ?",
            [v],
            |r| r.get(0),
        )
        .ok();
    let (cid_u, cid_v) = match (cid_u, cid_v) {
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(()),
    };
    if cid_u != cid_v {
        return Ok(());
    }

    // Get all members of this component
    let mut stmt = conn.prepare("SELECT fnode FROM mdoc_weak_component WHERE component_id = ?")?;
    let members: HashSet<String> = stmt
        .query_map([&cid_u], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if members.len() <= 1 {
        return Ok(());
    }

    let adj = build_undirected_adj_for_members(conn, &members)?;
    let u_side = bfs_reachable(&adj, u);
    if u_side.contains(v) {
        return Ok(());
    }

    let v_side: HashSet<String> = members.difference(&u_side).cloned().collect();
    let new_cid_u = u_side
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| u.to_string());
    let new_cid_v = v_side
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| v.to_string());
    let sz_u = u_side.len() as u32;
    let sz_v = v_side.len() as u32;

    for chunk in u_side.iter().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
        let ph = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE mdoc_weak_component SET component_id = ?, component_size = ?
             WHERE fnode IN ({ph})"
        );
        let mut params: Vec<&dyn rusqlite::types::ToSql> = vec![
            &new_cid_u as &dyn rusqlite::types::ToSql,
            &sz_u as &dyn rusqlite::types::ToSql,
        ];
        params.extend(chunk.iter().map(|f| *f as &dyn rusqlite::types::ToSql));
        conn.execute(&sql, params.as_slice())?;
    }
    for chunk in v_side.iter().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
        let ph = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE mdoc_weak_component SET component_id = ?, component_size = ?
             WHERE fnode IN ({ph})"
        );
        let mut params: Vec<&dyn rusqlite::types::ToSql> = vec![
            &new_cid_v as &dyn rusqlite::types::ToSql,
            &sz_v as &dyn rusqlite::types::ToSql,
        ];
        params.extend(chunk.iter().map(|f| *f as &dyn rusqlite::types::ToSql));
        conn.execute(&sql, params.as_slice())?;
    }
    Ok(())
}

/// Incrementally update weak components after a single-file upsert.
///
/// Handles:
/// - New node (old=None, new=Some): insert isolated, union with new deps.
/// - Same fnode, edge changes: split-check removed edges, union added edges.
/// - Other cases (fnode rename, deletion): fall back to full recompute.
///
/// Always ends with `weak_component_dirty = 0`.
pub(crate) fn update_weak_component_incremental(
    conn: &Connection,
    old_fnode: Option<&str>,
    new_fnode: Option<&str>,
    old_dsts: &HashSet<String>,
    new_dsts: &HashSet<String>,
) -> Result<()> {
    match (old_fnode, new_fnode) {
        (None, None) => {}

        (None, Some(new_f)) => {
            ensure_component_entry(conn, new_f)?;
            for dep in new_dsts {
                union_components(conn, new_f, dep)?;
            }
        }

        (Some(old_f), Some(new_f)) if old_f == new_f => {
            for dep in old_dsts.difference(new_dsts) {
                check_and_split_component(conn, old_f, dep)?;
            }
            for dep in new_dsts.difference(old_dsts) {
                union_components(conn, new_f, dep)?;
            }
        }

        _ => {
            // Fnode changed or node deleted: fall back to full recompute.
            super::queries::recompute_weak_components_full(conn)?;
            return Ok(());
        }
    }
    conn.execute(
        "UPDATE mdoc_index_state SET weak_component_dirty = 0 WHERE id = 1",
        [],
    )?;
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
                 FROM mdoc_edges
                 WHERE dst_fnode IN ({placeholders})
                   AND NOT EXISTS (
                     SELECT 1 FROM mdoc_issues
                     WHERE mdoc_issues.path = mdoc_edges.src_path
                       AND mdoc_issues.kind IN ('invalid', 'duplicate')
                   )
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

fn metadata_state(meta: &std::fs::Metadata) -> (i64, i64) {
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64 * 1_000_000_000 + d.subsec_nanos() as i64)
        .unwrap_or(0);
    let size = meta.len() as i64;
    (mtime_ns, size)
}

fn upsert_search_row(
    conn: &Connection,
    rel_path: &str,
    fnode: &str,
    title: &str,
    mtime_sec: i64,
    mtime_ns: i64,
    size: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO mdocs (path, fnode, title, title_lc, mtime_sec, mtime_ns, size)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             fnode = excluded.fnode,
             title = excluded.title,
             title_lc = excluded.title_lc,
             mtime_sec = excluded.mtime_sec,
             mtime_ns = excluded.mtime_ns,
             size = excluded.size",
        rusqlite::params![
            rel_path,
            fnode,
            title,
            title.to_lowercase(),
            mtime_sec,
            mtime_ns,
            size
        ],
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

fn refresh_duplicate_issues_for_fnode(conn: &Connection, fnode: Option<&str>) -> Result<()> {
    let fnode = match fnode {
        Some(f) if !f.is_empty() && !(f.starts_with('<') && f.ends_with('>')) => f,
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
    let has_blocking = conn
        .query_row(
            "SELECT 1 FROM mdoc_issues
             WHERE path = ? AND kind IN ('invalid', 'duplicate') LIMIT 1",
            [src_path],
            |_| Ok(()),
        )
        .is_ok();
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
        Some(f) if !f.is_empty() && !(f.starts_with('<') && f.ends_with('>')) => f,
        _ => return Ok(()),
    };
    conn.execute(
        "DELETE FROM mdoc_issues WHERE kind = 'missing' AND ref_fnode = ?",
        [target],
    )?;
    // If exactly 1 node claims this fnode, it's not missing
    let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? ORDER BY path LIMIT 2")?;
    let node_paths: Vec<String> = stmt
        .query_map([target], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if node_paths.len() == 1 {
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
        let src_has_blocking = conn
            .query_row(
                "SELECT 1 FROM mdoc_issues
                 WHERE path = ? AND kind IN ('invalid', 'duplicate') LIMIT 1",
                [&src_path],
                |_| Ok(()),
            )
            .is_ok();
        if src_has_blocking {
            continue;
        }
        insert_issue(conn, &src_path, "missing", target, &error)?;
    }
    Ok(())
}
