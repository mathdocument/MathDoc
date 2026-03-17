//! Write-path operations on the SQLite index.
//! All functions take `&Connection`; the caller provides transaction scope.

use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::indcache::queries::{
    edge_targets_for_source_path, fnode_for_path, path_for_fnode_if_unique,
    path_has_blocking_issue, CHUNK_SIZE,
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
    Ok(())
}

/// Upsert the root path and all reachable dependencies up to `depth` hops (-1 = infinite).
pub fn refresh_reachable_from_path(
    conn: &Connection,
    root: &Path,
    root_path: &Path,
    depth: i32,
) -> Result<()> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    let mut seen: HashSet<String> = HashSet::new();
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
        upsert_mdoc_row(conn, root, &file_path)?;
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
    Ok(())
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
