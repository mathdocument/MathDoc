import json
import sqlite3
from collections import deque
from contextlib import AbstractContextManager
from typing import Protocol

from ..core import (
    DependencyCycleError,
    DependencyItem,
    DependencyTraversalReport,
    GraphCheckReport,
    GraphIssue,
    GraphRootItem,
    component_has_cycle,
    find_cycle,
    representative_cycle,
    strongly_connected_components,
)


class CacheQueryProtocol(Protocol):
    def _open_conn(self) -> AbstractContextManager[sqlite3.Connection]: ...


def issue_for_fnode(cache: CacheQueryProtocol, fnode: str) -> GraphIssue | None:
    with cache._open_conn() as conn:
        return _issue_for_fnode(conn, fnode)


def ref_item_for_fnode(
    cache: CacheQueryProtocol,
    fnode: str,
    *,
    depth: int = 0,
) -> DependencyItem:
    with cache._open_conn() as conn:
        node_lookup = _node_lookup(conn)
        issue_lookup = _issue_lookup(conn)
        return _dependency_item_for_fnode(
            conn,
            fnode=fnode,
            depth=depth,
            node_lookup=node_lookup,
            issue_lookup=issue_lookup,
        )


def referrer_items(
    cache: CacheQueryProtocol,
    *,
    target_fnode: str,
    depth: int,
) -> list[DependencyItem]:
    if depth < -1:
        raise ValueError("depth must be -1 (infinite) or >= 0")

    with cache._open_conn() as conn:
        reverse_graph = _reverse_graph_snapshot(conn)
        node_lookup = _node_lookup(conn)
        issue_lookup = _issue_lookup(conn)
        items: list[DependencyItem] = []
        seen: set[str] = {target_fnode}
        queue: deque[tuple[str, int]] = deque(
            (ref_fnode, 1) for ref_fnode in reverse_graph.get(target_fnode, [])
        )

        while queue:
            fnode, item_depth = queue.popleft()
            if fnode in seen:
                continue
            seen.add(fnode)
            items.append(
                _dependency_item_for_fnode(
                    conn,
                    fnode=fnode,
                    depth=item_depth,
                    node_lookup=node_lookup,
                    issue_lookup=issue_lookup,
                )
            )

            if depth != -1 and item_depth >= depth:
                continue
            for ref_fnode in reverse_graph.get(fnode, []):
                if ref_fnode == target_fnode:
                    continue
                queue.append((ref_fnode, item_depth + 1))

    return items


def dependency_report(
    cache: CacheQueryProtocol,
    *,
    root_fnode: str,
    depth: int,
) -> DependencyTraversalReport:
    if depth < -1:
        raise ValueError("depth must be -1 (infinite) or >= 0")

    with cache._open_conn() as conn:
        return _dependency_report_for_root(
            conn,
            root_fnode=root_fnode,
            depth=depth,
            leaf_only=False,
        )


def leaf_dependency_report(
    cache: CacheQueryProtocol,
    *,
    root_fnode: str,
) -> DependencyTraversalReport:
    with cache._open_conn() as conn:
        return _dependency_report_for_root(
            conn,
            root_fnode=root_fnode,
            depth=-1,
            leaf_only=True,
        )


def global_root_items(cache: CacheQueryProtocol) -> list[GraphRootItem]:
    with cache._open_conn() as conn:
        state_row = conn.execute(
            "SELECT weak_component_dirty FROM mdoc_index_state WHERE id = 1"
        ).fetchone()
        if state_row is None or int(state_row[0]) == 1:
            _recompute_weak_components(conn)
            conn.execute(
                "UPDATE mdoc_index_state SET weak_component_dirty = 0 WHERE id = 1"
            )

        valid_root_rows = conn.execute(
            """
            SELECT mdocs.fnode, mdocs.title, mdocs.path
            FROM mdocs
            LEFT JOIN mdoc_in_degree ON mdocs.fnode = mdoc_in_degree.fnode
            WHERE (mdoc_in_degree.in_degree IS NULL OR mdoc_in_degree.in_degree = 0)
              AND NOT EXISTS (
                SELECT 1 FROM mdoc_issues
                WHERE mdoc_issues.path = mdocs.path
                  AND mdoc_issues.kind IN ('invalid', 'duplicate')
              )
            ORDER BY mdocs.path, mdocs.fnode
            """
        ).fetchall()

        invalid_issues = _invalid_issue_rows(conn)

        incoming_fnodes = {
            str(row[0])
            for row in conn.execute(
                "SELECT fnode FROM mdoc_in_degree WHERE in_degree > 0"
            ).fetchall()
        }

        component_sizes = {
            str(row[0]): int(row[1])
            for row in conn.execute(
                "SELECT fnode, component_size FROM mdoc_weak_component"
            ).fetchall()
        }

        items: list[GraphRootItem] = []

        for fnode, title, path in [
            (str(r[0]), str(r[1]), str(r[2])) for r in valid_root_rows
        ]:
            items.append(
                GraphRootItem(
                    fnode=fnode,
                    title=title,
                    rel_path=path,
                    component_size=component_sizes.get(fnode, 1),
                    broken=False,
                )
            )

        for issue in invalid_issues:
            if (
                not (issue.fnode.startswith("<") and issue.fnode.endswith(">"))
                and issue.fnode in incoming_fnodes
            ):
                continue
            items.append(
                GraphRootItem(
                    fnode=issue.fnode,
                    title=issue.title,
                    rel_path=issue.rel_path,
                    component_size=component_sizes.get(issue.fnode, 1),
                    broken=True,
                )
            )

    items.sort(
        key=lambda item: (
            -item.component_size,
            item.rel_path,
            item.title,
            item.fnode,
        )
    )
    return items


