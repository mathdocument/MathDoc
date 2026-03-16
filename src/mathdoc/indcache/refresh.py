import sqlite3
from collections import deque
from collections.abc import Iterator
from pathlib import Path

from ..mdochead import read_mdoc_head
from ..utils import find_nested_mdcroot, iter_workspace_mdoc_files


def iter_mdoc_files(root: Path) -> Iterator[Path]:
    yield from iter_workspace_mdoc_files(root)


def refresh_search_index(
    *,
    root: Path,
    conn: sqlite3.Connection,
) -> None:
    rows = conn.execute("SELECT path, mtime_ns, size FROM mdoc_files").fetchall()
    cached_by_path = {str(row[0]): (int(row[1]), int(row[2])) for row in rows}
    indexed_paths = {
        str(row[0])
        for row in conn.execute(
            """
            SELECT path FROM mdoc_files
            UNION
            SELECT path FROM mdocs
            UNION
            SELECT path FROM mdoc_issues
            UNION
            SELECT src_path AS path FROM mdoc_edges
            """
        ).fetchall()
    }

    seen_paths: set[str] = set()
    for file_path in iter_mdoc_files(root):
        rel_path = file_path.relative_to(root).as_posix()
        seen_paths.add(rel_path)

        try:
            stat = file_path.stat()
        except OSError:
            continue

        current_state = (int(stat.st_mtime_ns), int(stat.st_size))
        if cached_by_path.get(rel_path) == current_state:
            continue
        upsert_mdoc_row(
            root=root,
            conn=conn,
            file_path=file_path,
            commit=False,
        )

    for stale_path in indexed_paths - seen_paths:
        delete_indexed_path(conn, stale_path=stale_path)

    from .discovery import rebuild_directory_index

    rebuild_directory_index(root=root, conn=conn)
    conn.execute("UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1")
    conn.commit()


def refresh_indexed_paths(
    *,
    root: Path,
    conn: sqlite3.Connection,
) -> None:
    rows = conn.execute("SELECT path, mtime_ns, size FROM mdoc_files").fetchall()
    for row in rows:
        rel_path = str(row[0])
        file_path = root / rel_path
        try:
            stat = file_path.stat()
        except OSError:
            delete_indexed_path(conn, stale_path=rel_path)
            continue

        current_state = (int(stat.st_mtime_ns), int(stat.st_size))
        cached_state = (int(row[1]), int(row[2]))
        if current_state == cached_state:
            continue
        upsert_mdoc_row(
            root=root,
            conn=conn,
            file_path=file_path,
            commit=False,
        )


def refresh_rows(
    *,
    root: Path,
    conn: sqlite3.Connection,
    rows: list[tuple[str, str, str]],
) -> None:
    rel_paths: list[str] = []
    seen_paths: set[str] = set()
    for _, _, rel_path in rows:
        if rel_path.startswith("<") and rel_path.endswith(">"):
            continue
        if rel_path in seen_paths:
            continue
        seen_paths.add(rel_path)
        rel_paths.append(rel_path)

    for rel_path in rel_paths:
        upsert_mdoc_row(
            root=root,
            conn=conn,
            file_path=root / rel_path,
            commit=False,
        )


def refresh_reachable_from_path(
    *,
    root: Path,
    conn: sqlite3.Connection,
    root_path: Path,
    depth: int,
) -> None:
    if depth < -1:
        raise ValueError("depth must be -1 (infinite) or >= 0")

    seen_paths: set[str] = set()
    queue: deque[tuple[Path, int]] = deque([(root_path.resolve(), 0)])

    while queue:
        file_path, item_depth = queue.popleft()
        rel_path = rel_path_under_root(root, file_path)
        if rel_path in seen_paths:
            continue
        seen_paths.add(rel_path)

        upsert_mdoc_row(
            root=root,
            conn=conn,
            file_path=file_path,
            commit=False,
        )

        if depth != -1 and item_depth >= depth:
            continue
        if path_has_blocking_issue(conn, rel_path):
            continue

        for dep_fnode in edge_targets_for_source_path(conn, rel_path):
            dep_rel_path = path_for_fnode_if_unique(conn, dep_fnode)
            if dep_rel_path is None:
                continue
            queue.append((root / dep_rel_path, item_depth + 1))


