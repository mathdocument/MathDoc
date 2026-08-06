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
    let _profile = crate::profile::scope("derived::ensure_scc_cache");
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

    let graph = {
        let _phase = crate::profile::scope("derived::load_scc_graph");
        dep_graph_snapshot(conn, None, None)?
    };
    let cycles = {
        let _phase = crate::profile::scope("algorithm::representative_cycles");
        representative_cycles(&graph)
    };
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
    let _profile = crate::profile::scope("derived::refresh_all_derived_data");
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = {
        let _phase = crate::profile::scope("derived::load_graph");
        dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?
    };

    let depths = {
        let _phase = crate::profile::scope("algorithm::all_topo_depths");
        all_topo_depths(&graph)
    };
    {
        let _phase = crate::profile::scope("derived::persist_topo_depths");
        persist_topo_depths(conn, &depths)?;
    }
    let _phase = crate::profile::scope("derived::persist_weak_components");
    persist_weak_components(conn, &graph, &valid_nodes, &invalid_issues)
}

struct DepthState {
    has_dependencies: bool,
    remaining: usize,
    max_dependency_depth: u32,
}

/// Recompute all seeds and their reverse-reachable ancestors in one set operation.
pub(super) fn refresh_topo_depth_upward_from_many(
    conn: &Connection,
    seeds: &HashSet<String>,
) -> Result<()> {
    let _profile = crate::profile::scope("derived::refresh_topo_depth_upward_from_many");
    if seeds.is_empty() {
        return Ok(());
    }
    let collect_profile = crate::profile::scope("derived::collect_affected_topo_nodes");
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS mdc_topo_seeds (
             fnode TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         CREATE TEMP TABLE IF NOT EXISTS mdc_topo_affected (
             fnode TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         DELETE FROM mdc_topo_seeds;
         DELETE FROM mdc_topo_affected;",
    )?;
    let seeds = seeds.iter().collect::<Vec<_>>();
    for chunk in seeds.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "(?)").collect::<Vec<_>>().join(",");
        conn.execute(
            &format!("INSERT INTO mdc_topo_seeds (fnode) VALUES {placeholders}"),
            rusqlite::params_from_iter(chunk.iter().copied()),
        )?;
    }
    conn.execute_batch(
        "INSERT OR IGNORE INTO mdc_topo_affected (fnode)
         WITH RECURSIVE affected(fnode) AS (
             SELECT fnode FROM mdc_topo_seeds
             UNION
             SELECT e.src_fnode
             FROM mdoc_valid_edges e
             JOIN affected a ON a.fnode = e.dst_fnode
         )
         SELECT fnode FROM affected;",
    )?;
    drop(collect_profile);

    let load_profile = crate::profile::scope("derived::load_affected_topo_edges");
    let mut states = conn
        .prepare("SELECT fnode FROM mdc_topo_affected")?
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| {
            row.map(|fnode| {
                (
                    fnode,
                    DepthState {
                        has_dependencies: false,
                        remaining: 0,
                        max_dependency_depth: 0,
                    },
                )
            })
        })
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let mut referrers: HashMap<String, Vec<String>> = HashMap::new();
    let mut affected_edges = HashSet::new();
    let mut external_edges = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT e.src_fnode, e.dst_fnode
         FROM mdoc_valid_edges e
         JOIN mdc_topo_affected a ON a.fnode = e.src_fnode",
    )?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (source, dependency) = row?;
        let dependency_affected = states.contains_key(&dependency);
        let state = states
            .get_mut(&source)
            .expect("affected edge source was initialized");
        state.has_dependencies = true;
        if dependency_affected {
            if affected_edges.insert((source.clone(), dependency.clone())) {
                state.remaining += 1;
                referrers.entry(dependency).or_default().push(source);
            }
        } else {
            external_edges.push((source, dependency));
        }
    }
    drop(stmt);

    let external_fnodes = external_edges
        .iter()
        .map(|(_, dependency)| dependency)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut external_depths = HashMap::new();
    for chunk in external_fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT fnode, MAX(topo_depth) FROM mdocs
             WHERE fnode IN ({placeholders}) GROUP BY fnode"
        );
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })? {
            let (fnode, depth) = row?;
            external_depths.insert(fnode, depth);
        }
    }
    for (source, dependency) in external_edges {
        if let Some(depth) = external_depths.get(&dependency) {
            let state = states
                .get_mut(&source)
                .expect("affected edge source was initialized");
            state.max_dependency_depth = state.max_dependency_depth.max(*depth);
        }
    }
    drop(load_profile);

    let compute_profile = crate::profile::scope("derived::compute_affected_topo_depths");
    let mut ready = states
        .iter()
        .filter(|(_, state)| state.remaining == 0)
        .map(|(fnode, _)| fnode.clone())
        .collect::<VecDeque<_>>();
    let mut depths = HashMap::with_capacity(states.len());
    while let Some(fnode) = ready.pop_front() {
        let state = states.get(&fnode).expect("ready node has a depth state");
        let depth = if state.has_dependencies {
            state.max_dependency_depth + 1
        } else {
            0
        };
        depths.insert(fnode.clone(), depth);
        for referrer in referrers.get(&fnode).into_iter().flatten() {
            let state = states
                .get_mut(referrer)
                .expect("affected referrer has a depth state");
            state.remaining -= 1;
            state.max_dependency_depth = state.max_dependency_depth.max(depth);
            if state.remaining == 0 {
                ready.push_back(referrer.clone());
            }
        }
    }
    if depths.len() != states.len() {
        return backfill_all_topo_depths(conn);
    }
    drop(compute_profile);
    persist_topo_depths(conn, &depths)
}

pub(super) fn backfill_all_topo_depths(conn: &Connection) -> Result<()> {
    let graph = dep_graph_snapshot(conn, None, None)?;
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
