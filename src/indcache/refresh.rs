use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OptionalExtension, ToSql};
use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::core::{FormalCodeStatus, FormalizationStatus};
use crate::indcache::queries::{
    edge_targets_for_source_path, fnode_for_path, path_for_fnode_if_unique,
    path_has_blocking_issue, CHUNK_SIZE,
};
use crate::mdocnode::{MdocHead, MdocIdentity};
use crate::workspace::{iter_mdoc_files, to_indexed_rel_path, FileSnapshotBatch};

// ── Public write functions ────────────────────────────────────────────────────

/// Full workspace scan: reparse every file and delete stale paths.
pub(super) fn refresh_search_index(
    conn: &Connection,
    root: &Path,
) -> Result<crate::formal::status::FormalStatusValidation> {
    let _profile = crate::profile::scope("refresh::refresh_search_index");
    let files = {
        let _phase = crate::profile::scope("refresh::scan_workspace");
        scan_workspace(root)?
    };
    {
        let _phase = crate::profile::scope("refresh::sync_file_states");
        sync_file_states(conn, &files)?;
    }
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
    super::derived::backfill_all_topo_depths(conn)?;
    let formal_validation = crate::formal::status::refresh_index_statuses(conn, root)?;
    conn.execute(
        "UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1",
        [],
    )?;
    Ok(formal_validation)
}

const BULK_ROWS: usize = 200;
const SCAN_BATCH: usize = 2048;
const MAX_SCAN_WORKERS: usize = 12;

struct ScannedMdoc {
    path: String,
    mtime_ns: i64,
    size: i64,
    formal_status: FormalizationStatus,
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

#[derive(PartialEq, Eq)]
struct IndexedFileSemantics {
    node: Option<(String, String, Vec<String>)>,
    invalid: Option<(String, String)>,
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
        .min(MAX_SCAN_WORKERS)
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
    let path_string = to_indexed_rel_path(root, path)?;
    let (mtime_ns, size) = metadata_state(metadata)?;

    match MdocHead::load_bytes(path, content) {
        Ok(head) => Ok(ScannedMdoc {
            path: path_string,
            mtime_ns,
            size,
            formal_status: block_presence_status(&head),
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
                formal_status: FormalizationStatus::default(),
                node,
            })
        }
    }
}

fn build_issues(files: &[ScannedMdoc]) -> Vec<IndexIssue> {
    let mut issues = Vec::new();
    let mut claimants: HashMap<&str, Vec<&str>> = HashMap::new();

    for file in files {
        if let Some(invalid) = &file.invalid {
            issues.push(IndexIssue {
                path: invalid.path.clone(),
                kind: invalid.kind,
                ref_fnode: invalid.ref_fnode.clone(),
                error: invalid.error.clone(),
            });
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
            issues.push(IndexIssue {
                path: (*path).to_string(),
                kind: "duplicate",
                ref_fnode: fnode.to_string(),
                error: error.clone(),
            });
        }
    }
    issues.sort_by(|left, right| {
        (&left.path, left.kind, &left.ref_fnode).cmp(&(&right.path, right.kind, &right.ref_fnode))
    });
    issues
}

fn formal_status_value(status: FormalCodeStatus) -> i64 {
    match status {
        FormalCodeStatus::NoCode => 0,
        FormalCodeStatus::Unverified => 1,
        FormalCodeStatus::Verified => 2,
    }
}