def upsert_mdoc_row(
    *,
    root: Path,
    conn: sqlite3.Connection,
    file_path: Path,
    commit: bool,
) -> None:
    nested_root = find_nested_mdcroot(root, file_path.resolve().parent)
    if nested_root is not None:
        raise ValueError(f"mdoc path is inside nested mdoc root: {nested_root}")

    rel_path = rel_path_under_root(root, file_path)
    old_fnode = cached_fnode_for_path(conn, rel_path)

    if not file_path.is_file():
        delete_indexed_path(conn, stale_path=rel_path)
        if commit:
            conn.commit()
        return

    try:
        stat = file_path.stat()
    except OSError:
        delete_indexed_path(conn, stale_path=rel_path)
        if commit:
            conn.commit()
        return

    conn.execute(
        """
        INSERT INTO mdoc_files (path, mtime_sec, mtime_ns, size)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(path) DO UPDATE SET
            mtime_sec = excluded.mtime_sec,
            mtime_ns = excluded.mtime_ns,
            size = excluded.size
        """,
        (rel_path, int(stat.st_mtime), int(stat.st_mtime_ns), int(stat.st_size)),
    )
    from ..mdocnode import MdocNode

    _struct = MdocNode(mdcroot=root, path=file_path, title="")
    parse_node = None
    parse_error: str | None = None
    try:
        _struct.load(include_blocks=False)
        parse_node = _struct
    except (OSError, ValueError) as exc:
        parse_error = str(exc)
    head = read_mdoc_head(file_path)
    if head is None:
        conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
    else:
        upsert_search_row(
            conn,
            rel_path=rel_path,
            fnode=head[0],
            title=head[1],
            stat=stat,
        )

    old_dst_fnodes = {
        str(row[0])
        for row in conn.execute(
            "SELECT dst_fnode FROM mdoc_edges WHERE src_path = ?", (rel_path,)
        ).fetchall()
    }
    conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", (rel_path,))
    conn.execute("DELETE FROM mdoc_issues WHERE path = ?", (rel_path,))

    new_fnode = head[0] if head is not None else None
    new_dst_fnodes: set[str] = set()
    if parse_node is not None:
        new_fnode = parse_node.fnode
        upsert_search_row(
            conn,
            rel_path=rel_path,
            fnode=parse_node.fnode,
            title=parse_node.title,
            stat=stat,
        )
        for order, dep_fnode in enumerate(parse_node.depens):
            conn.execute(
                """
                INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                VALUES (?, ?, ?, ?)
                """,
                (rel_path, parse_node.fnode, dep_fnode, order),
            )
            new_dst_fnodes.add(dep_fnode)
    elif parse_error is not None:
        insert_issue(
            conn,
            path=rel_path,
            kind="invalid",
            ref_fnode=(head[0] if head is not None else "<unknown>"),
            error=parse_error,
        )

    refresh_duplicate_issues_for_fnode(conn, old_fnode)
    refresh_duplicate_issues_for_fnode(conn, new_fnode)
    refresh_missing_issues_for_source(conn, rel_path)
    refresh_missing_issues_for_target(conn, old_fnode)
    refresh_missing_issues_for_target(conn, new_fnode)

    # Refresh in_degree for all affected dst_fnodes (after issues are up to date)
    affected_fnodes = old_dst_fnodes | new_dst_fnodes
    # Also include targets of edges from other paths sharing old_fnode or new_fnode
    # (duplicate status may have changed for them)
    for fnode in [f for f in [old_fnode, new_fnode] if f]:
        fnode_targets = {
            str(row[0])
            for row in conn.execute(
                "SELECT dst_fnode FROM mdoc_edges WHERE src_fnode = ?", (fnode,)
            ).fetchall()
        }
        affected_fnodes |= fnode_targets
    _refresh_in_degree_for_fnodes(conn, affected_fnodes)
    if old_fnode != new_fnode or old_dst_fnodes != new_dst_fnodes:
        _bump_graph_epoch(conn)

    if commit:
        conn.commit()


