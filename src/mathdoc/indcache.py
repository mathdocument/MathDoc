import sqlite3
from collections import deque
from contextlib import closing, contextmanager
from pathlib import Path
from typing import Iterator

from .utils import find_nested_mdcroot, iter_workspace_mdoc_files


class IndCache:
    SCHEMA_VERSION = 2

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.db_path = self.root / ".mdc" / "index.db"

    @contextmanager
    def _open_conn(self) -> Iterator[sqlite3.Connection]:
        with closing(sqlite3.connect(self.db_path)) as conn:
            try:
                self._ensure_index_schema(conn)
                yield conn
                conn.commit()
            except Exception:
                conn.rollback()
                raise

    def bootstrap_if_needed(self) -> None:
        with self._open_conn() as conn:
            if self._bootstrap_required(conn):
                self._refresh_search_index(conn)

    def refresh_all(self) -> None:
        with self._open_conn() as conn:
            self._refresh_search_index(conn)

    def count(self) -> int:
        with self._open_conn() as conn:
            row = conn.execute("SELECT COUNT(*) FROM mdocs").fetchone()
        return int(row[0]) if row else 0

    def upsert_path(self, file_path: Path) -> None:
        with self._open_conn() as conn:
            self._upsert_mdoc_row(conn, file_path, commit=True)

    def refresh_rows(self, rows: list[tuple[str, str, str]]) -> None:
        rel_paths: list[str] = []
        seen_paths: set[str] = set()
        for _, _, rel_path in rows:
            if rel_path.startswith("<") and rel_path.endswith(">"):
                continue
            if rel_path in seen_paths:
                continue
            seen_paths.add(rel_path)
            rel_paths.append(rel_path)

        if not rel_paths:
            return

        with self._open_conn() as conn:
            for rel_path in rel_paths:
                self._upsert_mdoc_row(
                    conn,
                    self.root / rel_path,
                    commit=False,
                )
            conn.commit()

    def search(self, query: str) -> list[tuple[str, str, str]]:
        query_lc = query.casefold()
        like = f"%{query_lc}%"
        with self._open_conn() as conn:
            rows = conn.execute(
                """
                SELECT fnode, title, path
                FROM mdocs
                WHERE title_lc LIKE ? OR lower(fnode) LIKE ?
                ORDER BY
                    CASE WHEN lower(fnode) LIKE ? THEN 0 ELSE 1 END,
                    CASE WHEN instr(title_lc, ?) > 0 THEN instr(title_lc, ?) ELSE 999999 END,
                    length(title),
                    path
                """,
                (like, like, f"{query_lc}%", query_lc, query_lc),
            ).fetchall()
        return [(str(row[0]), str(row[1]), str(row[2])) for row in rows]

    def duplicate_fnode_paths(self, fnode: str) -> list[Path]:
        rows = self.exact_fnode_rows(fnode)
        return [self.root / row[2] for row in rows]

    def exact_fnode_rows(self, fnode: str) -> list[tuple[str, str, str]]:
        query_lc = fnode.casefold()
        with self._open_conn() as conn:
            rows = conn.execute(
                """
                SELECT fnode, title, path
                FROM mdocs
                WHERE lower(fnode) = ?
                ORDER BY path
                """,
                (query_lc,),
            ).fetchall()
        return [(str(row[0]), str(row[1]), str(row[2])) for row in rows]

    def resolve_ref(
        self, ref: str, *, cwd: Path | None = None
    ) -> tuple[str, str, Path]:
        raw_ref = ref.strip()
        if not raw_ref:
            raise ValueError("mdoc reference cannot be empty")

        base_cwd = (cwd or Path.cwd()).resolve()
        existing_path = self._resolve_existing_ref_path(raw_ref, cwd=base_cwd)
        with self._open_conn() as conn:
            if existing_path is not None:
                candidate, rel_path = existing_path
                row = conn.execute(
                    "SELECT fnode, title FROM mdocs WHERE path = ?", (rel_path,)
                ).fetchone()
                if row is not None:
                    return str(row[0]), str(row[1]), candidate

                head = self.read_mdoc_head(candidate)
                if head is None or not head[0]:
                    raise ValueError(f"invalid mdoc file: {candidate}")
                return str(head[0]), str(head[1]), candidate

            query_lc = raw_ref.casefold()
            rows = conn.execute(
                """
                SELECT fnode, title, path
                FROM mdocs
                WHERE lower(fnode) = ? OR lower(fnode) LIKE ?
                ORDER BY
                    CASE WHEN lower(fnode) = ? THEN 0 ELSE 1 END,
                    path
                """,
                (query_lc, f"{query_lc}%", query_lc),
            ).fetchall()

        if not rows:
            raise ValueError(f"no mdoc matched reference: {raw_ref}")

        exact_rows = [row for row in rows if str(row[0]).casefold() == query_lc]
        if exact_rows:
            if len(exact_rows) == 1:
                row = exact_rows[0]
            else:
                preview = self._format_ref_preview(exact_rows[:5])
                raise ValueError(
                    f"ambiguous mdoc reference '{raw_ref}', matches: {preview}"
                )
        elif len(rows) == 1:
            row = rows[0]
        else:
            preview = self._format_ref_preview(rows[:5])
            raise ValueError(
                f"ambiguous mdoc reference '{raw_ref}', matches: {preview}"
            )

        rel_path = str(row[2])
        return str(row[0]), str(row[1]), self.root / rel_path

    def resolve_edit_target_path(self, ref: str, *, cwd: Path | None = None) -> Path:
        raw_ref = ref.strip()
        if not raw_ref:
            raise ValueError("mdoc reference cannot be empty")

        base_cwd = (cwd or Path.cwd()).resolve()
        existing_path = self._resolve_existing_ref_path(raw_ref, cwd=base_cwd)
        if existing_path is not None:
            return existing_path[0]

        _, _, resolved = self.resolve_ref(raw_ref, cwd=base_cwd)
        return resolved

    def lookup_by_fnode(self, fnodes: list[str]) -> dict[str, tuple[str, str]]:
        if not fnodes:
            return {}

        rows_by_fnode: dict[str, tuple[str, str]] = {}
        duplicate_fnodes: set[str] = set()
        chunk_size = 500
        with self._open_conn() as conn:
            for start in range(0, len(fnodes), chunk_size):
                chunk = fnodes[start : start + chunk_size]
                placeholders = ",".join("?" for _ in chunk)
                rows = conn.execute(
                    f"SELECT fnode, title, path FROM mdocs WHERE fnode IN ({placeholders})",
                    tuple(chunk),
                ).fetchall()
                for row in rows:
                    fnode = str(row[0])
                    if fnode in duplicate_fnodes:
                        continue
                    entry = (str(row[1]), str(row[2]))
                    existing = rows_by_fnode.get(fnode)
                    if existing is None:
                        rows_by_fnode[fnode] = entry
                        continue
                    if existing != entry:
                        duplicate_fnodes.add(fnode)
                        rows_by_fnode.pop(fnode, None)
        return rows_by_fnode

    def dep_rows(self, depens: list[str]) -> list[tuple[str, str, str]]:
        dep_meta = self.lookup_by_fnode(depens)
        rows: list[tuple[str, str, str]] = []
        for dep_fnode in depens:
            title, path = dep_meta.get(dep_fnode, ("<missing>", "<not indexed>"))
            rows.append((dep_fnode, title, path))
        return rows

    def indexed_file_count(self) -> int:
        with self._open_conn() as conn:
            row = conn.execute("SELECT COUNT(*) FROM mdoc_files").fetchone()
        return int(row[0]) if row else 0

    def issue_for_fnode(self, fnode: str):
        with self._open_conn() as conn:
            return self._issue_for_fnode(conn, fnode)

    def is_broken_fnode(self, fnode: str) -> bool:
        return self.issue_for_fnode(fnode) is not None

    def ref_item_for_fnode(self, fnode: str, *, depth: int = 0):
        from .depgraph.models import DependencyItem

        with self._open_conn() as conn:
            return self._dependency_item_for_fnode(conn, fnode=fnode, depth=depth)

    def referrer_items(self, *, target_fnode: str, depth: int):
        from .depgraph.models import DependencyItem

        if depth < -1:
            raise ValueError("depth must be -1 (infinite) or >= 0")

        with self._open_conn() as conn:
            items: list[DependencyItem] = []
            seen: set[str] = {target_fnode}
            queue: deque[tuple[str, int]] = deque(
                (ref_fnode, 1)
                for ref_fnode in self._referrer_fnodes(conn, target_fnode)
            )

            while queue:
                fnode, item_depth = queue.popleft()
                if fnode in seen:
                    continue
                seen.add(fnode)
                items.append(
                    self._dependency_item_for_fnode(
                        conn,
                        fnode=fnode,
                        depth=item_depth,
                    )
                )

                if depth != -1 and item_depth >= depth:
                    continue
                for ref_fnode in self._referrer_fnodes(conn, fnode):
                    if ref_fnode == target_fnode:
                        continue
                    queue.append((ref_fnode, item_depth + 1))

        return items

    def global_root_items(self):
        from .depgraph.models import GraphRootItem

        with self._open_conn() as conn:
            incoming = {
                str(row[0])
                for row in conn.execute("SELECT DISTINCT dst_fnode FROM mdoc_edges")
            }
            component_sizes = self._component_sizes(conn)
            items: list[GraphRootItem] = []

            for fnode, title, path in self._valid_node_rows(conn):
                if fnode in incoming:
                    continue
                items.append(
                    GraphRootItem(
                        fnode=fnode,
                        title=title,
                        rel_path=path,
                        component_size=component_sizes.get(fnode, 1),
                    )
                )

            for issue in self._invalid_issue_rows(conn):
                if not (
                    issue.fnode.startswith("<") and issue.fnode.endswith(">")
                ) and issue.fnode in incoming:
                    continue
                items.append(
                    GraphRootItem(
                        fnode=issue.fnode,
                        title=issue.title,
                        rel_path=issue.rel_path,
                        component_size=component_sizes.get(issue.fnode, 1),
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

    def graph_check_report(self):
        from .depgraph.algorithms import (
            component_has_cycle,
            representative_cycle,
            strongly_connected_components,
        )
        from .depgraph.models import GraphCheckReport

        with self._open_conn() as conn:
            dep_graph = self._dep_graph_snapshot(conn)
            cycles: list[list[str]] = []
            for component in strongly_connected_components(dep_graph):
                if not component_has_cycle(dep_graph, component):
                    continue
                cycle = representative_cycle(dep_graph, component)
                if cycle is not None:
                    cycles.append(cycle)
            cycles.sort(key=lambda cycle: tuple(cycle))

            return GraphCheckReport(
                nodes=self._file_count(conn),
                edges=self._edge_count(conn),
                missing=self._missing_issue_rows(conn),
                invalid=self._invalid_issue_rows(conn),
                cycles=cycles,
            )

    @staticmethod
    def _file_count(conn: sqlite3.Connection) -> int:
        row = conn.execute("SELECT COUNT(*) FROM mdoc_files").fetchone()
        return int(row[0]) if row else 0

    @staticmethod
    def _edge_count(conn: sqlite3.Connection) -> int:
        row = conn.execute("SELECT COUNT(*) FROM mdoc_edges").fetchone()
        return int(row[0]) if row else 0

    def _issue_for_fnode(self, conn: sqlite3.Connection, fnode: str):
        from .depgraph.models import GraphIssue

        row = conn.execute(
            """
            SELECT kind, path, error
            FROM mdoc_issues
            WHERE ref_fnode = ? AND kind IN ('invalid', 'duplicate')
            ORDER BY path
            LIMIT 1
            """,
            (fnode,),
        ).fetchone()
        if row is not None:
            return GraphIssue(
                kind="invalid",
                fnode=fnode,
                title="<invalid>",
                rel_path=str(row[1]),
                error=str(row[2]),
            )

        row = conn.execute(
            """
            SELECT error
            FROM mdoc_issues
            WHERE ref_fnode = ? AND kind = 'missing'
            ORDER BY path
            LIMIT 1
            """,
            (fnode,),
        ).fetchone()
        if row is None:
            return None
        return GraphIssue(
            kind="missing",
            fnode=fnode,
            title="<missing>",
            rel_path="<unknown>",
            error=str(row[0]),
        )

    def _dependency_item_for_fnode(
        self,
        conn: sqlite3.Connection,
        *,
        fnode: str,
        depth: int,
    ):
        from .depgraph.models import DependencyItem

        issue = self._issue_for_fnode(conn, fnode)
        if issue is not None:
            return DependencyItem(
                depth=depth,
                fnode=issue.fnode,
                title=issue.title,
                rel_path=issue.rel_path,
            )

        rows = conn.execute(
            """
            SELECT fnode, title, path
            FROM mdocs
            WHERE fnode = ?
            ORDER BY path
            LIMIT 1
            """,
            (fnode,),
        ).fetchall()
        if rows:
            row = rows[0]
            return DependencyItem(
                depth=depth,
                fnode=str(row[0]),
                title=str(row[1]),
                rel_path=str(row[2]),
            )
        return DependencyItem(
            depth=depth,
            fnode=fnode,
            title="<missing>",
            rel_path="<unknown>",
        )

    def _referrer_fnodes(
        self,
        conn: sqlite3.Connection,
        target_fnode: str,
    ) -> list[str]:
        rows = conn.execute(
            """
            SELECT DISTINCT src_fnode, src_path
            FROM mdoc_edges
            WHERE dst_fnode = ?
            ORDER BY src_path
            """,
            (target_fnode,),
        ).fetchall()
        return [str(row[0]) for row in rows]

    def _valid_node_rows(
        self,
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

    def _missing_issue_rows(self, conn: sqlite3.Connection):
        from .depgraph.models import GraphIssue

        rows = conn.execute(
            """
            SELECT ref_fnode, error
            FROM mdoc_issues
            WHERE kind = 'missing'
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

    def _invalid_issue_rows(self, conn: sqlite3.Connection):
        from .depgraph.models import GraphIssue

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
        self,
        conn: sqlite3.Connection,
    ) -> dict[str, list[str]]:
        graph: dict[str, list[str]] = {}
        for fnode, _, _ in self._valid_node_rows(conn):
            graph.setdefault(fnode, [])
        for issue in self._invalid_issue_rows(conn):
            if issue.fnode.startswith("<") and issue.fnode.endswith(">"):
                continue
            graph.setdefault(issue.fnode, [])

        rows = conn.execute(
            """
            SELECT src_fnode, dst_fnode, ord
            FROM mdoc_edges
            ORDER BY src_path, ord
            """
        ).fetchall()
        for row in rows:
            src_fnode = str(row[0])
            dst_fnode = str(row[1])
            graph.setdefault(src_fnode, []).append(dst_fnode)
            graph.setdefault(dst_fnode, [])
        return graph

    def _component_sizes(self, conn: sqlite3.Connection) -> dict[str, int]:
        adjacency: dict[str, set[str]] = {}
        graph = self._dep_graph_snapshot(conn)
        for src_fnode, dep_fnodes in graph.items():
            adjacency.setdefault(src_fnode, set())
            for dep_fnode in dep_fnodes:
                if not self._is_component_member(conn, dep_fnode):
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
        return sizes

    def _is_component_member(self, conn: sqlite3.Connection, fnode: str) -> bool:
        if self._fnode_exists(conn, fnode):
            return True
        row = conn.execute(
            """
            SELECT 1
            FROM mdoc_issues
            WHERE ref_fnode = ? AND kind IN ('invalid', 'duplicate')
            LIMIT 1
            """,
            (fnode,),
        ).fetchone()
        return row is not None

    @staticmethod
    def _ensure_index_schema(conn: sqlite3.Connection) -> None:
        user_version = int(conn.execute("PRAGMA user_version").fetchone()[0])
        column_rows = conn.execute("PRAGMA table_info(mdocs)").fetchall()
        if not column_rows:
            IndCache._create_index_table(conn)
            column_rows = conn.execute("PRAGMA table_info(mdocs)").fetchall()

        columns = {str(row[1]) for row in column_rows}
        fnode_is_primary = any(
            str(row[1]) == "fnode" and int(row[5]) == 1 for row in column_rows
        )
        path_is_primary = any(
            str(row[1]) == "path" and int(row[5]) == 1 for row in column_rows
        )
        if fnode_is_primary or not path_is_primary:
            conn.execute("DROP TABLE mdocs")
            IndCache._create_index_table(conn)
            columns = {
                str(row[1]) for row in conn.execute("PRAGMA table_info(mdocs)").fetchall()
            }
        if "mtime_ns" not in columns:
            if "mtime_sec" not in columns:
                raise sqlite3.DatabaseError(
                    "mdocs table is missing required mtime columns"
                )
            conn.execute(
                "ALTER TABLE mdocs ADD COLUMN mtime_ns INTEGER NOT NULL DEFAULT 0"
            )
            conn.execute(
                "UPDATE mdocs SET mtime_ns = mtime_sec * 1000000000 WHERE mtime_ns = 0"
            )
            columns.add("mtime_ns")
        if "mtime_sec" not in columns:
            conn.execute(
                "ALTER TABLE mdocs ADD COLUMN mtime_sec INTEGER NOT NULL DEFAULT 0"
            )
            conn.execute(
                "UPDATE mdocs SET mtime_sec = CAST(mtime_ns / 1000000000 AS INTEGER)"
            )

        IndCache._create_file_state_table(conn)
        IndCache._create_edge_table(conn)
        IndCache._create_issue_table(conn)
        conn.execute("CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc)")
        conn.execute("CREATE INDEX IF NOT EXISTS idx_mdocs_fnode ON mdocs(fnode)")
        if user_version < IndCache.SCHEMA_VERSION:
            conn.execute("PRAGMA user_version = 2")

    @staticmethod
    def _create_index_table(conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS mdocs (
                path TEXT PRIMARY KEY,
                fnode TEXT NOT NULL,
                title TEXT NOT NULL,
                title_lc TEXT NOT NULL,
                mtime_sec INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL,
                size INTEGER NOT NULL
            )
            """
        )

    @staticmethod
    def _create_file_state_table(conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS mdoc_files (
                path TEXT PRIMARY KEY,
                mtime_sec INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL,
                size INTEGER NOT NULL
            )
            """
        )

    @staticmethod
    def _create_edge_table(conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS mdoc_edges (
                src_path TEXT NOT NULL,
                src_fnode TEXT NOT NULL,
                dst_fnode TEXT NOT NULL,
                ord INTEGER NOT NULL,
                PRIMARY KEY (src_path, ord)
            )
            """
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_fnode ON mdoc_edges(src_fnode)"
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_fnode ON mdoc_edges(dst_fnode)"
        )

    @staticmethod
    def _create_issue_table(conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS mdoc_issues (
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                ref_fnode TEXT NOT NULL,
                error TEXT NOT NULL,
                PRIMARY KEY (path, kind, ref_fnode)
            )
            """
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mdoc_issues_kind ON mdoc_issues(kind)"
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mdoc_issues_ref_fnode ON mdoc_issues(ref_fnode)"
        )

    def _iter_mdoc_files(self) -> Iterator[Path]:
        yield from iter_workspace_mdoc_files(self.root)

    @staticmethod
    def read_mdoc_head(file_path: Path) -> tuple[str, str] | None:
        fnode = ""
        title = ""
        try:
            with file_path.open("r", encoding="utf-8") as f:
                for raw_line in f:
                    line = raw_line.strip()
                    lower = line.lower()
                    if lower.startswith("@fnode:"):
                        fnode = line.split(":", 1)[1].strip()
                    elif lower.startswith("@title:"):
                        title = line.split(":", 1)[1].strip()
                    if fnode and title:
                        break
        except OSError:
            return None

        if not fnode or not title:
            return None
        return fnode, title

    @staticmethod
    def _index_is_empty(conn: sqlite3.Connection) -> bool:
        row = conn.execute("SELECT 1 FROM mdocs LIMIT 1").fetchone()
        return row is None

    @staticmethod
    def _bootstrap_required(conn: sqlite3.Connection) -> bool:
        if IndCache._index_is_empty(conn):
            return True
        row = conn.execute("SELECT 1 FROM mdoc_files LIMIT 1").fetchone()
        return row is None

    def _refresh_search_index(self, conn: sqlite3.Connection) -> None:
        rows = conn.execute(
            "SELECT path, mtime_ns, size FROM mdoc_files"
        ).fetchall()
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
        for file_path in self._iter_mdoc_files():
            rel_path = file_path.relative_to(self.root).as_posix()
            seen_paths.add(rel_path)

            try:
                stat = file_path.stat()
            except OSError:
                continue

            mtime_ns = int(stat.st_mtime_ns)
            size = int(stat.st_size)
            cached = cached_by_path.get(rel_path)
            if cached and cached[0] == mtime_ns and cached[1] == size:
                continue

            self._upsert_mdoc_row(
                conn,
                file_path,
                commit=False,
            )

        stale_paths = indexed_paths - seen_paths
        for stale_path in stale_paths:
            self._delete_indexed_path(
                conn,
                stale_path=stale_path,
            )

        conn.commit()

    def _upsert_mdoc_row(
        self,
        conn: sqlite3.Connection,
        file_path: Path,
        *,
        commit: bool,
    ) -> None:
        nested_root = find_nested_mdcroot(self.root, file_path.resolve().parent)
        if nested_root is not None:
            raise ValueError(f"mdoc path is inside nested mdoc root: {nested_root}")

        rel_path = self._rel_path_under_root(file_path)
        old_fnode = self._cached_fnode_for_path(conn, rel_path)

        if not file_path.is_file():
            self._delete_indexed_path(conn, stale_path=rel_path)
            if commit:
                conn.commit()
            return

        try:
            stat = file_path.stat()
        except OSError:
            self._delete_indexed_path(conn, stale_path=rel_path)
            if commit:
                conn.commit()
            return

        self._upsert_file_state(
            conn,
            rel_path=rel_path,
            stat=stat,
        )
        parse_node, parse_error = self._read_structure_node(file_path)
        head = self.read_mdoc_head(file_path)
        if head is None:
            conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
        else:
            self._upsert_search_row(
                conn,
                rel_path=rel_path,
                fnode=head[0],
                title=head[1],
                stat=stat,
            )

        conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", (rel_path,))
        conn.execute("DELETE FROM mdoc_issues WHERE path = ?", (rel_path,))

        new_fnode = head[0] if head is not None else None
        if parse_node is not None:
            new_fnode = parse_node.fnode
            self._upsert_search_row(
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
        elif parse_error is not None:
            self._insert_issue(
                conn,
                path=rel_path,
                kind="invalid",
                ref_fnode=(head[0] if head is not None else "<unknown>"),
                error=parse_error,
            )

        self._refresh_duplicate_issues_for_fnode(conn, old_fnode)
        self._refresh_duplicate_issues_for_fnode(conn, new_fnode)
        self._refresh_missing_issues_for_source(conn, rel_path)
        self._refresh_missing_issues_for_target(conn, old_fnode)
        self._refresh_missing_issues_for_target(conn, new_fnode)
        if commit:
            conn.commit()

    def _delete_indexed_path(
        self,
        conn: sqlite3.Connection,
        *,
        stale_path: str,
    ) -> None:
        old_fnode = self._cached_fnode_for_path(conn, stale_path)
        conn.execute("DELETE FROM mdoc_files WHERE path = ?", (stale_path,))
        conn.execute("DELETE FROM mdocs WHERE path = ?", (stale_path,))
        conn.execute("DELETE FROM mdoc_edges WHERE src_path = ?", (stale_path,))
        conn.execute("DELETE FROM mdoc_issues WHERE path = ?", (stale_path,))
        self._refresh_duplicate_issues_for_fnode(conn, old_fnode)
        self._refresh_missing_issues_for_target(conn, old_fnode)

    def _rel_path_under_root(self, file_path: Path) -> str:
        try:
            return file_path.resolve().relative_to(self.root.resolve()).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"mdoc path must be under mdoc root: {self.root.resolve()}"
            ) from exc

    @staticmethod
    def _cached_fnode_for_path(conn: sqlite3.Connection, rel_path: str) -> str | None:
        row = conn.execute(
            "SELECT fnode FROM mdocs WHERE path = ?",
            (rel_path,),
        ).fetchone()
        return None if row is None else str(row[0])

    @staticmethod
    def _upsert_file_state(
        conn: sqlite3.Connection,
        *,
        rel_path: str,
        stat: object,
    ) -> None:
        conn.execute(
            """
            INSERT INTO mdoc_files (path, mtime_sec, mtime_ns, size)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                mtime_sec = excluded.mtime_sec,
                mtime_ns = excluded.mtime_ns,
                size = excluded.size
            """,
            (
                rel_path,
                int(getattr(stat, "st_mtime")),
                int(getattr(stat, "st_mtime_ns")),
                int(getattr(stat, "st_size")),
            ),
        )

    @staticmethod
    def _upsert_search_row(
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

    def _read_structure_node(self, file_path: Path) -> tuple[object | None, str | None]:
        from .mdocnode import MdocNode

        node = MdocNode(mdcroot=self.root, path=file_path, title="")
        try:
            node.load(include_blocks=False)
        except (OSError, ValueError) as exc:
            return None, str(exc)
        return node, None

    @staticmethod
    def _insert_issue(
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

    def _refresh_duplicate_issues_for_fnode(
        self,
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
        if len(rows) <= 1:
            return
        paths = [str(row[0]) for row in rows]
        error = (
            f"duplicate fnode '{fnode}' found in: "
            + ", ".join(paths)
        )
        for path in paths:
            self._insert_issue(
                conn,
                path=path,
                kind="duplicate",
                ref_fnode=fnode,
                error=error,
            )

    def _refresh_missing_issues_for_source(
        self,
        conn: sqlite3.Connection,
        src_path: str,
    ) -> None:
        conn.execute(
            "DELETE FROM mdoc_issues WHERE kind = 'missing' AND path = ?",
            (src_path,),
        )
        rows = conn.execute(
            """
            SELECT dst_fnode
            FROM mdoc_edges
            WHERE src_path = ?
            ORDER BY ord
            """,
            (src_path,),
        ).fetchall()
        for row in rows:
            dst_fnode = str(row[0])
            if self._fnode_exists(conn, dst_fnode):
                continue
            self._insert_issue(
                conn,
                path=src_path,
                kind="missing",
                ref_fnode=dst_fnode,
                error=f"no mdoc matched reference: {dst_fnode}",
            )

    def _refresh_missing_issues_for_target(
        self,
        conn: sqlite3.Connection,
        fnode: str | None,
    ) -> None:
        if not fnode or (fnode.startswith("<") and fnode.endswith(">")):
            return
        rows = conn.execute(
            """
            SELECT DISTINCT src_path
            FROM mdoc_edges
            WHERE dst_fnode = ?
            """,
            (fnode,),
        ).fetchall()
        for row in rows:
            self._refresh_missing_issues_for_source(conn, str(row[0]))

    @staticmethod
    def _fnode_exists(conn: sqlite3.Connection, fnode: str) -> bool:
        row = conn.execute(
            "SELECT 1 FROM mdocs WHERE fnode = ? LIMIT 1",
            (fnode,),
        ).fetchone()
        return row is not None

    @staticmethod
    def _format_ref_preview(rows: list[tuple[object, object, object]]) -> str:
        return ", ".join(f"{str(row[0])[:8]}:{row[2]}" for row in rows)

    def _resolve_existing_ref_path(
        self,
        raw_ref: str,
        *,
        cwd: Path,
    ) -> tuple[Path, str] | None:
        if not self._looks_like_path_ref(raw_ref):
            return None

        raw_path = Path(raw_ref)
        for candidate in self._path_ref_candidates(raw_path, cwd=cwd):
            if not candidate.is_file():
                continue
            return candidate, self._workspace_rel_path(candidate)

        if raw_path.suffix == ".mdoc":
            raise ValueError(f"mdoc file not found: {raw_ref}")
        return None

    @staticmethod
    def _looks_like_path_ref(raw_ref: str) -> bool:
        return ("/" in raw_ref) or raw_ref.endswith(".mdoc") or raw_ref.startswith(".")

    def _path_ref_candidates(self, raw_path: Path, *, cwd: Path) -> list[Path]:
        if raw_path.is_absolute():
            return [raw_path.resolve()]
        return [(cwd / raw_path).resolve(), (self.root / raw_path).resolve()]

    def _workspace_rel_path(self, candidate: Path) -> str:
        root_resolved = self.root.resolve()
        nested_root = find_nested_mdcroot(root_resolved, candidate.parent)
        if nested_root is not None:
            raise ValueError(f"mdoc path is inside nested mdoc root: {nested_root}")
        try:
            return candidate.relative_to(root_resolved).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"mdoc path must be under mdoc root: {root_resolved}"
            ) from exc