def graph_check_report(cache: CacheQueryProtocol) -> GraphCheckReport:
    with cache._open_conn() as conn:
        state_row = conn.execute(
            "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1"
        ).fetchone()
        current_epoch = int(state_row[0]) if state_row is not None else 0

        scc_row = conn.execute(
            "SELECT graph_epoch, cycles_json FROM mdoc_scc_result WHERE id = 1"
        ).fetchone()

        if scc_row is not None and int(scc_row[0]) == current_epoch:
            cycles = json.loads(str(scc_row[1]))
        else:
            dep_graph = _dep_graph_snapshot(conn)
            cycles = []
            for component in strongly_connected_components(dep_graph):
                if not component_has_cycle(dep_graph, component):
                    continue
                cycle = representative_cycle(dep_graph, component)
                if cycle is not None:
                    cycles.append(cycle)
            cycles.sort(key=lambda cycle: tuple(cycle))

            conn.execute(
                """
                INSERT INTO mdoc_scc_result (id, graph_epoch, cycles_json)
                VALUES (1, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    graph_epoch = excluded.graph_epoch,
                    cycles_json = excluded.cycles_json
                """,
                (current_epoch, json.dumps(cycles)),
            )

        nodes = conn.execute("SELECT COUNT(*) FROM mdoc_files").fetchone()[0]
        edges = conn.execute(
            """
            SELECT COUNT(*)
            FROM mdoc_edges
            WHERE NOT EXISTS (
                SELECT 1
                FROM mdoc_issues
                WHERE mdoc_issues.path = mdoc_edges.src_path
                  AND mdoc_issues.kind IN ('invalid', 'duplicate')
            )
            """
        ).fetchone()[0]
        return GraphCheckReport(
            nodes=int(nodes),
            edges=int(edges),
            missing=_missing_issue_rows(conn),
            invalid=_invalid_issue_rows(conn),
            cycles=cycles,
        )


def _recompute_weak_components(conn: sqlite3.Connection) -> None:
    """BFS to compute weak connected components; stores results in mdoc_weak_component."""
    valid_node_rows = _valid_node_rows(conn)
    invalid_issues = _invalid_issue_rows(conn)
    dep_graph = _dep_graph_snapshot(
        conn,
        valid_node_rows=valid_node_rows,
        invalid_issues=invalid_issues,
    )

    component_members: set[str] = {fnode for fnode, _, _ in valid_node_rows}
    component_members.update(
        issue.fnode
        for issue in invalid_issues
        if not (issue.fnode.startswith("<") and issue.fnode.endswith(">"))
    )

    # Build undirected adjacency from the directed dep graph (treating edges as bidirectional
    # so that A→B and B→A end up in the same weak connected component).
    adjacency: dict[str, set[str]] = {}
    for src_fnode, dep_fnodes in dep_graph.items():
        if src_fnode not in component_members:
            continue
        adjacency.setdefault(src_fnode, set())
        for dep_fnode in dep_fnodes:
            if dep_fnode not in component_members:
                continue
            adjacency.setdefault(dep_fnode, set())
            adjacency[src_fnode].add(dep_fnode)
            adjacency[dep_fnode].add(src_fnode)

    sizes: dict[str, int] = {}
    seen: set[str] = set()
    for start_fnode in sorted(adjacency):
        if start_fnode in seen:
            continue
        queue: deque[str] = deque([start_fnode])
        component: list[str] = []
        while queue:
            fnode = queue.popleft()
            if fnode in seen:
                continue
            seen.add(fnode)
            component.append(fnode)
            for neighbor in adjacency.get(fnode, ()):
                if neighbor not in seen:
                    queue.append(neighbor)
        size = len(component)
        for fnode in component:
            sizes[fnode] = size

    conn.execute("DELETE FROM mdoc_weak_component")
    items = list(sizes.items())
    chunk_size = 500
    for start in range(0, len(items), chunk_size):
        chunk = items[start : start + chunk_size]
        placeholders = ",".join("(?,?)" for _ in chunk)
        flat = [v for fnode, size in chunk for v in (fnode, size)]
        conn.execute(
            f"INSERT INTO mdoc_weak_component (fnode, component_size) VALUES {placeholders}",
            flat,
        )