def delete_indexed_path(conn: sqlite3.Connection, *, stale_path: str) -> None:
    old_fnode = cached_fnode_for_path(conn, stale_path)
    old_dst_fnodes = {
        str(row[0])
        for row in conn.execute(
            "SELECT dst_fnode FROM mdoc_edges WHERE src_path = ?", (stale_path,)
        ).fetchall()
    }
    conn.execute("DELETE FROM mdoc_files WHERE path = ?", (stale_path,))
    conn.execute("DELETE FROM mdocs WHERE path = ?", (stale_path,))
    conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", (stale_path,))
    conn.execute("DELETE FROM mdoc_issues WHERE path = ?", (stale_path,))
    refresh_duplicate_issues_for_fnode(conn, old_fnode)
    refresh_missing_issues_for_target(conn, old_fnode)
    # Also refresh in_degree for targets of other edges from paths with old_fnode
    # (their duplicate status may have changed)
    if old_fnode:
        fnode_targets = {
            str(row[0])
            for row in conn.execute(
                "SELECT dst_fnode FROM mdoc_edges WHERE src_fnode = ?", (old_fnode,)
            ).fetchall()
        }
        old_dst_fnodes |= fnode_targets
    _refresh_in_degree_for_fnodes(conn, old_dst_fnodes)
    if old_fnode is not None:
        _bump_graph_epoch(conn)


def rel_path_under_root(root: Path, file_path: Path) -> str:
    root_resolved = root.resolve()
    try:
        return file_path.resolve().relative_to(root_resolved).as_posix()
    except ValueError as exc:
        raise ValueError(f"mdoc path must be under mdoc root: {root_resolved}") from exc


def path_has_blocking_issue(conn: sqlite3.Connection, rel_path: str) -> bool:
    row = conn.execute(
        """
        SELECT 1
        FROM mdoc_issues
        WHERE path = ?
          AND kind IN ('invalid', 'duplicate')
        LIMIT 1
        """,
        (rel_path,),
    ).fetchone()
    return row is not None


def edge_targets_for_source_path(conn: sqlite3.Connection, src_path: str) -> list[str]:
    rows = conn.execute(
        """
        SELECT dst_fnode
        FROM mdoc_edges
        WHERE src_path = ?
        ORDER BY ord
        """,
        (src_path,),
    ).fetchall()
    return [str(row[0]) for row in rows]


def path_for_fnode_if_unique(conn: sqlite3.Connection, fnode: str) -> str | None:
    rows = conn.execute(
        """
        SELECT path
        FROM mdocs
        WHERE fnode = ?
        ORDER BY path
        LIMIT 2
        """,
        (fnode,),
    ).fetchall()
    if len(rows) != 1:
        return None
    return str(rows[0][0])


def cached_fnode_for_path(conn: sqlite3.Connection, rel_path: str) -> str | None:
    row = conn.execute(
        "SELECT fnode FROM mdocs WHERE path = ?",
        (rel_path,),
    ).fetchone()
    return None if row is None else str(row[0])


def upsert_search_row(
    conn: sqlite3.Connection,
    *,
    rel_path: str,
    fnode: str,
    title: str,
    stat: object,
) -> None:
    conn.execute(
        """
        INSERT INTO mdocs (path, fnode, title, title_lc, mtime_sec, mtime_ns, size)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(path) DO UPDATE SET
            fnode = excluded.fnode,
            title = excluded.title,
            title_lc = excluded.title_lc,
            mtime_sec = excluded.mtime_sec,
            mtime_ns = excluded.mtime_ns,
            size = excluded.size
        """,
        (
            rel_path,
            fnode,
            title,
            title.casefold(),
            int(getattr(stat, "st_mtime")),
            int(getattr(stat, "st_mtime_ns")),
            int(getattr(stat, "st_size")),
        ),
    )


def insert_issue(
    conn: sqlite3.Connection,
    *,
    path: str,
    kind: str,
    ref_fnode: str,
    error: str,
) -> None:
    conn.execute(
        """
        INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(path, kind, ref_fnode) DO UPDATE SET
            error = excluded.error
        """,
        (path, kind, ref_fnode, error),
    )


