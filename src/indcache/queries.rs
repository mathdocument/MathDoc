use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) const CHUNK_SIZE: usize = 500;

use crate::core::{
    component_has_cycle, representative_cycle, strongly_connected_components, DependencyItem,
    DependencyTraversalReport, GraphCheckReport, GraphIssue, GraphRootItem, IssueKind,
};

// ── Public query functions ──────────────────────────────────────────────────

pub fn issue_for_fnode(conn: &Connection, fnode: &str) -> Result<Option<GraphIssue>> {
    Ok(issue_lookup(conn)?.remove(fnode))
}

pub fn ref_item_for_fnode(conn: &Connection, fnode: &str, depth: u32) -> Result<DependencyItem> {
    let nodes = node_lookup(conn)?;
    let issues = issue_lookup(conn)?;
    Ok(dependency_item(fnode, depth, &nodes, &issues))
}

pub fn referrer_items(
    conn: &Connection,
    target_fnode: &str,
    depth: i32,
) -> Result<Vec<DependencyItem>> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    let reverse = reverse_graph(conn)?;
    let nodes = node_lookup(conn)?;
    let issues = issue_lookup(conn)?;

    let mut items = Vec::new();
    let mut seen: HashSet<&str> = HashSet::from([target_fnode]);
    let mut queue: VecDeque<(&str, u32)> = reverse
        .get(target_fnode)
        .into_iter()
        .flat_map(|refs| refs.iter().map(|r| (r.as_str(), 1u32)))
        .collect();

    while let Some((fnode, item_depth)) = queue.pop_front() {
        if !seen.insert(fnode) {
            continue;
        }
        items.push(dependency_item(fnode, item_depth, &nodes, &issues));
        if depth != -1 && item_depth as i32 >= depth {
            continue;
        }
        for referrer in reverse.get(fnode).into_iter().flatten() {
            if referrer != target_fnode {
                queue.push_back((referrer.as_str(), item_depth + 1));
            }
        }
    }
    Ok(items)
}

