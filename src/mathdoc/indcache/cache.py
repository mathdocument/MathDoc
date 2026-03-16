import sqlite3
from contextlib import closing, contextmanager
from pathlib import Path
from typing import Iterator

from ..mdochead import read_mdoc_head
from .discovery import discover_workspace_changes
from .queries import (
    dependency_report as cached_dependency_report,
    global_root_items as cached_global_root_items,
    graph_check_report as cached_graph_check_report,
    issue_for_fnode as cached_issue_for_fnode,
    leaf_dependency_report as cached_leaf_dependency_report,
    ref_item_for_fnode as cached_ref_item_for_fnode,
    referrer_items as cached_referrer_items,
)
from .refresh import (
    refresh_indexed_paths,
    refresh_reachable_from_path as refresh_reachable_rows,
    refresh_rows as refresh_index_rows,
    refresh_search_index,
    upsert_mdoc_row,
)
from .schema import ensure_index_schema
from ..utils import find_nested_mdcroot


class IndCache:
    SCHEMA_VERSION = 5

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.db_path = self.root / ".mdc" / "index.db"

    @contextmanager
    def _open_conn(self) -> Iterator[sqlite3.Connection]:
        with closing(sqlite3.connect(self.db_path)) as conn:
            try:
                ensure_index_schema(
                    conn,
                    schema_version=self.SCHEMA_VERSION,
                )
                yield conn
                conn.commit()
            except Exception:
                conn.rollback()
                raise

    def bootstrap_if_needed(self) -> None:
        with self._open_conn() as conn:
            if self._bootstrap_required(conn):
                refresh_search_index(root=self.root, conn=conn)

    def refresh_all(self) -> None:
        with self._open_conn() as conn:
            refresh_search_index(root=self.root, conn=conn)

    def discover_workspace_changes(self) -> None:
        with self._open_conn() as conn:
            discover_workspace_changes(root=self.root, conn=conn)
            conn.execute("UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1")
            conn.commit()

    def refresh_workspace_index(self) -> None:
        with self._open_conn() as conn:
            discover_workspace_changes(root=self.root, conn=conn)
            refresh_indexed_paths(root=self.root, conn=conn)
            conn.execute("UPDATE mdoc_index_state SET bootstrapped = 1 WHERE id = 1")
            conn.commit()

    def count(self) -> int:
        with self._open_conn() as conn:
            row = conn.execute("SELECT COUNT(*) FROM mdocs").fetchone()
        return int(row[0]) if row else 0

    def upsert_path(self, file_path: Path) -> None:
        with self._open_conn() as conn:
            upsert_mdoc_row(
                root=self.root,
                conn=conn,
                file_path=file_path,
                commit=True,
            )

    def refresh_rows(self, rows: list[tuple[str, str, str]]) -> None:
        with self._open_conn() as conn:
            refresh_index_rows(root=self.root, conn=conn, rows=rows)
            conn.commit()

    def refresh_reachable_from_path(self, *, root_path: Path, depth: int) -> None:
        with self._open_conn() as conn:
            refresh_reachable_rows(
                root=self.root,
                conn=conn,
                root_path=root_path,
                depth=depth,
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

                head = read_mdoc_head(candidate)
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

    def indexed_file_count(self) -> int:
        with self._open_conn() as conn:
            row = conn.execute("SELECT COUNT(*) FROM mdoc_files").fetchone()
        return int(row[0]) if row else 0

    def knows_fnode(self, fnode: str) -> bool:
        with self._open_conn() as conn:
            row = conn.execute(
                "SELECT 1 FROM mdocs WHERE fnode = ? LIMIT 1", (fnode,)
            ).fetchone()
            if row is not None:
                return True
            row = conn.execute(
                "SELECT 1 FROM mdoc_issues WHERE ref_fnode = ? LIMIT 1",
                (fnode,),
            ).fetchone()
            return row is not None

    def issue_for_fnode(self, fnode: str):
        return cached_issue_for_fnode(self, fnode)

    def ref_item_for_fnode(self, fnode: str, *, depth: int = 0):
        return cached_ref_item_for_fnode(self, fnode, depth=depth)

    def referrer_items(self, *, target_fnode: str, depth: int):
        return cached_referrer_items(self, target_fnode=target_fnode, depth=depth)

    def dependency_report(self, *, root_fnode: str, depth: int):
        return cached_dependency_report(self, root_fnode=root_fnode, depth=depth)

    def leaf_dependency_report(self, *, root_fnode: str):
        return cached_leaf_dependency_report(self, root_fnode=root_fnode)

    def global_root_items(self):
        return cached_global_root_items(self)

    def graph_check_report(self):
        return cached_graph_check_report(self)

    @staticmethod
    def _bootstrap_required(conn: sqlite3.Connection) -> bool:
        row = conn.execute(
            "SELECT bootstrapped FROM mdoc_index_state WHERE id = 1"
        ).fetchone()
        return row is None or int(row[0]) == 0

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
