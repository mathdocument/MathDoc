import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator


class IndCache:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.db_path = self.root / ".mdc" / "index.db"

    @contextmanager
    def _open_conn(self) -> Iterator[sqlite3.Connection]:
        with sqlite3.connect(self.db_path) as conn:
            self._ensure_index_schema(conn)
            yield conn

    def bootstrap_if_needed(self) -> None:
        with self._open_conn() as conn:
            if self._index_is_empty(conn):
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

    def resolve_ref(
        self, ref: str, *, cwd: Path | None = None
    ) -> tuple[str, str, Path]:
        raw_ref = ref.strip()
        if not raw_ref:
            raise ValueError("mdoc reference cannot be empty")

        base_cwd = (cwd or Path.cwd()).resolve()
        root_resolved = self.root.resolve()
        maybe_path = (
            ("/" in raw_ref) or raw_ref.endswith(".mdoc") or raw_ref.startswith(".")
        )
        with self._open_conn() as conn:
            if maybe_path:
                raw_path = Path(raw_ref)
                candidates: list[Path] = []
                if raw_path.is_absolute():
                    candidates.append(raw_path.resolve())
                else:
                    candidates.append((base_cwd / raw_path).resolve())
                    candidates.append((self.root / raw_path).resolve())

                seen: set[Path] = set()
                for candidate in candidates:
                    if candidate in seen:
                        continue
                    seen.add(candidate)

                    if not candidate.is_file():
                        continue

                    try:
                        rel_path = candidate.relative_to(root_resolved).as_posix()
                    except ValueError as exc:
                        raise ValueError(
                            f"mdoc path must be under mdoc root: {root_resolved}"
                        ) from exc

                    row = conn.execute(
                        "SELECT fnode, title FROM mdocs WHERE path = ?", (rel_path,)
                    ).fetchone()
                    if row is not None:
                        return str(row[0]), str(row[1]), candidate

                    head = self._read_mdoc_head(candidate)
                    if head is None or not head[0]:
                        raise ValueError(f"invalid mdoc file: {candidate}")
                    return str(head[0]), str(head[1]), candidate

                if raw_path.suffix == ".mdoc":
                    raise ValueError(f"mdoc file not found: {raw_ref}")

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
            row = exact_rows[0]
        elif len(rows) == 1:
            row = rows[0]
        else:
            preview = ", ".join(f"{str(r[0])[:8]}:{r[1]}" for r in rows[:5])
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
        root_resolved = self.root.resolve()
        maybe_path = (
            ("/" in raw_ref) or raw_ref.endswith(".mdoc") or raw_ref.startswith(".")
        )
        if maybe_path:
            raw_path = Path(raw_ref)
            candidates: list[Path] = []
            if raw_path.is_absolute():
                candidates.append(raw_path.resolve())
            else:
                candidates.append((base_cwd / raw_path).resolve())
                candidates.append((self.root / raw_path).resolve())

            seen: set[Path] = set()
            for candidate in candidates:
                if candidate in seen:
                    continue
                seen.add(candidate)
                if not candidate.is_file():
                    continue
                try:
                    candidate.relative_to(root_resolved)
                except ValueError as exc:
                    raise ValueError(
                        f"mdoc path must be under mdoc root: {root_resolved}"
                    ) from exc
                return candidate

            if raw_path.suffix == ".mdoc":
                raise ValueError(f"mdoc file not found: {raw_ref}")

        _, _, resolved = self.resolve_ref(raw_ref, cwd=base_cwd)
        return resolved

    def lookup_by_fnode(self, fnodes: list[str]) -> dict[str, tuple[str, str]]:
        if not fnodes:
            return {}

        rows_by_fnode: dict[str, tuple[str, str]] = {}
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
                    rows_by_fnode[str(row[0])] = (str(row[1]), str(row[2]))
        return rows_by_fnode

    def dep_rows(self, depens: list[str]) -> list[tuple[str, str, str]]:
        dep_meta = self.lookup_by_fnode(depens)
        rows: list[tuple[str, str, str]] = []
        for dep_fnode in depens:
            title, path = dep_meta.get(dep_fnode, ("<missing>", "<not indexed>"))
            rows.append((dep_fnode, title, path))
        return rows

    @staticmethod
    def _ensure_index_schema(conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS mdocs (
                fnode TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                title_lc TEXT NOT NULL,
                mtime_sec INTEGER NOT NULL,
                mtime_ns INTEGER NOT NULL,
                size INTEGER NOT NULL
            )
            """
        )
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

        conn.execute("CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc)")

    def _iter_mdoc_files(self) -> Iterator[Path]:
        for file_path in self.root.rglob("*.mdoc"):
            if ".mdc" in file_path.parts:
                continue
            if file_path.is_file():
                yield file_path

    @staticmethod
    def _read_mdoc_head(file_path: Path) -> tuple[str, str] | None:
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

    def _refresh_search_index(self, conn: sqlite3.Connection) -> None:
        rows = conn.execute("SELECT path, fnode, mtime_ns, size FROM mdocs").fetchall()
        cached_by_path = {
            str(row[0]): (str(row[1]), int(row[2]), int(row[3])) for row in rows
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
            if cached and cached[1] == mtime_ns and cached[2] == size:
                continue

            head = self._read_mdoc_head(file_path)
            if head is None:
                conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
                continue

            fnode, title = head
            conn.execute(
                "DELETE FROM mdocs WHERE path = ? AND fnode != ?", (rel_path, fnode)
            )
            conn.execute(
                """
                INSERT INTO mdocs (fnode, path, title, title_lc, mtime_sec, mtime_ns, size)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(fnode) DO UPDATE SET
                    path = excluded.path,
                    title = excluded.title,
                    title_lc = excluded.title_lc,
                    mtime_sec = excluded.mtime_sec,
                    mtime_ns = excluded.mtime_ns,
                    size = excluded.size
                """,
                (
                    fnode,
                    rel_path,
                    title,
                    title.casefold(),
                    mtime_ns // 1_000_000_000,
                    mtime_ns,
                    size,
                ),
            )

        stale_paths = set(cached_by_path.keys()) - seen_paths
        for stale_path in stale_paths:
            conn.execute("DELETE FROM mdocs WHERE path = ?", (stale_path,))

        conn.commit()

    def _upsert_mdoc_row(
        self,
        conn: sqlite3.Connection,
        file_path: Path,
        *,
        commit: bool,
    ) -> None:
        try:
            rel_path = file_path.resolve().relative_to(self.root.resolve()).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"mdoc path must be under mdoc root: {self.root.resolve()}"
            ) from exc

        if not file_path.is_file():
            conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
            if commit:
                conn.commit()
            return

        try:
            stat = file_path.stat()
        except OSError:
            conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
            if commit:
                conn.commit()
            return

        head = self._read_mdoc_head(file_path)
        if head is None:
            conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
            if commit:
                conn.commit()
            return

        fnode, title = head
        conn.execute(
            "DELETE FROM mdocs WHERE path = ? AND fnode != ?", (rel_path, fnode)
        )
        conn.execute(
            """
            INSERT INTO mdocs (fnode, path, title, title_lc, mtime_sec, mtime_ns, size)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(fnode) DO UPDATE SET
                path = excluded.path,
                title = excluded.title,
                title_lc = excluded.title_lc,
                mtime_sec = excluded.mtime_sec,
                mtime_ns = excluded.mtime_ns,
                size = excluded.size
            """,
            (
                fnode,
                rel_path,
                title,
                title.casefold(),
                int(stat.st_mtime),
                int(stat.st_mtime_ns),
                int(stat.st_size),
            ),
        )
        if commit:
            conn.commit()