def _issue_for_fnode(
    conn: sqlite3.Connection,
    fnode: str,
    *,
    issue_lookup: dict[str, GraphIssue] | None = None,
) -> GraphIssue | None:
    issue_map = issue_lookup if issue_lookup is not None else _issue_lookup(conn)
    return issue_map.get(fnode)


def _dependency_report_for_root(
    conn: sqlite3.Connection,
    *,
    root_fnode: str,
    depth: int,
    leaf_only: bool,
) -> DependencyTraversalReport:
    node_lookup = _node_lookup_for_fnodes(conn, [root_fnode])
    issue_lookup = _issue_lookup_for_fnodes(conn, [root_fnode])

    root_issue = issue_lookup.get(root_fnode)
    if root_issue is not None:
        raise ValueError(root_issue.error)

    if root_fnode not in node_lookup:
        raise ValueError(f"no mdoc matched reference: {root_fnode}")

    report_graph: dict[str, list[str]] = {root_fnode: []}
    items: list[DependencyItem] = []
    discovered: set[str] = {root_fnode}
    queue: deque[tuple[str, int]] = deque([(root_fnode, 0)])

    while queue:
        batch: list[tuple[str, int]] = []
        while queue and len(batch) < 200:
            batch.append(queue.popleft())

        expandable = [
            src_fnode
            for src_fnode, src_depth in batch
            if leaf_only or depth == -1 or src_depth < depth
        ]
        edges_by_src = _edge_lookup_for_sources(conn, expandable)
        pending_nodes: list[tuple[str, int]] = []

        for fnode, item_depth in batch:
            if not leaf_only and depth != -1 and item_depth >= depth:
                report_graph[fnode] = []
                continue

            dep_fnodes = edges_by_src.get(fnode, [])
            report_graph[fnode] = dep_fnodes

            if leaf_only and fnode != root_fnode and not dep_fnodes:
                items.append(
                    _dependency_item_for_fnode(
                        conn,
                        fnode=fnode,
                        depth=item_depth,
                        node_lookup=node_lookup,
                        issue_lookup=issue_lookup,
                    )
                )
            for dep_fnode in dep_fnodes:
                if dep_fnode in discovered:
                    continue
                discovered.add(dep_fnode)
                pending_nodes.append((dep_fnode, item_depth + 1))

        if pending_nodes:
            pending_fnodes = [fnode for fnode, _ in pending_nodes]
            node_lookup.update(_node_lookup_for_fnodes(conn, pending_fnodes))
            issue_lookup.update(_issue_lookup_for_fnodes(conn, pending_fnodes))

            for fnode, item_depth in pending_nodes:
                if not leaf_only:
                    items.append(
                        _dependency_item_for_fnode(
                            conn,
                            fnode=fnode,
                            depth=item_depth,
                            node_lookup=node_lookup,
                            issue_lookup=issue_lookup,
                        )
                    )
                queue.append((fnode, item_depth))

    cycle = find_cycle(report_graph, root_fnode=root_fnode)
    if cycle is not None:
        raise DependencyCycleError(cycle)

    return DependencyTraversalReport(
        root_fnode=root_fnode,
        items=items,
        dep_graph=report_graph,
        issues_by_fnode={
            fnode: issue
            for fnode, issue in issue_lookup.items()
            if fnode in report_graph
        },
    )


