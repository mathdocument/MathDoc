use anyhow::{bail, Context, Result};
use rusqlite::{Connection, ToSql};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::indcache::queries::{
    edge_targets_for_source_path, fnode_for_path, path_for_fnode_if_unique,
    path_has_blocking_issue, CHUNK_SIZE,
};
use crate::mdocnode::{MdocHead, MdocIdentity};
use crate::workspace::{iter_mdoc_files, to_rel_path, FileSnapshotBatch};

// ── Public write functions ────────────────────────────────────────────────────

/// Full workspace scan: reparse every file and delete stale paths.
pub fn refresh_search_index(conn: &Connection, root: &Path) -> Result<()> {
    let _profile = crate::profile::scope("refresh::refresh_search_index");
    let files = {
        let _phase = crate::profile::scope("refresh::scan_workspace");
        scan_workspace(root)?
    };
    {
        let _phase = crate::profile::scope("refresh::sync_file_states");
        sync_file_states(conn, &files)?;
    }

    let digest = {
        let _phase = crate::profile::scope("refresh::index_digest");
        index_digest(&files)
    };
    let (old_digest, bootstrapped): (String, bool) = conn.query_row(
        "SELECT index_digest, bootstrapped FROM mdoc_index_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if !bootstrapped || old_digest != digest {
        let issues = {
            let _phase = crate::profile::scope("refresh::build_issues");
            build_issues(&files)
        };
        {
            let _phase = crate::profile::scope("refresh::replace_index_rows");
            replace_index_rows(conn, &files, &issues)?;
        }
        {
            let _phase = crate::profile::scope("refresh::rebuild_in_degree");
            rebuild_in_degree(conn)?;
        }
        super::derived::bump_graph_epoch(conn)?;
        super::derived::refresh_all_derived_data(conn)?;
    }
    conn.execute(
        "UPDATE mdoc_index_state SET bootstrapped = 1, index_digest = ? WHERE id = 1",
        [&digest],
    )?;
    Ok(())
}

const BULK_ROWS: usize = 200;
const SCAN_BATCH: usize = 2048;

struct ScannedMdoc {
    path: String,
    mtime_ns: i64,
    size: i64,
    node: Option<ScannedNode>,
    invalid: Option<IndexIssue>,
}

struct ScannedNode {
    fnode: String,
    title: String,
    title_lc: String,
    dependencies: Vec<String>,
    structurally_valid: bool,
}

struct IndexIssue {
    path: String,
    kind: &'static str,
    ref_fnode: String,
    error: String,
}

pub(super) struct UpsertOutcome {
    pub(super) old_fnode: Option<String>,
    pub(super) new_fnode: Option<String>,
    pub(super) graph_changed: bool,
}

fn scan_workspace(root: &Path) -> Result<Vec<ScannedMdoc>> {
    let mut paths = iter_mdoc_files(root).collect::<Result<Vec<_>>>()?;
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for paths in paths.chunks(SCAN_BATCH) {
        files.extend(scan_workspace_batch(root, paths)?);
    }
    Ok(files)
}

fn scan_workspace_batch(root: &Path, paths: &[PathBuf]) -> Result<Vec<ScannedMdoc>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(8)
        .min(paths.len());
    let chunk_size = paths.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for paths in paths.chunks(chunk_size) {
            workers.push(scope.spawn(move || -> Result<Vec<ScannedMdoc>> {
                let mut snapshots = FileSnapshotBatch::new(root)?;
                let mut files = Vec::with_capacity(paths.len());
                for path in paths {
                    let snapshot = snapshots.capture_read(path)?.ok_or_else(|| {
                        anyhow::anyhow!("mdoc file disappeared: {}", path.display())
                    })?;
                    files.push(scan_mdoc(
                        root,
                        path,
                        snapshot.content(),
                        snapshot.metadata(),
                    )?);
                }
                snapshots.finish()?;
                Ok(files)
            }));
        }

        let mut files = Vec::with_capacity(paths.len());
        let mut first_error = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok(worker_files)) => files.extend(worker_files),
                Ok(Err(error)) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
                Err(_) => {
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!("mdoc scan worker panicked"));
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(files),
        }
    })
}

