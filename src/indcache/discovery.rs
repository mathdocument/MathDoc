//! Workspace change detection using a metadata fast path.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::queries::fnode_for_path;
use super::refresh::{delete_indexed_path, metadata_state, upsert_mdoc_row};

type FileState = (i64, i64);

/// Enumerate the workspace and index files whose mtime or size changed.
/// `refresh_all` remains the explicit strong path for same-metadata edits.
pub fn discover_workspace_changes(
    conn: &Connection,
    root: &Path,
) -> Result<(HashSet<String>, bool)> {
    let known = file_states(conn)?;
    let mut seen = HashSet::new();
    let mut changed_fnodes = HashSet::new();

    for file_path in crate::workspace::iter_mdoc_files(root) {
        let file_path = file_path?;
        let rel_path = crate::workspace::to_rel_path(root, &file_path);
        seen.insert(rel_path.clone());

        let metadata = std::fs::metadata(&file_path)
            .with_context(|| format!("reading metadata for {}", file_path.display()))?;
        if known.get(&rel_path) == Some(&metadata_state(&metadata)) {
            continue;
        }

        let old_fnode = fnode_for_path(conn, &rel_path)?;
        upsert_mdoc_row(conn, root, &file_path)?;
        let new_fnode = fnode_for_path(conn, &rel_path)?;
        if old_fnode != new_fnode {
            changed_fnodes.extend(old_fnode);
        }
        changed_fnodes.extend(new_fnode);
    }

    let mut stale_paths: Vec<String> = known
        .keys()
        .filter(|path| !seen.contains(*path))
        .cloned()
        .collect();
    stale_paths.sort();
    for stale_path in &stale_paths {
        delete_indexed_path(conn, stale_path)?;
    }

    Ok((changed_fnodes, !stale_paths.is_empty()))
}

fn file_states(conn: &Connection) -> Result<HashMap<String, FileState>> {
    let mut stmt = conn.prepare("SELECT path, mtime_ns, size FROM mdoc_files")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