def refresh_duplicate_issues_for_fnode(
    conn: sqlite3.Connection,
    fnode: str | None,
) -> None:
    if not fnode or (fnode.startswith("<") and fnode.endswith(">")):
        return
    conn.execute(
        "DELETE FROM mdoc_issues WHERE kind = 'duplicate' AND ref_fnode = ?",
        (fnode,),
    )
    rows = conn.execute(
        "SELECT path FROM mdocs WHERE fnode = ? ORDER BY path",
        (fnode,),
    ).fetchall()
    if len(rows) < 2:
        return
    paths = [str(row[0]) for row in rows]
    error = f"duplicate fnode '{fnode}' across: {', '.join(paths)}"
    for path in paths:
        insert_issue(
            conn,
            path=path,
            kind="duplicate",
            ref_fnode=fnode,
            error=error,
        )


def refresh_missing_issues_for_source(conn: sqlite3.Connection, src_path: str) -> None:
    src_issue = conn.execute(
        """
        SELECT 1
        FROM mdoc_issues
        WHERE path = ?
          AND kind IN ('invalid', 'duplicate')
        LIMIT 1
        """,
        (src_path,),
    ).fetchone()
    if src_issue is not None:
        return

    dep_rows = conn.execute(
        """
        SELECT dst_fnode
        FROM mdoc_edges
        WHERE src_path = ?
        ORDER BY ord
        """,
        (src_path,),
    ).fetchall()
    for row in dep_rows:
        refresh_missing_issues_for_target(conn, str(row[0]))


def refresh_missing_issues_for_target(
    conn: sqlite3.Connection,
    target_fnode: str | None,
) -> None:
    if not target_fnode or (
        target_fnode.startswith("<") and target_fnode.endswith(">")
    ):
        return

    conn.execute(
        "DELETE FROM mdoc_issues WHERE kind = 'missing' AND ref_fnode = ?",
        (target_fnode,),
    )

    node_rows = conn.execute(
        """
        SELECT path
        FROM mdocs
        WHERE fnode = ?
        ORDER BY path
        LIMIT 2
        """,
        (target_fnode,),
    ).fetchall()
    if len(node_rows) == 1:
        return

    src_rows = conn.execute(
        """
        SELECT DISTINCT src_path
        FROM mdoc_edges
        WHERE dst_fnode = ?
        ORDER BY src_path
        """,
        (target_fnode,),
    ).fetchall()
    error = f"missing dependency target: {target_fnode}"
    for row in src_rows:
        src_path = str(row[0])
        src_issue = conn.execute(
            """
            SELECT 1
            FROM mdoc_issues
            WHERE path = ?
              AND kind IN ('invalid', 'duplicate')
            LIMIT 1
            """,
            (src_path,),
        ).fetchone()
        if src_issue is not None:
            continue
        insert_issue(
            conn,
            path=src_path,
            kind="missing",
            ref_fnode=target_fnode,
            error=error,
        )


def _refresh_in_degree_for_fnodes(
    conn: sqlite3.Connection,
    fnodes: set[str],
) -> None:
    """Recompute in_degree for the given fnodes based on current valid edges."""
    if not fnodes:
        return
    fnode_list = list(fnodes)
    chunk_size = 500
    for start in range(0, len(fnode_list), chunk_size):
        chunk = fnode_list[start : start + chunk_size]
        placeholders = ",".join("?" for _ in chunk)
        conn.execute(
            f"DELETE FROM mdoc_in_degree WHERE fnode IN ({placeholders})",
            tuple(chunk),
        )
        conn.execute(
            f"""
            INSERT INTO mdoc_in_degree (fnode, in_degree)
            SELECT dst_fnode, COUNT(*)
            FROM mdoc_edges
            WHERE dst_fnode IN ({placeholders})
              AND NOT EXISTS (
                SELECT 1 FROM mdoc_issues
                WHERE mdoc_issues.path = mdoc_edges.src_path
                  AND mdoc_issues.kind IN ('invalid', 'duplicate')
              )
            GROUP BY dst_fnode
            HAVING COUNT(*) > 0
            """,
            tuple(chunk),
        )


def _bump_graph_epoch(conn: sqlite3.Connection) -> None:
    """Increment graph_epoch and mark weak_component as dirty."""
    conn.execute(
        """
        UPDATE mdoc_index_state
        SET graph_epoch = graph_epoch + 1, weak_component_dirty = 1
        WHERE id = 1
        """
    )