fn scan_mdoc(root: &Path, path: &Path, content: &[u8], metadata: &Metadata) -> Result<ScannedMdoc> {
    let path_string = to_rel_path(root, path);
    let (mtime_ns, size) = metadata_state(metadata)?;

    match MdocHead::load_bytes(path, content) {
        Ok(head) => Ok(ScannedMdoc {
            path: path_string,
            mtime_ns,
            size,
            node: Some(ScannedNode {
                fnode: head.fnode,
                title_lc: head.title.to_lowercase(),
                title: head.title,
                dependencies: head.depens,
                structurally_valid: true,
            }),
            invalid: None,
        }),
        Err(error) => {
            let identity = MdocIdentity::from_bytes(content);
            let ref_fnode = identity.fnode.clone().unwrap_or_else(|| "<unknown>".into());
            let node = identity.complete().map(|(fnode, title)| ScannedNode {
                fnode: fnode.to_string(),
                title_lc: title.to_lowercase(),
                title: title.to_string(),
                dependencies: Vec::new(),
                structurally_valid: false,
            });
            Ok(ScannedMdoc {
                invalid: Some(IndexIssue {
                    path: path_string.clone(),
                    kind: "invalid",
                    ref_fnode,
                    error: error.to_string(),
                }),
                path: path_string,
                mtime_ns,
                size,
                node,
            })
        }
    }
}

fn build_issues(files: &[ScannedMdoc]) -> Vec<IndexIssue> {
    let mut issues = Vec::new();
    let mut claimants: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut blocking_paths = HashSet::new();

    for file in files {
        if let Some(invalid) = &file.invalid {
            issues.push(IndexIssue {
                path: invalid.path.clone(),
                kind: invalid.kind,
                ref_fnode: invalid.ref_fnode.clone(),
                error: invalid.error.clone(),
            });
            blocking_paths.insert(file.path.as_str());
        }
        if let Some(node) = &file.node {
            if is_reportable_fnode(&node.fnode) {
                claimants
                    .entry(node.fnode.as_str())
                    .or_default()
                    .push(file.path.as_str());
            }
        }
    }

    for (fnode, paths) in &claimants {
        if paths.len() < 2 {
            continue;
        }
        let error = format!("duplicate fnode '{}' across: {}", fnode, paths.join(", "));
        for path in paths {
            blocking_paths.insert(*path);
            issues.push(IndexIssue {
                path: (*path).to_string(),
                kind: "duplicate",
                ref_fnode: fnode.to_string(),
                error: error.clone(),
            });
        }
    }

    for file in files {
        let Some(node) = &file.node else {
            continue;
        };
        if !node.structurally_valid || blocking_paths.contains(file.path.as_str()) {
            continue;
        }
        for dependency in &node.dependencies {
            if !claimants.contains_key(dependency.as_str()) {
                issues.push(IndexIssue {
                    path: file.path.clone(),
                    kind: "missing",
                    ref_fnode: dependency.clone(),
                    error: format!("missing dependency target: {dependency}"),
                });
            }
        }
    }
    issues.sort_by(|left, right| {
        (&left.path, left.kind, &left.ref_fnode).cmp(&(&right.path, right.kind, &right.ref_fnode))
    });
    issues
}

fn index_digest(files: &[ScannedMdoc]) -> String {
    let mut digest = Sha256::new();
    hash_value(&mut digest, b"mathdoc-index-v1");
    for file in files {
        hash_value(&mut digest, file.path.as_bytes());
        match &file.node {
            Some(node) => {
                hash_value(&mut digest, b"node");
                hash_value(&mut digest, node.fnode.as_bytes());
                hash_value(&mut digest, node.title.as_bytes());
                hash_value(&mut digest, &[u8::from(node.structurally_valid)]);
                for dependency in &node.dependencies {
                    hash_value(&mut digest, dependency.as_bytes());
                }
            }
            None => hash_value(&mut digest, b"no-node"),
        }
        if let Some(invalid) = &file.invalid {
            hash_value(&mut digest, invalid.ref_fnode.as_bytes());
            hash_value(&mut digest, invalid.error.as_bytes());
        }
    }
    format!("{:x}", digest.finalize())
}

