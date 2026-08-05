//! Workspace change detection using a metadata fast path.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

use super::queries::fnode_for_path;
use super::refresh::{delete_indexed_path, metadata_state, upsert_mdoc_row};

type FileState = (i64, i64);

pub(super) struct WorkspaceChanges {
    changed_paths: Vec<PathBuf>,
    stale_paths: Vec<String>,
}

impl WorkspaceChanges {
    pub(super) fn is_empty(&self) -> bool {
        self.changed_paths.is_empty() && self.stale_paths.is_empty()
    }
}

/// Enumerate the workspace and index files whose mtime or size changed.
/// `refresh_all` remains the explicit strong path for same-metadata edits.
pub(super) fn discover_workspace_changes(
    conn: &Connection,
    root: &Path,
) -> Result<WorkspaceChanges> {
    let mut known = file_states(conn)?;
    for orphan in orphaned_index_paths(conn)? {
        known.entry(orphan).or_insert(None);
    }

    let files = scan_workspace_metadata(root)?;
    let mut changed_paths = Vec::new();
    for (file_path, rel_path, state) in files {
        if known.remove(&rel_path) != Some(Some(state)) {
            changed_paths.push(file_path);
        }
    }

    let mut stale_paths: Vec<String> = known.into_keys().collect();
    stale_paths.sort();
    Ok(WorkspaceChanges {
        changed_paths,
        stale_paths,
    })
}

pub(super) fn apply_workspace_changes(
    conn: &Connection,
    root: &Path,
    changes: WorkspaceChanges,
) -> Result<(HashSet<String>, bool)> {
    let mut changed_fnodes = HashSet::new();

    for file_path in changes.changed_paths {
        let rel_path = crate::workspace::to_rel_path(root, &file_path);
        let old_fnode = fnode_for_path(conn, &rel_path)?;
        upsert_mdoc_row(conn, root, &file_path)?;
        let new_fnode = fnode_for_path(conn, &rel_path)?;
        if old_fnode != new_fnode {
            changed_fnodes.extend(old_fnode);
        }
        changed_fnodes.extend(new_fnode);
    }

    for stale_path in &changes.stale_paths {
        delete_indexed_path(conn, stale_path)?;
    }

    Ok((changed_fnodes, !changes.stale_paths.is_empty()))
}

fn orphaned_index_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM mdocs
         WHERE NOT EXISTS (SELECT 1 FROM mdoc_files f WHERE f.path = mdocs.path)
         UNION
         SELECT path FROM mdoc_issues
         WHERE NOT EXISTS (SELECT 1 FROM mdoc_files f WHERE f.path = mdoc_issues.path)",
    )?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn file_states(conn: &Connection) -> Result<HashMap<String, Option<FileState>>> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Some((row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn scan_workspace_metadata(root: &Path) -> Result<Vec<(PathBuf, String, FileState)>> {
    let mut paths = crate::workspace::iter_mdoc_files(root).collect::<Result<Vec<_>>>()?;
    paths.sort();
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
        let workers = paths
            .chunks(chunk_size)
            .map(|paths| {
                scope.spawn(move || {
                    paths
                        .iter()
                        .map(|file_path| {
                            let metadata = std::fs::metadata(file_path).with_context(|| {
                                format!("reading metadata for {}", file_path.display())
                            })?;
                            if metadata.nlink() > 1 {
                                bail!(
                                    "refusing to access hard-linked file {} ({} links)",
                                    file_path.display(),
                                    metadata.nlink()
                                );
                            }
                            Ok((
                                file_path.clone(),
                                crate::workspace::to_rel_path(root, file_path),
                                metadata_state(&metadata)?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect::<Vec<_>>();

        let mut files = Vec::with_capacity(paths.len());
        for worker in workers {
            files.extend(
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("workspace metadata worker panicked"))??,
            );
        }
        Ok(files)
    })
}