/// Compute the height of every node from a pre-loaded graph: leaves = 0,
/// a node's height = 1 + max height of its dependencies.
/// Nodes in cycles get whatever height was accumulated before the cycle closes.
pub(super) fn all_topo_depths_impl(graph: &HashMap<String, Vec<String>>) -> HashMap<String, u32> {
    if graph.is_empty() {
        return HashMap::new();
    }
    // Reverse graph: reverse[B] = nodes that depend on B.
    let mut reverse: HashMap<&str, Vec<&str>> =
        graph.keys().map(|k| (k.as_str(), vec![])).collect();
    for (src, dsts) in graph {
        for dst in dsts {
            if graph.contains_key(dst.as_str()) {
                reverse.entry(dst.as_str()).or_default().push(src.as_str());
            }
        }
    }
    // remaining[node] = number of its dependencies not yet processed.
    let mut remaining: HashMap<&str, usize> = graph
        .iter()
        .map(|(k, dsts)| {
            let n = dsts
                .iter()
                .filter(|d| graph.contains_key(d.as_str()))
                .count();
            (k.as_str(), n)
        })
        .collect();
    let mut depth: HashMap<&str, u32> = graph.keys().map(|k| (k.as_str(), 0)).collect();
    let mut queue: std::collections::VecDeque<&str> = remaining
        .iter()
        .filter(|(_, &r)| r == 0)
        .map(|(&f, _)| f)
        .collect();
    while let Some(node) = queue.pop_front() {
        let node_depth = depth[node];
        for &parent in reverse.get(node).into_iter().flatten() {
            if let Some(pd) = depth.get_mut(parent) {
                if node_depth + 1 > *pd {
                    *pd = node_depth + 1;
                }
            }
            let r = remaining.entry(parent).or_insert(0);
            *r = r.saturating_sub(1);
            if *r == 0 {
                queue.push_back(parent);
            }
        }
    }
    depth.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Compute topo depths from scratch via graph traversal. Used by backfill operations.
pub(super) fn compute_all_topo_depths_from_edges(
    conn: &Connection,
) -> Result<HashMap<String, u32>> {
    let graph = dep_graph_snapshot(conn, None, None)?;
    Ok(all_topo_depths_impl(&graph))
}

/// Returns the topo depth of every node in the workspace, reading from the persisted DB column.
pub fn all_topo_depths(conn: &Connection) -> Result<HashMap<String, u32>> {
    let mut stmt = conn.prepare("SELECT fnode, topo_depth FROM mdocs")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// BFS reachability check on `mdoc_edges`. Returns true if `to_fnode` is reachable from
/// `from_fnode` (including the trivial case where they are equal).
pub fn is_reachable(conn: &Connection, from_fnode: &str, to_fnode: &str) -> Result<bool> {
    if from_fnode == to_fnode {
        return Ok(true);
    }
    let mut stmt = conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_fnode = ?")?;
    let mut seen: HashSet<String> = HashSet::from([from_fnode.to_string()]);
    let mut queue: VecDeque<String> = VecDeque::from([from_fnode.to_string()]);
    while let Some(current) = queue.pop_front() {
        let deps: Vec<String> = stmt
            .query_map([&current], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for dep in deps {
            if dep == to_fnode {
                return Ok(true);
            }
            if seen.insert(dep.clone()) {
                queue.push_back(dep);
            }
        }
    }
    Ok(false)
}

/// Returns the direct referrers (depth-1) of `target_fnode` as (fnode, title, rel_path) tuples.
/// Referrers whose own file is invalid/duplicate are excluded.
pub fn direct_referrers_for_fnode(
    conn: &Connection,
    target_fnode: &str,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT e.src_fnode, m.title, m.path
         FROM mdoc_edges e
         JOIN mdocs m ON m.fnode = e.src_fnode
         WHERE e.dst_fnode = ?
           AND NOT EXISTS (
               SELECT 1 FROM mdoc_issues i
               WHERE i.path = e.src_path
                 AND i.kind IN ('invalid', 'duplicate')
           )
         GROUP BY e.src_fnode
         ORDER BY m.path",
    )?;
    let rows = stmt
        .query_map([target_fnode], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn dependency_report(
    conn: &Connection,
    root_fnode: &str,
    depth: i32,
) -> Result<DependencyTraversalReport> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    dependency_report_inner(conn, root_fnode, depth, false)
}

pub fn leaf_dependency_report(
    conn: &Connection,
    root_fnode: &str,
) -> Result<DependencyTraversalReport> {
    dependency_report_inner(conn, root_fnode, -1, true)
}

pub fn global_root_items(conn: &Connection) -> Result<Vec<GraphRootItem>> {
    // Recompute weak components if dirty (requires a full graph load).
    let dirty: i32 = conn.query_row(
        "SELECT weak_component_dirty FROM mdoc_index_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    if dirty != 0 {
        let valid_nodes = valid_node_rows(conn)?;
        let invalid_issues = invalid_issue_rows(conn)?;
        let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?;
        recompute_weak_components_from_graph(conn, &graph, &valid_nodes, &invalid_issues)?;
        conn.execute(
            "UPDATE mdoc_index_state SET weak_component_dirty = 0 WHERE id = 1",
            [],
        )?;
    }

    // Valid root nodes: join with component table and read persisted topo_depth — no graph load.
    let valid_roots: Vec<(String, String, String, u32, u32)> = {
        let mut stmt = conn.prepare(
            "SELECT m.fnode, m.title, m.path, m.topo_depth,
                    COALESCE(w.component_size, 1)
             FROM mdocs m
             LEFT JOIN mdoc_in_degree id ON m.fnode = id.fnode
             LEFT JOIN mdoc_weak_component w ON m.fnode = w.fnode
             WHERE (id.in_degree IS NULL OR id.in_degree = 0)
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE mdoc_issues.path = m.path
                   AND mdoc_issues.kind IN ('invalid', 'duplicate')
               )
             ORDER BY m.path, m.fnode",
        )?;
        let rows: Vec<_> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };

    let fnodes_with_incoming: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT fnode FROM mdoc_in_degree WHERE in_degree > 0")?;
        let rows: HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };

    let component_sizes: HashMap<String, u32> = {
        let mut stmt = conn.prepare("SELECT fnode, component_size FROM mdoc_weak_component")?;
        let rows: HashMap<String, u32> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };

    let invalid_issues = invalid_issue_rows(conn)?;

    let mut items: Vec<GraphRootItem> = valid_roots
        .into_iter()
        .map(
            |(fnode, title, path, topo_depth, component_size)| GraphRootItem {
                fnode,
                title,
                rel_path: path,
                component_size,
                broken: false,
                topo_depth,
            },
        )
        .collect();

    for issue in &invalid_issues {
        let is_placeholder = issue.fnode.starts_with('<') && issue.fnode.ends_with('>');
        if !is_placeholder && fnodes_with_incoming.contains(&issue.fnode) {
            continue;
        }
        let size = component_sizes.get(&issue.fnode).copied().unwrap_or(1);
        items.push(GraphRootItem {
            fnode: issue.fnode.clone(),
            title: issue.title.clone(),
            rel_path: issue.rel_path.clone(),
            component_size: size,
            broken: true,
            topo_depth: 0,
        });
    }

    // Primary: most depended-upon (deepest topo) first; secondary: largest component.
    // Broken nodes (topo_depth=0) sort after valid ones of equal depth.
    items.sort_by(|a, b| {
        b.topo_depth
            .cmp(&a.topo_depth)
            .then(b.component_size.cmp(&a.component_size))
            .then(a.rel_path.cmp(&b.rel_path))
            .then(a.fnode.cmp(&b.fnode))
    });
    Ok(items)
}

pub fn graph_check_report(conn: &Connection) -> Result<GraphCheckReport> {
    let current_epoch: i32 = conn.query_row(
        "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;

    let cached = conn
        .query_row(
            "SELECT graph_epoch, cycles_json FROM mdoc_scc_result WHERE id = 1",
            [],
            |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)),
        )
        .ok();

    let cycles: Vec<Vec<String>> = if let Some((epoch, json)) = cached {
        if epoch == current_epoch {
            serde_json::from_str(&json)?
        } else {
            compute_and_cache_cycles(conn, current_epoch)?
        }
    } else {
        compute_and_cache_cycles(conn, current_epoch)?
    };

    let nodes: i64 = conn.query_row("SELECT COUNT(*) FROM mdoc_files", [], |r| r.get(0))?;
    let edges: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mdoc_edges
         WHERE NOT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE mdoc_issues.path = mdoc_edges.src_path
               AND mdoc_issues.kind IN ('invalid', 'duplicate')
         )",
        [],
        |r| r.get(0),
    )?;

    Ok(GraphCheckReport {
        nodes: nodes as u32,
        edges: edges as u32,
        missing: missing_issue_rows(conn)?,
        invalid: invalid_issue_rows(conn)?,
        cycles,
    })
}

