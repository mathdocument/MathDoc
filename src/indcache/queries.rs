use anyhow::{bail, Result};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet, VecDeque};

pub(super) const CHUNK_SIZE: usize = 500;

use crate::core::{
    representative_cycles, weak_component_sizes, DependencyCandidates, DependencyCandidatesEmpty,
    DependencyItem, DependencyTraversalReport, FormalCodeStatus, FormalizationStatus,
    GraphCheckReport, GraphIssue, GraphRootItem, IssueKind, NodeDegrees, NodeSummary,
};

// ── Public query functions ──────────────────────────────────────────────────

pub(super) fn issue_for_fnode(conn: &Connection, fnode: &str) -> Result<Option<GraphIssue>> {
    Ok(issue_lookup_for_fnodes(conn, &[fnode])?.remove(fnode))
}

pub(super) fn ref_item_for_fnode(
    conn: &Connection,
    fnode: &str,
    depth: u32,
) -> Result<DependencyItem> {
    Ok(ref_items_for_fnodes(conn, &[fnode], depth)?
        .pop()
        .expect("one requested reference item"))
}

pub(super) fn ref_items_for_fnodes(
    conn: &Connection,
    fnodes: &[&str],
    depth: u32,
) -> Result<Vec<DependencyItem>> {
    let nodes = node_lookup_for_fnodes(conn, fnodes)?;
    let issues = issue_lookup_for_fnodes(conn, fnodes)?;
    Ok(fnodes
        .iter()
        .map(|fnode| dependency_item(fnode, depth, &nodes, &issues))
        .collect())
}

pub(super) fn referrer_items(
    conn: &Connection,
    target_fnode: &str,
    depth: i32,
) -> Result<Vec<DependencyItem>> {
    let _profile = crate::profile::scope("queries::referrer_items");
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    if depth == 0 {
        return Ok(Vec::new());
    }
    let max_depth = (depth != -1).then_some(depth as u32);
    let reached = reverse_bfs(target_fnode, &reverse_graph(conn)?, max_depth);
    let fnodes = reached
        .iter()
        .map(|(fnode, _)| fnode.as_str())
        .collect::<Vec<_>>();
    let nodes = node_lookup_for_fnodes(conn, &fnodes)?;
    let issues = issue_lookup_for_fnodes(conn, &fnodes)?;
    Ok(reached
        .into_iter()
        .map(|(fnode, item_depth)| dependency_item(&fnode, item_depth, &nodes, &issues))
        .collect())
}

/// BFS reachability check on `mdoc_edges`. Returns true if `to_fnode` is reachable from
/// `from_fnode` (including the trivial case where they are equal).
pub(super) fn is_reachable(conn: &Connection, from_fnode: &str, to_fnode: &str) -> Result<bool> {
    let _profile = crate::profile::scope("queries::is_reachable");
    Ok(from_fnode == to_fnode
        || reverse_bfs(to_fnode, &reverse_graph(conn)?, None)
            .iter()
            .any(|(fnode, _)| fnode == from_fnode))
}

pub(super) fn reverse_reachable_fnodes(
    conn: &Connection,
    target_fnode: &str,
) -> Result<HashSet<String>> {
    let _profile = crate::profile::scope("queries::reverse_reachable_fnodes");
    let mut reached = reverse_bfs(target_fnode, &reverse_graph(conn)?, None)
        .into_iter()
        .map(|(fnode, _)| fnode)
        .collect::<HashSet<_>>();
    reached.insert(target_fnode.to_string());
    Ok(reached)
}

