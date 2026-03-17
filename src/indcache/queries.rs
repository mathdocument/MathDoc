//! Read-only query functions over an open SQLite connection.
//! All functions take `&Connection`; the caller owns the transaction boundary.

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) const CHUNK_SIZE: usize = 500;

use crate::core::{
    component_has_cycle, representative_cycle, strongly_connected_components,
    DependencyItem, DependencyTraversalReport, GraphCheckReport, GraphIssue, GraphRootItem,
    IssueKind,
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
    // Recompute weak components if dirty
    let dirty: i32 = conn.query_row(
        "SELECT weak_component_dirty FROM mdoc_index_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    if dirty != 0 {
        recompute_weak_components(conn)?;
        conn.execute(
            "UPDATE mdoc_index_state SET weak_component_dirty = 0 WHERE id = 1",
            [],
        )?;
    }

    let valid_roots: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT mdocs.fnode, mdocs.title, mdocs.path
             FROM mdocs
             LEFT JOIN mdoc_in_degree ON mdocs.fnode = mdoc_in_degree.fnode
             WHERE (mdoc_in_degree.in_degree IS NULL OR mdoc_in_degree.in_degree = 0)
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE mdoc_issues.path = mdocs.path
                   AND mdoc_issues.kind IN ('invalid', 'duplicate')
               )
             ORDER BY mdocs.path, mdocs.fnode",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };

    let invalid_issues = invalid_issue_rows(conn)?;

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

    let mut items: Vec<GraphRootItem> = valid_roots
        .into_iter()
        .map(|(fnode, title, path)| {
            let size = component_sizes.get(&fnode).copied().unwrap_or(1);
            GraphRootItem {
                fnode,
                title,
                rel_path: path,
                component_size: size,
                broken: false,
            }
        })
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
        });
    }

    items.sort_by(|a, b| {
        b.component_size
            .cmp(&a.component_size)
            .then(a.rel_path.cmp(&b.rel_path))
            .then(a.title.cmp(&b.title))
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

pub(crate) fn recompute_weak_components(conn: &Connection) -> Result<()> {
    let valid_nodes = valid_node_rows(conn)?;
    let inv_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn, Some(&valid_nodes), Some(&inv_issues))?;

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
    for (src, deps) in &graph {
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

    // BFS to find connected components
    let mut sizes: HashMap<&str, u32> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut sorted_starts: Vec<&&str> = adj.keys().collect();
    sorted_starts.sort();
    for &start in sorted_starts {
        if seen.contains(start) {
            continue;
        }
        let mut component: Vec<&str> = Vec::new();
        let mut queue = VecDeque::from([start]);
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
        for node in component {
            sizes.insert(node, size);
        }
    }

    conn.execute("DELETE FROM mdoc_weak_component", [])?;
    for chunk in sizes.iter().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "(?,?)").collect::<Vec<_>>().join(",");
        let sql = format!(
            "INSERT INTO mdoc_weak_component (fnode, component_size) VALUES {placeholders}"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .flat_map(|(f, s)| {
                [
                    f as &dyn rusqlite::types::ToSql,
                    s as &dyn rusqlite::types::ToSql,
                ]
            })
            .collect();
        conn.execute(&sql, params.as_slice())?;
    }
    Ok(())
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

pub(crate) fn valid_node_rows(conn: &Connection) -> Result<Vec<(String, String, String)>> {
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

pub(crate) fn node_lookup(conn: &Connection) -> Result<HashMap<String, (String, String)>> {
    Ok(valid_node_rows(conn)?
        .into_iter()
        .map(|(f, t, p)| (f, (t, p)))
        .collect())
}

pub(crate) fn issue_lookup(conn: &Connection) -> Result<HashMap<String, GraphIssue>> {
    let mut map: HashMap<String, GraphIssue> = HashMap::new();
    for issue in invalid_issue_rows(conn)? {
        map.entry(issue.fnode.clone()).or_insert(issue);
    }
    for issue in missing_issue_rows(conn)? {
        map.entry(issue.fnode.clone()).or_insert(issue);
    }
    Ok(map)
}

pub(crate) fn invalid_issue_rows(conn: &Connection) -> Result<Vec<GraphIssue>> {
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

pub(crate) fn missing_issue_rows(conn: &Connection) -> Result<Vec<GraphIssue>> {
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