fn hash_value(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

fn sync_file_states(conn: &Connection, files: &[ScannedMdoc]) -> Result<()> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let current: HashMap<String, (i64, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    let desired_paths: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let stale: Vec<&str> = current
        .keys()
        .map(String::as_str)
        .filter(|path| !desired_paths.contains(path))
        .collect();
    for chunk in stale.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!("DELETE FROM mdoc_files WHERE path IN ({placeholders})"),
            rusqlite::params_from_iter(chunk.iter().copied()),
        )?;
    }

    let changed: Vec<&ScannedMdoc> = files
        .iter()
        .filter(|file| current.get(&file.path) != Some(&(file.mtime_ns, file.size)))
        .collect();
    for chunk in changed.chunks(BULK_ROWS) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 3);
        for file in chunk {
            params.push(&file.path);
            params.push(&file.mtime_ns);
            params.push(&file.size);
        }
        conn.execute(
            &format!(
                "INSERT INTO mdoc_files (path, mtime_ns, size) VALUES {placeholders}
                 ON CONFLICT(path) DO UPDATE SET
                   mtime_ns = excluded.mtime_ns,
                   size = excluded.size"
            ),
            params.as_slice(),
        )?;
    }
    Ok(())
}

fn replace_index_rows(
    conn: &Connection,
    files: &[ScannedMdoc],
    issues: &[IndexIssue],
) -> Result<()> {
    conn.execute_batch("DELETE FROM mdoc_edges; DELETE FROM mdoc_issues; DELETE FROM mdocs;")?;

    let nodes: Vec<(&ScannedMdoc, &ScannedNode)> = files
        .iter()
        .filter_map(|file| file.node.as_ref().map(|node| (file, node)))
        .collect();
    for chunk in nodes.chunks(BULK_ROWS) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 4);
        for (file, node) in chunk {
            params.push(&file.path);
            params.push(&node.fnode);
            params.push(&node.title);
            params.push(&node.title_lc);
        }
        conn.execute(
            &format!("INSERT INTO mdocs (path, fnode, title, title_lc) VALUES {placeholders}"),
            params.as_slice(),
        )?;
    }

    let edges: Vec<(&str, &str, &str, i64)> = files
        .iter()
        .filter_map(|file| {
            file.node
                .as_ref()
                .filter(|node| node.structurally_valid)
                .map(|node| (file, node))
        })
        .flat_map(|(file, node)| {
            node.dependencies
                .iter()
                .enumerate()
                .map(move |(order, dep)| {
                    (
                        file.path.as_str(),
                        node.fnode.as_str(),
                        dep.as_str(),
                        order as i64,
                    )
                })
        })
        .collect();
    for chunk in edges.chunks(BULK_ROWS) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 4);
        for (path, source, target, order) in chunk {
            params.push(path);
            params.push(source);
            params.push(target);
            params.push(order);
        }
        conn.execute(
            &format!(
                "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                 VALUES {placeholders}"
            ),
            params.as_slice(),
        )?;
    }

    for chunk in issues.chunks(BULK_ROWS) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 4);
        for issue in chunk {
            params.push(&issue.path);
            params.push(&issue.kind);
            params.push(&issue.ref_fnode);
            params.push(&issue.error);
        }
        conn.execute(
            &format!(
                "INSERT INTO mdoc_issues (path, kind, ref_fnode, error) VALUES {placeholders}"
            ),
            params.as_slice(),
        )?;
    }
    Ok(())
}

