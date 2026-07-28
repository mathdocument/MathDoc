//! Materialization and invalidation of graph-derived database state.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::core::{all_topo_depths, representative_cycles, weak_component_sizes, GraphIssue};

use super::queries::{dep_graph_snapshot, invalid_issue_rows, valid_node_rows, CHUNK_SIZE};

pub(super) fn ensure_weak_components(conn: &Connection) -> Result<()> {
    let dirty: i32 = conn.query_row(
        "SELECT weak_component_dirty FROM mdoc_index_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    if dirty != 0 {
        recompute_weak_components_full(conn)?;
    }
    Ok(())
}

fn recompute_weak_components_full(conn: &Connection) -> Result<()> {
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?;
    persist_weak_components(conn, &graph, &valid_nodes, &invalid_issues)
}

fn persist_weak_components(
    conn: &Connection,
    graph: &HashMap<String, Vec<String>>,
    valid_nodes: &[(String, String, String)],
    invalid_issues: &[GraphIssue],
) -> Result<()> {
    let members: HashSet<String> = valid_nodes
        .iter()
        .map(|(fnode, _, _)| fnode.clone())
        .chain(
            invalid_issues
                .iter()
                .filter(|issue| !is_placeholder(&issue.fnode))
                .map(|issue| issue.fnode.clone()),
        )
        .collect();
    let mut rows: Vec<(String, u32)> = weak_component_sizes(graph, &members).into_iter().collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));

    conn.execute("DELETE FROM mdoc_weak_component", [])?;
    for chunk in rows.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "(?,?)").collect::<Vec<_>>().join(",");
        let sql = format!(
            "INSERT INTO mdoc_weak_component (fnode, component_size) VALUES {placeholders}"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .flat_map(|(fnode, size)| {
                [
                    fnode as &dyn rusqlite::types::ToSql,
                    size as &dyn rusqlite::types::ToSql,
                ]
            })
            .collect();
        conn.execute(&sql, params.as_slice())?;
    }
    conn.execute(
        "UPDATE mdoc_index_state SET weak_component_dirty = 0 WHERE id = 1",
        [],
    )?;
    Ok(())
}

pub(super) fn ensure_scc_cache(conn: &Connection) -> Result<Vec<Vec<String>>> {
    let current_epoch: i32 = conn.query_row(
        "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    if let Some((cached_epoch, cycles_json)) = read_scc_cache(conn)? {
        if cached_epoch == current_epoch {
            return Ok(serde_json::from_str(&cycles_json)?);
        }
    }

    let graph = dep_graph_snapshot(conn, None, None)?;
    let cycles = representative_cycles(&graph);
    write_scc_cache(conn, current_epoch, &cycles)?;
    Ok(cycles)
}

fn read_scc_cache(conn: &Connection) -> Result<Option<(i32, String)>> {
    Ok(conn
        .query_row(
            "SELECT graph_epoch, cycles_json FROM mdoc_scc_result WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn write_scc_cache(conn: &Connection, graph_epoch: i32, cycles: &[Vec<String>]) -> Result<()> {
    let cycles_json = serde_json::to_string(cycles)?;
    conn.execute(
        "INSERT INTO mdoc_scc_result (id, graph_epoch, cycles_json)
         VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             graph_epoch = excluded.graph_epoch,
             cycles_json = excluded.cycles_json",
        rusqlite::params![graph_epoch, cycles_json],
    )?;
    Ok(())
}

/// Recompute topo depths and weak components from one graph snapshot.
pub(super) fn refresh_all_derived_data(conn: &Connection) -> Result<()> {
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?;

    persist_topo_depths(conn, &all_topo_depths(&graph))?;
    persist_weak_components(conn, &graph, &valid_nodes, &invalid_issues)
}

/// Recompute `start_fnode` and its reverse-reachable ancestors in dependency-first order.
pub(super) fn refresh_topo_depth_upward_from(conn: &Connection, start_fnode: &str) -> Result<()> {
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
            .query_map([&fnode], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for parent in &parents {
            if affected.insert(parent.clone()) {
                queue.push_back(parent.clone());
            }
        }
        reverse.insert(fnode, parents);
    }
    drop(stmt);

    let mut remaining: HashMap<String, usize> =
        affected.iter().map(|fnode| (fnode.clone(), 0)).collect();
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

    // Cycles have no dependency-first ordering and local relaxation would grow forever.
    if processed != affected.len() {
        backfill_all_topo_depths(conn)?;
    }
    Ok(())
}

fn compute_node_topo_depth(conn: &Connection, fnode: &str) -> Result<u32> {
    let max_dep: Option<u32> = conn.query_row(
        "SELECT MAX(m.topo_depth)
         FROM mdoc_valid_edges e
         LEFT JOIN mdocs m ON m.fnode = e.dst_fnode
         WHERE e.src_fnode = ?",
        [fnode],
        |row| row.get::<_, Option<u32>>(0),
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

pub(super) fn backfill_all_topo_depths(conn: &Connection) -> Result<()> {
    let graph = dep_graph_snapshot(conn, None, None)?;
    persist_topo_depths(conn, &all_topo_depths(&graph))
}

fn persist_topo_depths(conn: &Connection, depths: &HashMap<String, u32>) -> Result<()> {
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

pub(super) fn bump_graph_epoch(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE mdoc_index_state
         SET graph_epoch = graph_epoch + 1, weak_component_dirty = 1
         WHERE id = 1",
        [],
    )?;
    Ok(())
}

fn is_placeholder(fnode: &str) -> bool {
    fnode.starts_with('<') && fnode.ends_with('>')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scc_cache_read_propagates_invalid_column_types() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = crate::indcache::schema::open_db(&dir.path().join("index.db")).unwrap();
        conn.execute(
            "INSERT INTO mdoc_scc_result (id, graph_epoch, cycles_json)
             VALUES (1, 'corrupt', '[]')",
            [],
        )
        .unwrap();

        let error = ensure_scc_cache(&conn).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::InvalidColumnType(..))
        ));
    }
}