pub(super) fn node_summary(conn: &Connection, fnode: &str) -> Result<NodeSummary> {
    let sql = format!(
        "SELECT {NODE_SUMMARY_COLUMNS_SQL}
         FROM mdocs m
         WHERE m.fnode = ?
         ORDER BY m.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map([fnode], node_summary_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() == 1 {
        return Ok(rows.pop().expect("one summary row"));
    }
    if let Some(issue) = issue_lookup_for_fnodes(conn, &[fnode])?.remove(fnode) {
        return Ok(NodeSummary {
            fnode: issue.fnode,
            title: issue.title,
            rel_path: issue.rel_path,
            broken: true,
            depth: 0,
        });
    }
    if rows.is_empty() {
        bail!("no mdoc matched reference: {fnode}");
    }
    bail!("duplicate fnode: {fnode}")
}

pub(super) fn node_degrees(conn: &Connection, fnode: &str) -> Result<NodeDegrees> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(id.in_degree, 0),
                (SELECT COUNT(*) FROM mdoc_valid_edges e WHERE e.src_fnode = m.fnode)
         FROM mdocs m
         LEFT JOIN mdoc_in_degree id ON id.fnode = m.fnode
         WHERE m.fnode = ?
           AND NOT EXISTS (
             SELECT 1 FROM mdoc_issues i
             WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
           )
         ORDER BY m.path",
    )?;
    let rows: Vec<(i64, i64)> = stmt
        .query_map([fnode], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let [(in_degree, out_degree)] = rows.as_slice() else {
        bail!("metric source must be one valid, uniquely indexed node: {fnode}");
    };
    Ok(NodeDegrees {
        in_degree: u32::try_from(*in_degree)?,
        out_degree: u32::try_from(*out_degree)?,
    })
}

pub(super) fn formalization_status(conn: &Connection, fnode: &str) -> Result<FormalizationStatus> {
    let mut stmt = conn.prepare(
        "SELECT f.lean_status, f.rocq_status
         FROM mdocs m
         JOIN mdoc_files f ON f.path = m.path
         WHERE m.fnode = ?
         ORDER BY m.path",
    )?;
    let rows = stmt
        .query_map([fnode], |row| {
            Ok(FormalizationStatus {
                lean: decode_formal_status(row.get(0)?)?,
                rocq: decode_formal_status(row.get(1)?)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let [status] = rows.as_slice() else {
        bail!("formal status source must be one indexed node: {fnode}");
    };
    Ok(*status)
}

fn decode_formal_status(value: i64) -> rusqlite::Result<FormalCodeStatus> {
    match value {
        0 => Ok(FormalCodeStatus::NoCode),
        1 => Ok(FormalCodeStatus::Unverified),
        2 => Ok(FormalCodeStatus::Verified),
        _ => Err(rusqlite::Error::IntegralValueOutOfRange(0, value)),
    }
}

/// Direct referrers with all list metadata in one query.
/// Referrers whose own file is invalid/duplicate are excluded.
pub(super) fn direct_referrer_summaries(
    conn: &Connection,
    target_fnode: &str,
) -> Result<Vec<NodeSummary>> {
    let sql = format!(
        "SELECT {NODE_SUMMARY_COLUMNS_SQL}
         FROM mdoc_valid_edges e
         JOIN mdocs m ON m.fnode = e.src_fnode
         WHERE e.dst_fnode = ?
         GROUP BY e.src_fnode
         ORDER BY m.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([target_fnode], node_summary_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub(super) fn dependency_report(
    conn: &Connection,
    root_fnode: &str,
    depth: i32,
) -> Result<DependencyTraversalReport> {
    if depth < -1 {
        bail!("depth must be -1 (infinite) or >= 0");
    }
    dependency_report_inner(conn, root_fnode, depth, false)
}

pub(super) fn leaf_dependency_report(
    conn: &Connection,
    root_fnode: &str,
) -> Result<DependencyTraversalReport> {
    dependency_report_inner(conn, root_fnode, -1, true)
}

pub(super) fn direct_dependency_summaries(
    conn: &Connection,
    root_fnode: &str,
) -> Result<Vec<NodeSummary>> {
    let root_nodes = node_lookup_for_fnodes(conn, &[root_fnode])?;
    let root_issues = issue_lookup_for_fnodes(conn, &[root_fnode])?;
    if let Some(issue) = root_issues.get(root_fnode) {
        bail!("{}", issue.error);
    }
    if !root_nodes.contains_key(root_fnode) {
        bail!("no mdoc matched reference: {root_fnode}");
    }

    let mut seen = HashSet::from([root_fnode.to_string()]);
    let dependencies = edge_lookup_for_sources(conn, &[root_fnode])?
        .remove(root_fnode)
        .unwrap_or_default()
        .into_iter()
        .filter(|fnode| seen.insert(fnode.clone()))
        .collect::<Vec<_>>();
    let fnodes = dependencies.iter().map(String::as_str).collect::<Vec<_>>();
    let nodes = node_lookup_for_fnodes(conn, &fnodes)?;
    let issues = issue_lookup_for_fnodes(conn, &fnodes)?;
    let items = dependencies
        .into_iter()
        .map(|fnode| dependency_item(&fnode, 1, &nodes, &issues))
        .collect();
    summaries_for_items(conn, items, &issues)
}

pub(super) fn global_root_items(conn: &Connection) -> Result<Vec<GraphRootItem>> {
    let _profile = crate::profile::scope("queries::global_root_items");
    let valid_nodes = valid_node_rows(conn)?;
    let invalid_issues = invalid_issue_rows(conn)?;
    let graph = dep_graph_snapshot(conn)?;
    let component_members = valid_nodes
        .iter()
        .map(|(fnode, _, _)| fnode.clone())
        .chain(
            invalid_issues
                .iter()
                .filter(|issue| !is_placeholder(&issue.fnode))
                .map(|issue| issue.fnode.clone()),
        )
        .collect::<HashSet<_>>();
    let component_sizes = weak_component_sizes(&graph, &component_members);

    let valid_roots: Vec<(String, String, String, u32)> = {
        let mut stmt = conn.prepare(
            "SELECT m.fnode, m.title, m.path, m.topo_depth
             FROM mdocs m
             LEFT JOIN mdoc_in_degree id ON m.fnode = id.fnode
             WHERE (id.in_degree IS NULL OR id.in_degree = 0)
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues
                 WHERE mdoc_issues.path = m.path
                   AND mdoc_issues.kind IN ('invalid', 'duplicate')
               )",
        )?;
        let rows: Vec<_> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };

    let mut items: Vec<GraphRootItem> = valid_roots
        .into_iter()
        .map(|(fnode, title, path, topo_depth)| {
            let component_size = component_sizes.get(&fnode).copied().unwrap_or(1);
            GraphRootItem {
                fnode,
                title,
                rel_path: path,
                component_size,
                broken: false,
                topo_depth,
            }
        })
        .collect();

    let mut stmt = conn.prepare(
        "SELECT issue.ref_fnode, issue.path
         FROM mdoc_issues issue
         LEFT JOIN mdoc_in_degree degree ON degree.fnode = issue.ref_fnode
         WHERE issue.kind IN ('invalid', 'duplicate')
           AND NOT EXISTS (
             SELECT 1 FROM mdocs claimant
             WHERE claimant.fnode = issue.ref_fnode
               AND NOT EXISTS (
                 SELECT 1 FROM mdoc_issues blocking
                 WHERE blocking.path = claimant.path
                   AND blocking.kind IN ('invalid', 'duplicate')
               )
           )
           AND (
             (substr(issue.ref_fnode, 1, 1) = '<'
              AND substr(issue.ref_fnode, -1, 1) = '>')
             OR COALESCE(degree.in_degree, 0) = 0
           )
         ORDER BY issue.path,
                  CASE WHEN issue.kind = 'invalid' THEN 0 ELSE 1 END,
                  issue.ref_fnode, issue.error",
    )?;
    let mut broken_paths = HashSet::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (fnode, rel_path) = row?;
        if !broken_paths.insert(rel_path.clone()) {
            continue;
        }
        let component_size = component_sizes.get(&fnode).copied().unwrap_or(1);
        items.push(GraphRootItem {
            fnode,
            title: "<invalid>".to_string(),
            rel_path,
            component_size,
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

pub(super) fn graph_check_report(conn: &Connection) -> Result<GraphCheckReport> {
    let nodes: i64 = conn.query_row("SELECT COUNT(*) FROM mdoc_files", [], |r| r.get(0))?;
    let edges: i64 = conn.query_row(
        "SELECT COALESCE(SUM(in_degree), 0) FROM mdoc_in_degree",
        [],
        |row| row.get(0),
    )?;

    Ok(GraphCheckReport {
        nodes: nodes as u32,
        edges: edges as u32,
        missing: missing_issue_rows(conn)?,
        invalid: invalid_issue_rows(conn)?,
        cycles: representative_cycles(&dep_graph_snapshot(conn)?),
    })
}

// ── Private helpers ─────────────────────────────────────────────────────────

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

    let cycles = representative_cycles(&report_graph);

    let issues_in_graph: HashMap<String, GraphIssue> = issues
        .into_iter()
        .filter(|(f, _)| report_graph.contains_key(f.as_str()))
        .collect();

    Ok(DependencyTraversalReport {
        items,
        issues_by_fnode: issues_in_graph,
        cycles,
    })
}

// ── Low-level data accessors ─────────────────────────────────────────────────

pub(super) fn valid_node_rows(conn: &Connection) -> Result<Vec<(String, String, String)>> {
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

pub(super) fn invalid_issue_rows(conn: &Connection) -> Result<Vec<GraphIssue>> {
    let mut stmt = conn.prepare(
        "SELECT path, ref_fnode, error FROM mdoc_issues
         WHERE kind IN ('invalid', 'duplicate')
         ORDER BY path,
                  CASE WHEN kind = 'invalid' THEN 0 ELSE 1 END,
                  ref_fnode, error",
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
        "SELECT ref_fnode, error FROM mdoc_missing_issues
         ORDER BY ref_fnode",
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

pub(super) fn dep_graph_snapshot(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT mdocs.fnode
         FROM mdocs
         WHERE NOT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE mdoc_issues.path = mdocs.path
               AND mdoc_issues.kind IN ('invalid', 'duplicate')
         )",
    )?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        graph.entry(row?).or_default();
    }
    let mut stmt = conn.prepare(
        "SELECT ref_fnode FROM mdoc_issues
         WHERE kind IN ('invalid', 'duplicate')",
    )?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let fnode = row?;
        if !is_placeholder(&fnode) {
            graph.entry(fnode).or_default();
        }
    }

    let mut stmt =
        conn.prepare("SELECT src_fnode, dst_fnode FROM mdoc_valid_edges ORDER BY src_path, ord")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (src, dst) = row?;
        graph.entry(src.clone()).or_default().push(dst.clone());
        graph.entry(dst).or_default();
    }
    Ok(graph)
}

fn is_placeholder(fnode: &str) -> bool {
    fnode.starts_with('<') && fnode.ends_with('>')
}

fn reverse_graph(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let _profile = crate::profile::scope("queries::reverse_graph");
    let mut reverse = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT src_fnode, dst_fnode
         FROM mdoc_valid_edges
         ORDER BY src_path, ord",
    )?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (source, target) = row?;
        reverse.entry(target).or_insert_with(Vec::new).push(source);
    }
    Ok(reverse)
}

fn reverse_bfs(
    target_fnode: &str,
    reverse: &HashMap<String, Vec<String>>,
    max_depth: Option<u32>,
) -> Vec<(String, u32)> {
    let mut reached = Vec::new();
    let mut seen = HashSet::from([target_fnode.to_string()]);
    let mut queue = VecDeque::from([(target_fnode.to_string(), 0)]);
    while let Some((fnode, item_depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max_depth| item_depth >= max_depth) {
            continue;
        }
        for referrer in reverse.get(&fnode).into_iter().flatten() {
            if seen.insert(referrer.clone()) {
                reached.push((referrer.clone(), item_depth + 1));
                queue.push_back((referrer.clone(), item_depth + 1));
            }
        }
    }
    reached
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
            "WITH issue_rows AS (
               SELECT path, kind, ref_fnode, error FROM mdoc_issues
               WHERE kind IN ('invalid', 'duplicate')
               UNION ALL
               SELECT path, kind, ref_fnode, error FROM mdoc_missing_issues
             )
             SELECT path, kind, ref_fnode, error FROM issue_rows
             WHERE ref_fnode IN ({placeholders})
                AND NOT EXISTS (
                  SELECT 1 FROM mdocs AS valid
                  WHERE valid.fnode = issue_rows.ref_fnode
                    AND NOT EXISTS (
                      SELECT 1 FROM mdoc_issues AS blocking
                      WHERE blocking.path = valid.path
                        AND blocking.kind IN ('invalid', 'duplicate')
                    )
                )
               ORDER BY CASE WHEN kind IN ('invalid', 'duplicate') THEN 0 ELSE 1 END,
                        path, ref_fnode, error"
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
            "SELECT src_fnode, dst_fnode, ord FROM mdoc_valid_edges
             WHERE src_fnode IN ({placeholders})
            "
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
    if let Some((title, path)) = nodes.get(fnode) {
        return DependencyItem {
            depth,
            fnode: fnode.to_string(),
            title: title.clone(),
            rel_path: path.clone(),
        };
    }
    if let Some(issue) = issues.get(fnode) {
        return DependencyItem {
            depth,
            fnode: issue.fnode.clone(),
            title: issue.title.clone(),
            rel_path: issue.rel_path.clone(),
        };
    }
    DependencyItem {
        depth,
        fnode: fnode.to_string(),
        title: "<missing>".to_string(),
        rel_path: "<unknown>".to_string(),
    }
}

fn summaries_for_items(
    conn: &Connection,
    items: Vec<DependencyItem>,
    issues: &HashMap<String, GraphIssue>,
) -> Result<Vec<NodeSummary>> {
    let fnodes: Vec<&str> = items.iter().map(|item| item.fnode.as_str()).collect();
    let depths = topo_depth_lookup_for_fnodes(conn, &fnodes)?;
    Ok(items
        .into_iter()
        .map(|item| NodeSummary {
            broken: issues.contains_key(&item.fnode),
            depth: depths.get(&item.fnode).copied().unwrap_or(0),
            fnode: item.fnode,
            title: item.title,
            rel_path: item.rel_path,
        })
        .collect())
}

fn topo_depth_lookup_for_fnodes(
    conn: &Connection,
    fnodes: &[&str],
) -> Result<HashMap<String, u32>> {
    let mut result = HashMap::new();
    for chunk in fnodes.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT fnode, topo_depth FROM mdocs WHERE fnode IN ({placeholders})");
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|fnode| fnode as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        for row in stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })? {
            let (fnode, depth) = row?;
            result.insert(fnode, depth);
        }
    }
    Ok(result)
}

// ── Helper to resolve a path reference from the DB ──────────────────────────

pub(super) fn lookup_by_fnode(
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

pub(super) fn resolve_ref_by_path(
    conn: &Connection,
    rel_path: &str,
) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT fnode, title FROM mdocs WHERE path = ?",
            [rel_path],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?)
}