// ── Private helpers ─────────────────────────────────────────────────────────

fn compute_and_cache_cycles(conn: &Connection, current_epoch: i32) -> Result<Vec<Vec<String>>> {
    let graph = dep_graph_snapshot(conn, None, None)?;
    let mut cycles: Vec<Vec<String>> = strongly_connected_components(&graph)
        .into_iter()
        .filter(|c| component_has_cycle(&graph, c))
        .filter_map(|c| representative_cycle(&graph, &c))
        .collect();
    cycles.sort();
    let json = serde_json::to_string(&cycles)?;
    conn.execute(
        "INSERT INTO mdoc_scc_result (id, graph_epoch, cycles_json)
         VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             graph_epoch = excluded.graph_epoch,
             cycles_json = excluded.cycles_json",
        rusqlite::params![current_epoch, json],
    )?;
    Ok(cycles)
}

fn recompute_weak_components_from_graph(
    conn: &Connection,
    graph: &HashMap<String, Vec<String>>,
    valid_nodes: &[(String, String, String)],
    inv_issues: &[GraphIssue],
) -> Result<()> {
    // Members: valid nodes + non-placeholder invalid fnodes
    let members: HashSet<&str> = valid_nodes
        .iter()
        .map(|(f, _, _)| f.as_str())
        .chain(
            inv_issues
                .iter()
                .filter(|i| !(i.fnode.starts_with('<') && i.fnode.ends_with('>')))
                .map(|i| i.fnode.as_str()),
        )
        .collect();

    // Build undirected adjacency from the directed graph
    let mut adj: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (src, deps) in graph {
        if !members.contains(src.as_str()) {
            continue;
        }
        adj.entry(src.as_str()).or_default();
        for dst in deps {
            if !members.contains(dst.as_str()) {
                continue;
            }
            adj.entry(dst.as_str()).or_default().insert(src.as_str());
            adj.entry(src.as_str()).or_default().insert(dst.as_str());
        }
    }

    // BFS to find connected components. Single pass: collect (fnode, component_id, size).
    // component_id = lex-min fnode string in the component.
    let mut rows: Vec<(String, String, u32)> = Vec::new(); // (fnode, component_id, size)
    let mut seen: HashSet<&str> = HashSet::new();
    let mut sorted_starts: Vec<&&str> = adj.keys().collect();
    sorted_starts.sort();
    for &start in sorted_starts {
        if seen.contains(start) {
            continue;
        }
        let mut component: Vec<&str> = Vec::new();
        let mut queue: VecDeque<&str> = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            if !seen.insert(node) {
                continue;
            }
            component.push(node);
            for &neighbor in adj.get(node).into_iter().flatten() {
                if !seen.contains(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        let size = component.len() as u32;
        let rep = component.iter().copied().min().unwrap_or(start).to_string();
        for node in component {
            rows.push((node.to_string(), rep.clone(), size));
        }
    }

    conn.execute("DELETE FROM mdoc_weak_component", [])?;
    for chunk in rows.chunks(CHUNK_SIZE) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?,?,?)")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT INTO mdoc_weak_component (fnode, component_id, component_size) VALUES {placeholders}"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .flat_map(|(f, cid, s)| {
                [
                    f as &dyn rusqlite::types::ToSql,
                    cid as &dyn rusqlite::types::ToSql,
                    s as &dyn rusqlite::types::ToSql,
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

/// Full weak-component recompute from scratch, including dirty-flag reset.
pub(super) fn recompute_weak_components_full(conn: &Connection) -> Result<()> {
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?;
    recompute_weak_components_from_graph(conn, &graph, &valid_nodes, &invalid_issues)
}

/// Recompute both `topo_depth` (mdocs) and weak components (mdoc_weak_component) from a
/// single graph load. Use this after bulk write operations instead of two separate passes.
pub(super) fn refresh_all_derived_data(conn: &Connection) -> Result<()> {
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&invalid_issues))?;

    // Topo depths — one combined Kahn pass, then bulk UPDATE.
    let depths = all_topo_depths_impl(&graph);
    for chunk in depths.iter().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
        for (fnode, depth) in chunk {
            conn.execute(
                "UPDATE mdocs SET topo_depth = ? WHERE fnode = ?",
                rusqlite::params![depth, fnode],
            )?;
        }
    }

    // Weak components — clears and rebuilds mdoc_weak_component, resets dirty=0.
    recompute_weak_components_from_graph(conn, &graph, &valid_nodes, &invalid_issues)
}

fn dependency_report_inner(
    conn: &Connection,
    root_fnode: &str,
    depth: i32,
    leaf_only: bool,
) -> Result<DependencyTraversalReport> {
    // Validate root
    let mut nodes = node_lookup_for_fnodes(conn, &[root_fnode])?;
    let mut issues = issue_lookup_for_fnodes(conn, &[root_fnode])?;
    if let Some(issue) = issues.get(root_fnode) {
        bail!("{}", issue.error);
    }
    if !nodes.contains_key(root_fnode) {
        bail!("no mdoc matched reference: {root_fnode}");
    }

    let mut report_graph: HashMap<String, Vec<String>> =
        HashMap::from([(root_fnode.to_string(), vec![])]);
    let mut items: Vec<DependencyItem> = Vec::new();
    let mut discovered: HashSet<String> = HashSet::from([root_fnode.to_string()]);
    let mut queue: VecDeque<(String, u32)> = VecDeque::from([(root_fnode.to_string(), 0)]);

    while !queue.is_empty() {
        // Drain up to 200 items as a batch
        let batch: Vec<(String, u32)> = (0..200).map_while(|_| queue.pop_front()).collect();

        let expandable: Vec<&str> = batch
            .iter()
            .filter(|(_, d)| leaf_only || depth == -1 || (*d as i32) < depth)
            .map(|(f, _)| f.as_str())
            .collect();
        let edges = edge_lookup_for_sources(conn, &expandable)?;

        let mut pending: Vec<(String, u32)> = Vec::new();
        for (fnode, item_depth) in &batch {
            if !leaf_only && depth != -1 && (*item_depth as i32) >= depth {
                report_graph.insert(fnode.clone(), vec![]);
                continue;
            }
            let dep_fnodes = edges.get(fnode.as_str()).cloned().unwrap_or_default();
            report_graph.insert(fnode.clone(), dep_fnodes.clone());

            if leaf_only && fnode != root_fnode && dep_fnodes.is_empty() {
                items.push(dependency_item(fnode, *item_depth, &nodes, &issues));
            }
            for dep in dep_fnodes {
                if discovered.insert(dep.clone()) {
                    pending.push((dep, item_depth + 1));
                }
            }
        }

        if !pending.is_empty() {
            let pending_fnodes: Vec<&str> = pending.iter().map(|(f, _)| f.as_str()).collect();
            nodes.extend(node_lookup_for_fnodes(conn, &pending_fnodes)?);
            issues.extend(issue_lookup_for_fnodes(conn, &pending_fnodes)?);

            for (fnode, item_depth) in &pending {
                if !leaf_only {
                    items.push(dependency_item(fnode, *item_depth, &nodes, &issues));
                }
                queue.push_back((fnode.clone(), *item_depth));
            }
        }
    }

    let mut cycles: Vec<Vec<String>> = strongly_connected_components(&report_graph)
        .into_iter()
        .filter(|c| component_has_cycle(&report_graph, c))
        .filter_map(|c| representative_cycle(&report_graph, &c))
        .collect();
    cycles.sort();

    let issues_in_graph: HashMap<String, GraphIssue> = issues
        .into_iter()
        .filter(|(f, _)| report_graph.contains_key(f.as_str()))
        .collect();

    Ok(DependencyTraversalReport {
        root_fnode: root_fnode.to_string(),
        items,
        dep_graph: report_graph,
        issues_by_fnode: issues_in_graph,
        cycles,
    })
}

// ── Low-level data accessors ─────────────────────────────────────────────────

fn valid_node_rows(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT mdocs.fnode, mdocs.title, mdocs.path
         FROM mdocs
         WHERE NOT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE mdoc_issues.path = mdocs.path
               AND mdoc_issues.kind IN ('invalid', 'duplicate')
         )
         ORDER BY mdocs.path, mdocs.fnode",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn node_lookup(conn: &Connection) -> Result<HashMap<String, (String, String)>> {
    Ok(valid_node_rows(conn)?
        .into_iter()
        .map(|(f, t, p)| (f, (t, p)))
        .collect())
}

fn issue_lookup(conn: &Connection) -> Result<HashMap<String, GraphIssue>> {
    let mut map: HashMap<String, GraphIssue> = HashMap::new();
    for issue in invalid_issue_rows(conn)? {
        map.entry(issue.fnode.clone()).or_insert(issue);
    }
    for issue in missing_issue_rows(conn)? {
        map.entry(issue.fnode.clone()).or_insert(issue);
    }
    Ok(map)
}

fn invalid_issue_rows(conn: &Connection) -> Result<Vec<GraphIssue>> {
    let mut stmt = conn.prepare(
        "SELECT path, ref_fnode, error FROM mdoc_issues
         WHERE kind IN ('invalid', 'duplicate')
         ORDER BY path, ref_fnode, error",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(GraphIssue {
                kind: IssueKind::Invalid,
                fnode: r.get(1)?,
                title: "<invalid>".to_string(),
                rel_path: r.get(0)?,
                error: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn missing_issue_rows(conn: &Connection) -> Result<Vec<GraphIssue>> {
    let mut stmt = conn.prepare(
        "SELECT ref_fnode, error FROM mdoc_issues
         WHERE kind = 'missing'
           AND NOT EXISTS (
             SELECT 1 FROM mdoc_issues AS si
             WHERE si.path = mdoc_issues.path
               AND si.kind IN ('invalid', 'duplicate')
           )
         ORDER BY ref_fnode, path",
    )?;
    let mut deduped: Vec<GraphIssue> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (fnode, error) = row?;
        if seen.insert(fnode.clone()) {
            deduped.push(GraphIssue {
                kind: IssueKind::Missing,
                fnode,
                title: "<missing>".to_string(),
                rel_path: "<unknown>".to_string(),
                error,
            });
        }
    }
    Ok(deduped)
}

fn dep_graph_snapshot(
    conn: &Connection,
    valid_nodes: Option<&[(String, String, String)]>,
    inv_issues: Option<&[GraphIssue]>,
) -> Result<HashMap<String, Vec<String>>> {
    let owned_nodes;
    let owned_issues;
    let nodes = match valid_nodes {
        Some(v) => v,
        None => {
            owned_nodes = valid_node_rows(conn)?;
            &owned_nodes
        }
    };
    let issues = match inv_issues {
        Some(i) => i,
        None => {
            owned_issues = invalid_issue_rows(conn)?;
            &owned_issues
        }
    };

    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for (fnode, _, _) in nodes {
        graph.entry(fnode.clone()).or_default();
    }
    for issue in issues {
        if !(issue.fnode.starts_with('<') && issue.fnode.ends_with('>')) {
            graph.entry(issue.fnode.clone()).or_default();
        }
    }

    let mut stmt = conn.prepare(
        "SELECT src_fnode, dst_fnode FROM mdoc_edges
         WHERE NOT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE mdoc_issues.path = mdoc_edges.src_path
               AND mdoc_issues.kind IN ('invalid', 'duplicate')
         )
         ORDER BY src_path, ord",
    )?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (src, dst) = row?;
        graph.entry(src.clone()).or_default().push(dst.clone());
        graph.entry(dst).or_default();
    }
    Ok(graph)
}

