//! Materialization of graph-derived database state.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::core::all_topo_depths;

use super::queries::{dep_graph_snapshot, CHUNK_SIZE};

pub(super) fn backfill_all_topo_depths(conn: &Connection) -> Result<()> {
    // ponytail: Recompute every depth after graph mutations; restore incremental
    // updates only if profiling identifies this as a bottleneck.
    let graph = dep_graph_snapshot(conn)?;
    persist_topo_depths(conn, &all_topo_depths(&graph))
}

fn persist_topo_depths(conn: &Connection, depths: &HashMap<String, u32>) -> Result<()> {
    let _profile = crate::profile::scope("derived::persist_topo_depths");
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS mdc_topo_updates (
             fnode TEXT PRIMARY KEY,
             depth INTEGER NOT NULL
         ) WITHOUT ROWID;
         DELETE FROM mdc_topo_updates;",
    )?;
    let rows = depths.iter().collect::<Vec<_>>();
    for chunk in rows.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "(?,?)").collect::<Vec<_>>().join(",");
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(chunk.len() * 2);
        for (fnode, depth) in chunk {
            params.push(fnode);
            params.push(depth);
        }
        conn.execute(
            &format!("INSERT INTO mdc_topo_updates (fnode, depth) VALUES {placeholders}"),
            params.as_slice(),
        )?;
    }
    conn.execute_batch("UPDATE mdocs SET topo_depth = 0;")?;
    conn.execute(
        "UPDATE mdocs
         SET topo_depth = (SELECT depth FROM mdc_topo_updates u WHERE u.fnode = mdocs.fnode)
         WHERE fnode IN (SELECT fnode FROM mdc_topo_updates)",
        [],
    )?;
    Ok(())
}

pub(super) fn refresh_in_degree_for_fnodes(
    conn: &Connection,
    fnodes: &HashSet<String>,
) -> Result<()> {
    if fnodes.is_empty() {
        return Ok(());
    }
    let fnode_vec: Vec<&str> = fnodes.iter().map(String::as_str).collect();
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