fn sync_file_states(conn: &Connection, files: &[ScannedMdoc]) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT path, mtime_ns, size, lean_status, rocq_status FROM mdoc_files")?;
    let current: HashMap<String, (i64, i64, i64, i64)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
            ))
        })?
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
        .filter(|file| {
            current.get(&file.path)
                != Some(&(
                    file.mtime_ns,
                    file.size,
                    formal_status_value(file.formal_status.lean),
                    formal_status_value(file.formal_status.rocq),
                ))
        })
        .collect();
    for chunk in changed.chunks(BULK_ROWS) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let statuses: Vec<(i64, i64)> = chunk
            .iter()
            .map(|file| {
                (
                    formal_status_value(file.formal_status.lean),
                    formal_status_value(file.formal_status.rocq),
                )
            })
            .collect();
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 5);
        for (file, (lean_status, rocq_status)) in chunk.iter().zip(&statuses) {
            params.push(&file.path);
            params.push(&file.mtime_ns);
            params.push(&file.size);
            params.push(lean_status);
            params.push(rocq_status);
        }
        conn.execute(
            &format!(
                "INSERT INTO mdoc_files
                   (path, mtime_ns, size, lean_status, rocq_status)
                 VALUES {placeholders}
                 ON CONFLICT(path) DO UPDATE SET
                   mtime_ns = excluded.mtime_ns,
                   size = excluded.size,
                   lean_status = excluded.lean_status,
                   rocq_status = excluded.rocq_status"
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
    conn.execute_batch(
        "DELETE FROM mdoc_edges;
         DELETE FROM mdoc_symbols;
         DELETE FROM mdoc_issues;",
    )?;

    let nodes: Vec<(&ScannedMdoc, &ScannedNode)> = files
        .iter()
        .filter_map(|file| file.node.as_ref().map(|node| (file, node)))
        .collect();
    let desired_paths = nodes
        .iter()
        .map(|(file, _)| file.path.as_str())
        .collect::<HashSet<_>>();
    let stale_paths = {
        let mut stmt = conn.prepare("SELECT path FROM mdocs")?;
        let paths = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|path| !desired_paths.contains(path.as_str()))
            .collect::<Vec<_>>();
        paths
    };
    for chunk in stale_paths.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!("DELETE FROM mdocs WHERE path IN ({placeholders})"),
            rusqlite::params_from_iter(chunk),
        )?;
    }
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
            &format!(
                "INSERT INTO mdocs (path, fnode, title, title_lc) VALUES {placeholders}
                 ON CONFLICT(path) DO UPDATE SET
                   fnode = excluded.fnode,
                   title = excluded.title,
                   title_lc = excluded.title_lc
                 WHERE mdocs.fnode != excluded.fnode
                    OR mdocs.title != excluded.title
                    OR mdocs.title_lc != excluded.title_lc"
            ),
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
    insert_edges(conn, &edges)?;

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