fn reverse_graph(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut rev: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT src_fnode, dst_fnode FROM mdoc_edges
         WHERE NOT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE mdoc_issues.path = mdoc_edges.src_path
               AND mdoc_issues.kind IN ('invalid', 'duplicate')
         )
         ORDER BY src_path, ord",
    )?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (src, dst) = row?;
        rev.entry(dst).or_default().push(src);
    }
    Ok(rev)
}

fn node_lookup_for_fnodes(
    conn: &Connection,
    fnodes: &[&str],
) -> Result<HashMap<String, (String, String)>> {
    if fnodes.is_empty() {
        return Ok(HashMap::new());
    }
    let mut result = HashMap::new();
    for chunk in fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT mdocs.fnode, mdocs.title, mdocs.path
             FROM mdocs
             WHERE mdocs.fnode IN ({placeholders})
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE mdoc_issues.path = mdocs.path
                   AND mdoc_issues.kind IN ('invalid', 'duplicate')
               )"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|f| f as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(params.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (f, t, p) = row?;
            result.insert(f, (t, p));
        }
    }
    Ok(result)
}

fn issue_lookup_for_fnodes(
    conn: &Connection,
    fnodes: &[&str],
) -> Result<HashMap<String, GraphIssue>> {
    if fnodes.is_empty() {
        return Ok(HashMap::new());
    }
    let mut result: HashMap<String, GraphIssue> = HashMap::new();
    for chunk in fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT path, kind, ref_fnode, error FROM mdoc_issues
             WHERE ref_fnode IN ({placeholders})
               AND (
                 kind IN ('invalid', 'duplicate')
                 OR (kind = 'missing' AND NOT EXISTS (
                   SELECT 1 FROM mdoc_issues AS si
                   WHERE si.path = mdoc_issues.path
                     AND si.kind IN ('invalid', 'duplicate')
                 ))
               )
             ORDER BY path, ref_fnode, error"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|f| f as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(params.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (path, kind, fnode, error) = row?;
            result.entry(fnode.clone()).or_insert_with(|| {
                if kind == "missing" {
                    GraphIssue {
                        kind: IssueKind::Missing,
                        fnode,
                        title: "<missing>".to_string(),
                        rel_path: "<unknown>".to_string(),
                        error,
                    }
                } else {
                    GraphIssue {
                        kind: IssueKind::Invalid,
                        fnode,
                        title: "<invalid>".to_string(),
                        rel_path: path,
                        error,
                    }
                }
            });
        }
    }
    Ok(result)
}