fn rebuild_in_degree(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM mdoc_in_degree;
         INSERT INTO mdoc_in_degree (fnode, in_degree)
         SELECT dst_fnode, COUNT(*)
         FROM mdoc_valid_edges
         GROUP BY dst_fnode
         HAVING COUNT(*) > 0;",
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
        let outcome = upsert_mdoc_row(conn, root, &file_path)?;
        if outcome.graph_changed {
            affected_fnodes.extend(outcome.old_fnode);
            affected_fnodes.extend(outcome.new_fnode);
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
pub fn upsert_mdoc_row(conn: &Connection, root: &Path, file_path: &Path) -> Result<UpsertOutcome> {
    let root_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let file_path = crate::workspace::resolve_mdoc_path(&root_resolved, file_path)?;
    let rel_path = to_rel_path(&root_resolved, &file_path);
    let old_fnode = fnode_for_path(conn, &rel_path)?;
    let old_had_blocking_issue = path_has_blocking_issue(conn, &rel_path)?;

    let meta = match std::fs::metadata(&file_path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => bail!("mdoc path is not a regular file: {}", file_path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            delete_indexed_path(conn, &rel_path)?;
            return Ok(UpsertOutcome {
                graph_changed: old_fnode.is_some() || old_had_blocking_issue,
                old_fnode,
                new_fnode: None,
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading metadata for {}", file_path.display()))
        }
    };
    let snapshot = crate::workspace::FileSnapshot::capture(&file_path)?;
    let Some(content) = snapshot.content() else {
        delete_indexed_path(conn, &rel_path)?;
        return Ok(UpsertOutcome {
            graph_changed: old_fnode.is_some() || old_had_blocking_issue,
            old_fnode,
            new_fnode: None,
        });
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

    let identity_fnodes: HashSet<&str> = [old_fnode.as_deref(), new_fnode.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    for fnode in &identity_fnodes {
        refresh_duplicate_issues_for_fnode(conn, Some(fnode))?;
    }
    refresh_missing_issues_for_source(conn, &rel_path)?;
    for fnode in &identity_fnodes {
        refresh_missing_issues_for_target(conn, Some(fnode))?;
    }
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
    super::derived::refresh_in_degree_for_fnodes(conn, &affected)?;

    // Blocking issues filter edges and nodes without changing their stored
    // identities. In particular, a duplicate claimant becoming a malformed
    // partial identity can make another claimant valid while this path remains
    // blocking and reports the same fallback fnode. Conservatively invalidate
    // graph-derived caches whenever a touched path is or was blocking.
    let graph_changed = old_fnode != new_fnode
        || old_dst_fnodes != new_dst_fnodes
        || old_had_blocking_issue
        || new_has_blocking_issue;
    if graph_changed {
        super::derived::bump_graph_epoch(conn)?;
    }
    invalidate_index_digest(conn)?;
    Ok(UpsertOutcome {
        old_fnode,
        new_fnode,
        graph_changed,
    })
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
    super::derived::refresh_in_degree_for_fnodes(conn, &affected)?;

    if old_fnode.is_some() || !affected.is_empty() || old_had_blocking_issue {
        super::derived::bump_graph_epoch(conn)?;
    }
    invalidate_index_digest(conn)?;
    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn invalidate_index_digest(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE mdoc_index_state SET index_digest = '' WHERE id = 1",
        [],
    )?;
    Ok(())
}

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
        Some(f) if is_reportable_fnode(f) => f,
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

fn is_reportable_fnode(fnode: &str) -> bool {
    !(fnode.is_empty() || fnode.starts_with('<') && fnode.ends_with('>'))
}

fn refresh_missing_issues_for_source(conn: &Connection, src_path: &str) -> Result<()> {
    if path_has_blocking_issue(conn, src_path)? {
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
        if path_has_blocking_issue(conn, &src_path)? {
            continue;
        }
        insert_issue(conn, &src_path, "missing", target, &error)?;
    }
    Ok(())
}