const SEARCH_MATCH_SQL: &str = "m.title_lc LIKE ? ESCAPE '\\' OR m.fnode_lc LIKE ? ESCAPE '\\'";
const SEARCH_ORDER_SQL: &str = "CASE WHEN m.fnode_lc LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END,
     CASE WHEN instr(m.title_lc, ?) > 0 THEN instr(m.title_lc, ?) ELSE 999999 END,
     length(m.title),
     m.path";
const NODE_SUMMARY_COLUMNS_SQL: &str = "m.fnode, m.title, m.path,
     EXISTS(
       SELECT 1 FROM mdoc_issues i
       WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
     ),
     m.topo_depth";
const EXISTING_DEPENDENCIES_SQL: &str = "
    SELECT dst.fnode
    FROM mdoc_symbols src
    JOIN mdoc_edges e ON e.src_symbol_id = src.id
    JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
    WHERE src.fnode = ?
      AND NOT EXISTS (
          SELECT 1 FROM mdoc_issues i
          WHERE i.path = e.src_path AND i.kind IN ('invalid', 'duplicate')
      )";

fn search_patterns(query: &str) -> (String, String, String) {
    let query_lc = query.to_lowercase();
    let escaped = escape_like_pattern(&query_lc);
    let like = format!("%{escaped}%");
    let prefix_like = format!("{escaped}%");
    (query_lc, like, prefix_like)
}