fn edge_lookup_for_sources<'a>(
    conn: &Connection,
    src_fnodes: &[&'a str],
) -> Result<HashMap<&'a str, Vec<String>>> {
    if src_fnodes.is_empty() {
        return Ok(HashMap::new());
    }
    let positions: HashMap<&str, usize> = src_fnodes
        .iter()
        .enumerate()
        .map(|(i, &f)| (f, i))
        .collect();
    let mut edge_rows: Vec<(usize, String, String, i32)> = Vec::new();

    for chunk in src_fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT src_fnode, dst_fnode, ord FROM mdoc_edges
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
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)?,
            ))
        })? {
            let (src, dst, ord) = row?;
            let pos = positions[src.as_str()];
            edge_rows.push((pos, src, dst, ord));
        }
    }
    edge_rows.sort_by_key(|&(pos, _, _, ord)| (pos, ord));

    let mut result: HashMap<&str, Vec<String>> =
        src_fnodes.iter().map(|&f| (f, Vec::new())).collect();
    for (_, src, dst, _) in edge_rows {
        result.get_mut(src.as_str()).unwrap().push(dst);
    }
    Ok(result)
}

fn dependency_item(
    fnode: &str,
    depth: u32,
    nodes: &HashMap<String, (String, String)>,
    issues: &HashMap<String, GraphIssue>,
) -> DependencyItem {
    if let Some(issue) = issues.get(fnode) {
        return DependencyItem {
            depth,
            fnode: issue.fnode.clone(),
            title: issue.title.clone(),
            rel_path: issue.rel_path.clone(),
        };
    }
    if let Some((title, path)) = nodes.get(fnode) {
        return DependencyItem {
            depth,
            fnode: fnode.to_string(),
            title: title.clone(),
            rel_path: path.clone(),
        };
    }
    DependencyItem {
        depth,
        fnode: fnode.to_string(),
        title: "<missing>".to_string(),
        rel_path: "<unknown>".to_string(),
    }
}