fn insert_edges(conn: &Connection, edges: &[(&str, &str, &str, i64)]) -> Result<()> {
    let mut symbols = Vec::with_capacity(edges.len() * 2);
    for (_, source, target, _) in edges {
        symbols.push(*source);
        symbols.push(*target);
    }
    symbols.sort_unstable();
    symbols.dedup();

    for chunk in symbols.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "(?)").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!(
                "INSERT INTO mdoc_symbols (fnode) VALUES {placeholders}
                 ON CONFLICT(fnode) DO NOTHING"
            ),
            rusqlite::params_from_iter(chunk.iter().copied()),
        )?;
    }

    let mut symbol_ids: HashMap<String, i64> = HashMap::with_capacity(symbols.len());
    for chunk in symbols.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT fnode, id FROM mdoc_symbols WHERE fnode IN ({placeholders})"
        ))?;
        for row in stmt.query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })? {
            let (fnode, id) = row?;
            symbol_ids.insert(fnode, id);
        }
    }

    for chunk in edges.chunks(BULK_ROWS) {
        let resolved: Vec<(&str, i64, i64, i64)> = chunk
            .iter()
            .map(|(path, source, target, order)| {
                Ok((
                    *path,
                    *symbol_ids.get(*source).ok_or_else(|| {
                        anyhow::anyhow!("missing interned source symbol {source}")
                    })?,
                    *symbol_ids.get(*target).ok_or_else(|| {
                        anyhow::anyhow!("missing interned target symbol {target}")
                    })?,
                    *order,
                ))
            })
            .collect::<Result<_>>()?;
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() * 4);
        for (path, source, target, order) in &resolved {
            params.push(path);
            params.push(source);
            params.push(target);
            params.push(order);
        }
        conn.execute(
            &format!(
                "INSERT INTO mdoc_edges (src_path, src_symbol_id, dst_symbol_id, ord)
                 VALUES {placeholders}"
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
pub(super) fn refresh_reachable_from_path(
    conn: &Connection,
    root: &Path,
    root_path: &Path,
    depth: i32,
) -> Result<bool> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut graph_changed = false;
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, u32)> =
        std::collections::VecDeque::new();
    let canonical_root = crate::workspace::resolve_mdoc_path(root, root_path)?;
    queue.push_back((canonical_root, 0));

    while let Some((file_path, item_depth)) = queue.pop_front() {
        let file_path = crate::workspace::resolve_mdoc_path(root, &file_path)?;
        let rel_path = to_indexed_rel_path(root, &file_path)?;
        if !seen.insert(rel_path.clone()) {
            continue;
        }
        graph_changed |= upsert_mdoc_row(conn, root, &file_path)?;
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
    Ok(graph_changed)
}

pub(super) fn current_cached_mdoc_path(root: &Path, rel_path: &str) -> Result<Option<PathBuf>> {
    match validate_cached_mdoc_path(root, rel_path) {
        Ok(path) => Ok(Some(path)),
        Err(error) if cached_path_error_is_stale(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn validate_cached_mdoc_path(root: &Path, rel_path: &str) -> Result<PathBuf> {
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

fn cached_path_error_is_stale(error: &anyhow::Error) -> bool {
    let mut io_errors = error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>());
    match io_errors.next() {
        None => true,
        Some(first) => {
            first.kind() == std::io::ErrorKind::NotFound
                && io_errors.all(|error| error.kind() == std::io::ErrorKind::NotFound)
        }
    }
}

/// Upsert a single .mdoc file: update metadata, parse, rebuild edges and issues.
pub(super) fn upsert_mdoc_row(conn: &Connection, root: &Path, file_path: &Path) -> Result<bool> {
    let root_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let file_path = crate::workspace::resolve_mdoc_path(&root_resolved, file_path)?;
    let rel_path = to_indexed_rel_path(&root_resolved, &file_path)?;
    let old_fnode = fnode_for_path(conn, &rel_path)?;
    let old_had_blocking_issue = path_has_blocking_issue(conn, &rel_path)?;
    let old_semantics = indexed_file_semantics(conn, &rel_path)?;

    let mut source_snapshots = FileSnapshotBatch::new(&root_resolved)?;
    let source_snapshot = source_snapshots.capture_read(&file_path)?;
    source_snapshots.finish()?;
    let source_snapshot = match source_snapshot {
        Some(snapshot) => snapshot,
        None => {
            return delete_indexed_path(conn, &rel_path);
        }
    };
    let content = source_snapshot.content();
    let (mtime_ns, size) = metadata_state(source_snapshot.metadata())?;
    // Strict structural parse and tolerant identity fallback both use this one
    // captured byte generation.
    let parse_result = MdocHead::load_bytes(&file_path, content);
    let formal_status = match &parse_result {
        Ok(node) => block_presence_status(node),
        Err(_) => FormalizationStatus::default(),
    };
    let file_state = (
        mtime_ns,
        size,
        formal_status_value(formal_status.lean),
        formal_status_value(formal_status.rocq),
    );
    let new_semantics = parsed_file_semantics(&parse_result, content);
    if !old_had_blocking_issue && old_semantics == new_semantics {
        if indexed_file_state(conn, &rel_path)? != Some(file_state) {
            upsert_file_state(conn, &rel_path, file_state)?;
        }
        return Ok(false);
    }
    if !old_had_blocking_issue && old_semantics.invalid.is_none() && new_semantics.invalid.is_none()
    {
        if let (
            Some((old_fnode, _, old_dependencies)),
            Some((new_fnode, new_title, new_dependencies)),
        ) = (&old_semantics.node, &new_semantics.node)
        {
            if old_fnode == new_fnode && old_dependencies == new_dependencies {
                upsert_file_state(conn, &rel_path, file_state)?;
                upsert_search_row(conn, &rel_path, new_fnode, new_title)?;
                return Ok(false);
            }
        }
    }

    let old_symbol_ids = symbol_ids_for_source_path(conn, &rel_path)?;
    let old_dst_fnodes: HashSet<String> = {
        let mut stmt = conn.prepare(
            "SELECT dst.fnode
             FROM mdoc_edges e
             JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
             WHERE e.src_path = ?",
        )?;
        let rows: HashSet<String> = stmt
            .query_map([&rel_path], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    upsert_file_state(conn, &rel_path, file_state)?;
    conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", [&rel_path])?;
    conn.execute("DELETE FROM mdoc_issues WHERE path = ?", [&rel_path])?;
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
            new_dst_fnodes.extend(head.depens.iter().cloned());
            let edges: Vec<(&str, &str, &str, i64)> = head
                .depens
                .iter()
                .enumerate()
                .map(|(order, dependency)| {
                    (
                        rel_path.as_str(),
                        head.fnode.as_str(),
                        dependency.as_str(),
                        order as i64,
                    )
                })
                .collect();
            insert_edges(conn, &edges)?;
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
    let new_has_blocking_issue = path_has_blocking_issue(conn, &rel_path)?;

    // Collect all fnodes whose in_degree may have changed
    let mut affected: HashSet<String> = old_dst_fnodes.union(&new_dst_fnodes).cloned().collect();
    for fnode in [old_fnode.as_deref(), new_fnode.as_deref()]
        .into_iter()
        .flatten()
    {
        let mut stmt = conn.prepare(
            "SELECT dst.fnode
             FROM mdoc_edges e
             JOIN mdoc_symbols src ON src.id = e.src_symbol_id
             JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
             WHERE src.fnode = ?",
        )?;
        let targets: HashSet<String> = stmt
            .query_map([fnode], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        affected.extend(targets);
    }
    super::derived::refresh_in_degree_for_fnodes(conn, &affected)?;
    prune_orphaned_symbols(conn, &old_symbol_ids)?;

    // Blocking issues filter edges and nodes without changing their stored
    // identities. In particular, a duplicate claimant becoming a malformed
    // partial identity can make another claimant valid while this path remains
    // blocking and reports the same fallback fnode. Conservatively treat the
    // graph as changed whenever a touched path is or was blocking.
    let graph_changed = old_fnode != new_fnode
        || old_dst_fnodes != new_dst_fnodes
        || old_had_blocking_issue
        || new_has_blocking_issue;
    Ok(graph_changed)
}

fn upsert_file_state(
    conn: &Connection,
    rel_path: &str,
    (mtime_ns, size, lean_status, rocq_status): (i64, i64, i64, i64),
) -> Result<()> {
    conn.execute(
        "INSERT INTO mdoc_files
           (path, mtime_ns, size, lean_status, rocq_status)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             mtime_ns = excluded.mtime_ns,
             size = excluded.size,
             lean_status = excluded.lean_status,
             rocq_status = excluded.rocq_status",
        rusqlite::params![rel_path, mtime_ns, size, lean_status, rocq_status,],
    )?;
    Ok(())
}

/// Remove all index entries for a path (file deleted or moved).
pub(super) fn delete_indexed_path(conn: &Connection, stale_path: &str) -> Result<bool> {
    let old_fnode = fnode_for_path(conn, stale_path)?;
    let old_had_blocking_issue = path_has_blocking_issue(conn, stale_path)?;
    let old_symbol_ids = symbol_ids_for_source_path(conn, stale_path)?;
    let old_dst_fnodes: HashSet<String> = {
        let mut stmt = conn.prepare(
            "SELECT dst.fnode
             FROM mdoc_edges e
             JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
             WHERE e.src_path = ?",
        )?;
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

    let mut affected = old_dst_fnodes;
    if let Some(ref fnode) = old_fnode {
        let mut stmt = conn.prepare(
            "SELECT dst.fnode
             FROM mdoc_edges e
             JOIN mdoc_symbols src ON src.id = e.src_symbol_id
             JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
             WHERE src.fnode = ?",
        )?;
        let targets: HashSet<String> = stmt
            .query_map([fnode.as_str()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        affected.extend(targets);
    }
    super::derived::refresh_in_degree_for_fnodes(conn, &affected)?;
    prune_orphaned_symbols(conn, &old_symbol_ids)?;

    Ok(old_fnode.is_some() || !affected.is_empty() || old_had_blocking_issue)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn symbol_ids_for_source_path(conn: &Connection, rel_path: &str) -> Result<HashSet<i64>> {
    let mut stmt =
        conn.prepare("SELECT src_symbol_id, dst_symbol_id FROM mdoc_edges WHERE src_path = ?")?;
    let mut ids = HashSet::new();
    for row in stmt.query_map([rel_path], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (source, target) = row?;
        ids.insert(source);
        ids.insert(target);
    }
    Ok(ids)
}

fn prune_orphaned_symbols(conn: &Connection, ids: &HashSet<i64>) -> Result<()> {
    let ids: Vec<i64> = ids.iter().copied().collect();
    for chunk in ids.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!(
                "DELETE FROM mdoc_symbols
                 WHERE id IN ({placeholders})
                   AND NOT EXISTS (
                       SELECT 1 FROM mdoc_edges WHERE src_symbol_id = mdoc_symbols.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM mdoc_edges WHERE dst_symbol_id = mdoc_symbols.id
                   )"
            ),
            rusqlite::params_from_iter(chunk),
        )?;
    }
    Ok(())
}

fn block_presence_status(head: &MdocHead) -> FormalizationStatus {
    FormalizationStatus {
        lean: if head.has_source_block("lean") {
            FormalCodeStatus::Unverified
        } else {
            FormalCodeStatus::NoCode
        },
        rocq: if head.has_source_block("rocq") {
            FormalCodeStatus::Unverified
        } else {
            FormalCodeStatus::NoCode
        },
    }
}

fn indexed_file_semantics(conn: &Connection, rel_path: &str) -> Result<IndexedFileSemantics> {
    let node = conn
        .query_row(
            "SELECT fnode, title FROM mdocs WHERE path = ?",
            [rel_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let node = match node {
        Some((fnode, title)) => {
            let mut stmt = conn.prepare(
                "SELECT dst.fnode
                 FROM mdoc_edges e
                 JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
                 WHERE e.src_path = ?
                 ORDER BY e.ord",
            )?;
            let dependencies = stmt
                .query_map([rel_path], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Some((fnode, title, dependencies))
        }
        None => None,
    };
    let invalid = conn
        .query_row(
            "SELECT ref_fnode, error FROM mdoc_issues
             WHERE path = ? AND kind = 'invalid'",
            [rel_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(IndexedFileSemantics { node, invalid })
}

fn parsed_file_semantics(parse_result: &Result<MdocHead>, content: &[u8]) -> IndexedFileSemantics {
    match parse_result {
        Ok(head) => IndexedFileSemantics {
            node: Some((head.fnode.clone(), head.title.clone(), head.depens.clone())),
            invalid: None,
        },
        Err(error) => {
            let identity = MdocIdentity::from_bytes(content);
            IndexedFileSemantics {
                node: identity
                    .complete()
                    .map(|(fnode, title)| (fnode.to_string(), title.to_string(), Vec::new())),
                invalid: Some((
                    identity.fnode.unwrap_or_else(|| "<unknown>".into()),
                    error.to_string(),
                )),
            }
        }
    }
}

fn indexed_file_state(conn: &Connection, rel_path: &str) -> Result<Option<(i64, i64, i64, i64)>> {
    conn.query_row(
        "SELECT mtime_ns, size, lean_status, rocq_status
         FROM mdoc_files WHERE path = ?",
        [rel_path],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .optional()
    .map_err(Into::into)
}

pub(super) fn metadata_state(meta: &std::fs::Metadata) -> Result<(i64, i64)> {
    let mtime_ns = i128::from(meta.mtime())
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(i128::from(meta.mtime_nsec())))
        .and_then(|nanoseconds| i64::try_from(nanoseconds).ok())
        .context("file modification time is outside the supported nanosecond range")?;
    let size = i64::try_from(meta.len()).context("file size exceeds the index range")?;
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
        if current_cached_mdoc_path(root, &stale_rel)?.is_none() {
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
    if paths.len() >= 2 {
        let error = format!("duplicate fnode '{}' across: {}", fnode, paths.join(", "));
        for path in &paths {
            insert_issue(conn, path, "duplicate", fnode, &error)?;
        }
    }
    Ok(())
}

fn is_reportable_fnode(fnode: &str) -> bool {
    !(fnode.is_empty() || fnode.starts_with('<') && fnode.ends_with('>'))
}