def _node_lookup_for_fnodes(
    conn: sqlite3.Connection,
    fnodes: list[str],
) -> dict[str, tuple[str, str]]:
    if not fnodes:
        return {}

    rows_by_fnode: dict[str, tuple[str, str]] = {}
    chunk_size = 500
    for start in range(0, len(fnodes), chunk_size):
        chunk = fnodes[start : start + chunk_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = conn.execute(
            f"""
            SELECT mdocs.fnode, mdocs.title, mdocs.path
            FROM mdocs
            WHERE mdocs.fnode IN ({placeholders})
              AND NOT EXISTS (
                SELECT 1
                FROM mdoc_issues
                WHERE mdoc_issues.path = mdocs.path
                  AND mdoc_issues.kind IN ('invalid', 'duplicate')
              )
            """,
            tuple(chunk),
        ).fetchall()
        for row in rows:
            rows_by_fnode[str(row[0])] = (str(row[1]), str(row[2]))
    return rows_by_fnode


def _issue_lookup_for_fnodes(
    conn: sqlite3.Connection,
    fnodes: list[str],
) -> dict[str, GraphIssue]:
    if not fnodes:
        return {}

    issues_by_fnode: dict[str, GraphIssue] = {}
    chunk_size = 500
    for start in range(0, len(fnodes), chunk_size):
        chunk = fnodes[start : start + chunk_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = conn.execute(
            f"""
            SELECT path, kind, ref_fnode, error
            FROM mdoc_issues
            WHERE ref_fnode IN ({placeholders})
              AND (
                kind IN ('invalid', 'duplicate')
                OR (
                  kind = 'missing'
                  AND NOT EXISTS (
                    SELECT 1
                    FROM mdoc_issues AS src_issues
                    WHERE src_issues.path = mdoc_issues.path
                      AND src_issues.kind IN ('invalid', 'duplicate')
                  )
                )
              )
            ORDER BY path, ref_fnode, error
            """,
            tuple(chunk),
        ).fetchall()
        for row in rows:
            fnode = str(row[2])
            if fnode in issues_by_fnode:
                continue
            kind = str(row[1])
            if kind == "missing":
                issues_by_fnode[fnode] = GraphIssue(
                    kind="missing",
                    fnode=fnode,
                    title="<missing>",
                    rel_path="<unknown>",
                    error=str(row[3]),
                )
                continue
            issues_by_fnode[fnode] = GraphIssue(
                kind="invalid",
                fnode=fnode,
                title="<invalid>",
                rel_path=str(row[0]),
                error=str(row[3]),
            )
    return issues_by_fnode


def _edge_lookup_for_sources(
    conn: sqlite3.Connection,
    src_fnodes: list[str],
) -> dict[str, list[str]]:
    if not src_fnodes:
        return {}

    positions = {fnode: index for index, fnode in enumerate(src_fnodes)}
    edge_rows: list[tuple[str, str, int]] = []
    chunk_size = 500
    for start in range(0, len(src_fnodes), chunk_size):
        chunk = src_fnodes[start : start + chunk_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = conn.execute(
            f"""
            SELECT src_fnode, dst_fnode, ord
            FROM mdoc_edges
            WHERE src_fnode IN ({placeholders})
              AND NOT EXISTS (
                SELECT 1
                FROM mdoc_issues
                WHERE mdoc_issues.path = mdoc_edges.src_path
                  AND mdoc_issues.kind IN ('invalid', 'duplicate')
              )
            """,
            tuple(chunk),
        ).fetchall()
        edge_rows.extend((str(row[0]), str(row[1]), int(row[2])) for row in rows)

    edge_rows.sort(key=lambda row: (positions[row[0]], row[2]))
    edges_by_src = {fnode: [] for fnode in src_fnodes}
    for src_fnode, dst_fnode, _ in edge_rows:
        edges_by_src[src_fnode].append(dst_fnode)
    return edges_by_src


def _dependency_item_for_fnode(
    conn: sqlite3.Connection,
    *,
    fnode: str,
    depth: int,
    node_lookup: dict[str, tuple[str, str]] | None = None,
    issue_lookup: dict[str, GraphIssue] | None = None,
) -> DependencyItem:
    issue = _issue_for_fnode(
        conn,
        fnode,
        issue_lookup=issue_lookup,
    )
    if issue is not None:
        return DependencyItem(
            depth=depth,
            fnode=issue.fnode,
            title=issue.title,
            rel_path=issue.rel_path,
        )

    nodes = node_lookup if node_lookup is not None else _node_lookup(conn)
    row = nodes.get(fnode)
    if row is not None:
        return DependencyItem(
            depth=depth,
            fnode=fnode,
            title=row[0],
            rel_path=row[1],
        )
    return DependencyItem(
        depth=depth,
        fnode=fnode,
        title="<missing>",
        rel_path="<unknown>",
    )


def _valid_node_rows(
    conn: sqlite3.Connection,
) -> list[tuple[str, str, str]]:
    rows = conn.execute(
        """
        SELECT mdocs.fnode, mdocs.title, mdocs.path
        FROM mdocs
        WHERE NOT EXISTS (
            SELECT 1
            FROM mdoc_issues
            WHERE mdoc_issues.path = mdocs.path
              AND mdoc_issues.kind IN ('invalid', 'duplicate')
        )
        ORDER BY mdocs.path, mdocs.fnode
        """
    ).fetchall()
    return [(str(row[0]), str(row[1]), str(row[2])) for row in rows]


def _node_lookup(
    conn: sqlite3.Connection,
) -> dict[str, tuple[str, str]]:
    return {fnode: (title, path) for fnode, title, path in _valid_node_rows(conn)}


def _issue_lookup(
    conn: sqlite3.Connection,
) -> dict[str, GraphIssue]:
    issue_map: dict[str, GraphIssue] = {}
    for issue in _invalid_issue_rows(conn):
        issue_map.setdefault(issue.fnode, issue)
    for issue in _missing_issue_rows(conn):
        issue_map.setdefault(issue.fnode, issue)
    return issue_map


def _valid_edge_rows(
    conn: sqlite3.Connection,
) -> list[tuple[str, str]]:
    rows = conn.execute(
        """
        SELECT src_fnode, dst_fnode
        FROM mdoc_edges
        WHERE NOT EXISTS (
            SELECT 1
            FROM mdoc_issues
            WHERE mdoc_issues.path = mdoc_edges.src_path
              AND mdoc_issues.kind IN ('invalid', 'duplicate')
        )
        ORDER BY src_path, ord
        """
    ).fetchall()
    return [(str(row[0]), str(row[1])) for row in rows]


def _reverse_graph_snapshot(
    conn: sqlite3.Connection,
) -> dict[str, list[str]]:
    reverse_graph: dict[str, list[str]] = {}
    for src_fnode, dst_fnode in _valid_edge_rows(conn):
        refs = reverse_graph.setdefault(dst_fnode, [])
        if src_fnode not in refs:
            refs.append(src_fnode)
    return reverse_graph


def _missing_issue_rows(conn: sqlite3.Connection) -> list[GraphIssue]:
    rows = conn.execute(
        """
        SELECT ref_fnode, error
        FROM mdoc_issues
        WHERE kind = 'missing'
          AND NOT EXISTS (
            SELECT 1
            FROM mdoc_issues AS src_issues
            WHERE src_issues.path = mdoc_issues.path
              AND src_issues.kind IN ('invalid', 'duplicate')
          )
        ORDER BY ref_fnode, path
        """
    ).fetchall()
    deduped: list[GraphIssue] = []
    seen: set[str] = set()
    for row in rows:
        fnode = str(row[0])
        if fnode in seen:
            continue
        seen.add(fnode)
        deduped.append(
            GraphIssue(
                kind="missing",
                fnode=fnode,
                title="<missing>",
                rel_path="<unknown>",
                error=str(row[1]),
            )
        )
    return deduped


def _invalid_issue_rows(conn: sqlite3.Connection) -> list[GraphIssue]:
    rows = conn.execute(
        """
        SELECT path, ref_fnode, error
        FROM mdoc_issues
        WHERE kind IN ('invalid', 'duplicate')
        ORDER BY path, ref_fnode, error
        """
    ).fetchall()
    return [
        GraphIssue(
            kind="invalid",
            fnode=str(row[1]),
            title="<invalid>",
            rel_path=str(row[0]),
            error=str(row[2]),
        )
        for row in rows
    ]


def _dep_graph_snapshot(
    conn: sqlite3.Connection,
    *,
    valid_node_rows: list[tuple[str, str, str]] | None = None,
    invalid_issues: list[GraphIssue] | None = None,
) -> dict[str, list[str]]:
    graph: dict[str, list[str]] = {}
    active_valid_node_rows = valid_node_rows or _valid_node_rows(conn)
    active_invalid_issues = invalid_issues or _invalid_issue_rows(conn)
    for fnode, _, _ in active_valid_node_rows:
        graph.setdefault(fnode, [])
    for issue in active_invalid_issues:
        if issue.fnode.startswith("<") and issue.fnode.endswith(">"):
            continue
        graph.setdefault(issue.fnode, [])

    for src_fnode, dst_fnode in _valid_edge_rows(conn):
        graph.setdefault(src_fnode, []).append(dst_fnode)
        graph.setdefault(dst_fnode, [])
    return graph