// ── Helper to resolve a path reference from the DB ──────────────────────────

pub fn lookup_by_fnode(
    conn: &Connection,
    fnodes: &[&str],
) -> Result<HashMap<String, (String, String)>> {
    if fnodes.is_empty() {
        return Ok(HashMap::new());
    }
    let mut result: HashMap<String, (String, String)> = HashMap::new();
    let mut duplicates: HashSet<String> = HashSet::new();
    for chunk in fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT fnode, title, path FROM mdocs WHERE fnode IN ({placeholders})");
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|f| f as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(params.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (fnode, title, path) = row?;
            if duplicates.contains(&fnode) {
                continue;
            }
            let entry = (title, path);
            if let Some(existing) = result.get(&fnode) {
                if *existing != entry {
                    duplicates.insert(fnode.clone());
                    result.remove(&fnode);
                    continue;
                }
            }
            result.insert(fnode, entry);
        }
    }
    Ok(result)
}

pub fn resolve_ref_by_path(conn: &Connection, rel_path: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT fnode, title FROM mdocs WHERE path = ?",
            [rel_path],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok())
}

pub fn search(conn: &Connection, query: &str) -> Result<Vec<(String, String, String)>> {
    let query_lc = query.to_lowercase();
    let like = format!("%{query_lc}%");
    let prefix_like = format!("{query_lc}%");
    let mut stmt = conn.prepare(
        "SELECT fnode, title, path FROM mdocs
         WHERE title_lc LIKE ? OR lower(fnode) LIKE ?
         ORDER BY
             CASE WHEN lower(fnode) LIKE ? THEN 0 ELSE 1 END,
             CASE WHEN instr(title_lc, ?) > 0 THEN instr(title_lc, ?) ELSE 999999 END,
             length(title),
             path",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![like, like, prefix_like, query_lc, query_lc],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn exact_fnode_rows(conn: &Connection, fnode: &str) -> Result<Vec<(String, String, String)>> {
    let fnode_lc = fnode.to_lowercase();
    let mut stmt =
        conn.prepare("SELECT fnode, title, path FROM mdocs WHERE lower(fnode) = ? ORDER BY path")?;
    let rows = stmt
        .query_map([fnode_lc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn is_bootstrapped(conn: &Connection) -> Result<bool> {
    let row: i32 = conn.query_row(
        "SELECT bootstrapped FROM mdoc_index_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(row != 0)
}

pub fn fnode_for_path(conn: &Connection, rel_path: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT fnode FROM mdocs WHERE path = ?", [rel_path], |r| {
            r.get::<_, String>(0)
        })
        .ok())
}

pub fn resolve_fnode_ref(
    conn: &Connection,
    raw_ref: &str,
) -> Result<Option<Vec<(String, String, String)>>> {
    let query_lc = raw_ref.to_lowercase();
    let prefix_like = format!("{query_lc}%");
    let mut stmt = conn.prepare(
        "SELECT fnode, title, path FROM mdocs
         WHERE lower(fnode) = ? OR lower(fnode) LIKE ?
         ORDER BY CASE WHEN lower(fnode) = ? THEN 0 ELSE 1 END, path",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map(rusqlite::params![query_lc, prefix_like, query_lc], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rows))
    }
}

pub fn knows_fnode(conn: &Connection, fnode: &str) -> Result<bool> {
    let in_mdocs: bool = conn
        .query_row(
            "SELECT 1 FROM mdocs WHERE fnode = ? LIMIT 1",
            [fnode],
            |_| Ok(()),
        )
        .is_ok();
    if in_mdocs {
        return Ok(true);
    }
    Ok(conn
        .query_row(
            "SELECT 1 FROM mdoc_issues WHERE ref_fnode = ? LIMIT 1",
            [fnode],
            |_| Ok(()),
        )
        .is_ok())
}

pub fn indexed_file_count(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM mdoc_files", [], |r| {
        r.get::<_, i64>(0)
    })? as u32)
}

pub fn mdoc_count(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM mdocs", [], |r| r.get::<_, i64>(0))? as u32)
}

pub fn path_has_blocking_issue(conn: &Connection, rel_path: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM mdoc_issues
             WHERE path = ? AND kind IN ('invalid', 'duplicate') LIMIT 1",
            [rel_path],
            |_| Ok(()),
        )
        .is_ok())
}

pub fn edge_targets_for_source_path(conn: &Connection, src_path: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT dst_fnode FROM mdoc_edges WHERE src_path = ? ORDER BY ord")?;
    let rows = stmt
        .query_map([src_path], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn path_for_fnode_if_unique(conn: &Connection, fnode: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT path FROM mdocs WHERE fnode = ? ORDER BY path LIMIT 2")?;
    let paths: Vec<String> = stmt
        .query_map([fnode], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(if paths.len() == 1 {
        Some(paths.into_iter().next().unwrap())
    } else {
        None
    })
}