fn fts5_query(query_lc: &str) -> Option<String> {
    if query_lc.contains('\0') {
        return None;
    }
    let chars: Vec<char> = query_lc.chars().collect();
    if chars.len() < 3 {
        return None;
    }

    let mut seen = HashSet::new();
    let terms: Vec<String> = chars
        .windows(3)
        .filter_map(|window| {
            let trigram: String = window.iter().collect();
            seen.insert(trigram.clone()).then_some(trigram)
        })
        .collect();
    let expression = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    Some(expression)
}

fn search_index_predicate(fts_query: Option<&String>) -> &'static str {
    if fts_query.is_some() {
        "m.id IN (
             SELECT rowid FROM mdoc_search
             WHERE mdoc_search MATCH ?
         ) AND"
    } else {
        ""
    }
}

fn node_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeSummary> {
    Ok(NodeSummary {
        fnode: row.get(0)?,
        title: row.get(1)?,
        rel_path: row.get(2)?,
        broken: row.get(3)?,
        depth: row.get(4)?,
    })
}

pub(super) fn search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<NodeSummary>> {
    let _profile = crate::profile::scope("queries::search");
    let (query_lc, like_pattern, prefix_pattern) = search_patterns(query);
    let fts_query = fts5_query(&query_lc);
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let sql = format!(
        "SELECT {NODE_SUMMARY_COLUMNS_SQL}
         FROM mdocs m
         WHERE {} {SEARCH_MATCH_SQL}
         ORDER BY {SEARCH_ORDER_SQL}
         LIMIT ?",
        search_index_predicate(fts_query.as_ref())
    );
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(7);
    if let Some(fts_query) = &fts_query {
        params.push(fts_query);
    }
    params.push(&like_pattern);
    params.push(&like_pattern);
    params.push(&prefix_pattern);
    params.push(&query_lc);
    params.push(&query_lc);
    params.push(&limit);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params.as_slice(), node_summary_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub(super) fn all_node_summaries(conn: &Connection) -> Result<Vec<NodeSummary>> {
    let sql = format!(
        "SELECT {NODE_SUMMARY_COLUMNS_SQL}
         FROM mdocs m
         ORDER BY m.path, m.fnode"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], node_summary_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub(super) fn dependency_candidates(
    conn: &Connection,
    source_fnode: &str,
    query: &str,
    limit: usize,
) -> Result<DependencyCandidates> {
    let _profile = crate::profile::scope("queries::dependency_candidates");
    let (query_lc, like_pattern, prefix_pattern) = search_patterns(query);
    let fts_query = fts5_query(&query_lc);
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let sql = format!(
        "SELECT {NODE_SUMMARY_COLUMNS_SQL}
         FROM mdocs m
         WHERE {} ({SEARCH_MATCH_SQL})
            AND m.fnode != ?
            AND m.fnode NOT IN ({EXISTING_DEPENDENCIES_SQL})
            AND NOT EXISTS (
               SELECT 1 FROM mdoc_issues i
               WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
           )
         ORDER BY {SEARCH_ORDER_SQL}
         LIMIT ?",
        search_index_predicate(fts_query.as_ref())
    );
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(9);
    if let Some(fts_query) = &fts_query {
        params.push(fts_query);
    }
    params.push(&like_pattern);
    params.push(&like_pattern);
    params.push(&source_fnode);
    params.push(&source_fnode);
    params.push(&prefix_pattern);
    params.push(&query_lc);
    params.push(&query_lc);
    params.push(&limit);
    let mut stmt = conn.prepare(&sql)?;
    let nodes: Vec<NodeSummary> = stmt
        .query_map(params.as_slice(), node_summary_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let empty = if nodes.is_empty() {
        Some(dependency_candidates_empty(
            conn,
            source_fnode,
            &query_lc,
            &like_pattern,
        )?)
    } else {
        None
    };
    Ok(DependencyCandidates { nodes, empty })
}

fn dependency_candidates_empty(
    conn: &Connection,
    source_fnode: &str,
    query_lc: &str,
    like_pattern: &str,
) -> Result<DependencyCandidatesEmpty> {
    let fts_query = fts5_query(query_lc);
    let sql = format!(
        "WITH matching(reason) AS (
             SELECT CASE
                 WHEN m.fnode = ? THEN 'source'
                 WHEN m.fnode IN ({EXISTING_DEPENDENCIES_SQL}) THEN 'existing'
                 WHEN EXISTS (
                     SELECT 1 FROM mdoc_issues i
                     WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
                 ) THEN 'invalid'
                 ELSE 'available'
             END
             FROM mdocs m
             WHERE {} {SEARCH_MATCH_SQL}
         )
         SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN reason = 'source' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN reason = 'existing' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN reason = 'invalid' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN reason = 'available' THEN 1 ELSE 0 END), 0)
         FROM matching",
        search_index_predicate(fts_query.as_ref())
    );
    let (total, source, existing_dependencies, invalid_or_duplicate, available): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = {
        let mut params: Vec<&dyn rusqlite::types::ToSql> =
            Vec::with_capacity(if fts_query.is_some() { 5 } else { 4 });
        params.push(&source_fnode);
        params.push(&source_fnode);
        if let Some(fts_query) = &fts_query {
            params.push(fts_query);
        }
        params.push(&like_pattern);
        params.push(&like_pattern);
        conn.query_row(&sql, params.as_slice(), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
    };
    let as_usize = |count| usize::try_from(count).unwrap_or(usize::MAX);

    if total == 0 {
        Ok(DependencyCandidatesEmpty::NoMatch)
    } else if available > 0 {
        Ok(DependencyCandidatesEmpty::ResultLimit {
            available: as_usize(available),
        })
    } else {
        Ok(DependencyCandidatesEmpty::Excluded {
            source: as_usize(source),
            existing_dependencies: as_usize(existing_dependencies),
            invalid_or_duplicate: as_usize(invalid_or_duplicate),
        })
    }
}

pub(super) fn exact_fnode_rows(
    conn: &Connection,
    fnode: &str,
) -> Result<Vec<(String, String, String)>> {
    let fnode_lc = fnode.to_lowercase();
    let mut stmt =
        conn.prepare("SELECT fnode, title, path FROM mdocs WHERE fnode_lc = ? ORDER BY path")?;
    let rows = stmt
        .query_map([fnode_lc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub(super) fn is_bootstrapped(conn: &Connection) -> Result<bool> {
    let row: i32 = conn.query_row(
        "SELECT bootstrapped FROM mdoc_index_state WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(row != 0)
}

pub(super) fn fnode_for_path(conn: &Connection, rel_path: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT fnode FROM mdocs WHERE path = ?", [rel_path], |r| {
            r.get::<_, String>(0)
        })
        .optional()?)
}

pub(super) fn resolve_fnode_ref(
    conn: &Connection,
    raw_ref: &str,
) -> Result<Option<Vec<(String, String, String)>>> {
    let query_lc = raw_ref.to_lowercase();
    let prefix_like = format!("{}%", escape_like_pattern(&query_lc));
    let mut stmt = conn.prepare(
        "SELECT fnode, title, path FROM mdocs
         WHERE fnode_lc = ? OR fnode_lc LIKE ? ESCAPE '\\'
         ORDER BY CASE WHEN fnode_lc = ? THEN 0 ELSE 1 END, path",
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

pub(super) fn exact_title_rows(
    conn: &Connection,
    title: &str,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT fnode, title, path FROM mdocs
         WHERE title_lc = ?
         ORDER BY path, fnode",
    )?;
    let rows = stmt
        .query_map([title.trim().to_lowercase()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(super) fn mdoc_count(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM mdocs", [], |r| r.get::<_, i64>(0))? as u32)
}

pub(super) fn path_has_blocking_issue(conn: &Connection, rel_path: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM mdoc_issues
             WHERE path = ? AND kind IN ('invalid', 'duplicate')
         )",
        [rel_path],
        |row| row.get(0),
    )?)
}

pub(super) fn edge_targets_for_source_path(
    conn: &Connection,
    src_path: &str,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT dst.fnode
         FROM mdoc_edges e
         JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
         WHERE e.src_path = ?
         ORDER BY e.ord",
    )?;
    let rows = stmt
        .query_map([src_path], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub(super) fn path_for_fnode_if_unique(conn: &Connection, fnode: &str) -> Result<Option<String>> {
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

/// All dependency edges from non-blocking source documents, as `(src_fnode, dst_fnode)`.
/// Used by `mdc serve`'s force-graph view to render the full workspace graph.
pub(super) fn all_valid_edges(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT src_fnode, dst_fnode
         FROM mdoc_valid_edges
         ORDER BY src_fnode, ord",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_and_exists_queries_propagate_schema_errors() {
        let conn = Connection::open_in_memory().unwrap();

        assert!(resolve_ref_by_path(&conn, "missing.mdoc").is_err());
        assert!(fnode_for_path(&conn, "missing.mdoc").is_err());
        assert!(path_has_blocking_issue(&conn, "missing.mdoc").is_err());
    }

    #[test]
    fn fts5_queries_quote_user_input_and_skip_short_terms() {
        assert_eq!(fts5_query("ab"), None);
        assert_eq!(fts5_query("a\"b"), Some("\"a\"\"b\"".to_string()));
        assert_eq!(fts5_query("ababa"), Some("\"aba\" AND \"bab\"".to_string()));
    }
}
