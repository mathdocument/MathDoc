import os
import sqlite3
from collections import defaultdict
from pathlib import Path, PurePosixPath

from .refresh import delete_indexed_path, upsert_mdoc_row


def rebuild_directory_index(*, root: Path, conn: sqlite3.Connection) -> None:
    conn.execute("DELETE FROM mdoc_dirs")
    for rel_dir, mtime_ns in _scan_workspace_dirs(root):
        conn.execute(
            """
            INSERT INTO mdoc_dirs (path, mtime_ns)
            VALUES (?, ?)
            ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns
            """,
            (rel_dir, mtime_ns),
        )


def discover_workspace_changes(*, root: Path, conn: sqlite3.Connection) -> None:
    known_dirs = _dir_mtimes(conn)
    known_file_states = _file_states(conn)
    child_dirs_by_parent = _group_dirs_by_parent(known_dirs)
    files_by_parent = _group_files_by_parent(known_file_states)
    seen_dirs: set[str] = set()

    def purge_subtree(rel_dir: str) -> None:
        prefix = f"{rel_dir}/" if rel_dir else ""
        stale_paths = [
            path
            for path in known_file_states
            if path == rel_dir or path.startswith(prefix)
        ]
        for stale_path in stale_paths:
            delete_indexed_path(conn, stale_path=stale_path)
            known_file_states.pop(stale_path, None)

        stale_dirs = [
            path
            for path in known_dirs
            if path == rel_dir or (prefix and path.startswith(prefix))
        ]
        if rel_dir == "":
            stale_dirs = list(known_dirs)
        for stale_dir in stale_dirs:
            conn.execute("DELETE FROM mdoc_dirs WHERE path = ?", (stale_dir,))
            known_dirs.pop(stale_dir, None)
            child_dirs_by_parent.pop(stale_dir, None)
            files_by_parent.pop(stale_dir, None)

    def scan_dir(rel_dir: str) -> None:
        dir_path = root if not rel_dir else root / rel_dir
        if rel_dir and (dir_path / ".mdc").is_dir():
            purge_subtree(rel_dir)
            return
        if not dir_path.is_dir():
            purge_subtree(rel_dir)
            return

        try:
            dir_stat = dir_path.stat()
        except OSError:
            purge_subtree(rel_dir)
            return

        seen_dirs.add(rel_dir)
        current_mtime_ns = int(dir_stat.st_mtime_ns)
        known_mtime_ns = known_dirs.get(rel_dir)
        changed = known_mtime_ns != current_mtime_ns

        if not changed and known_mtime_ns is not None:
            for child_dir in sorted(child_dirs_by_parent.get(rel_dir, set())):
                scan_dir(child_dir)
            return

        discovered_child_dirs: set[str] = set()
        seen_files: set[str] = set()

        try:
            entries = list(os.scandir(dir_path))
        except OSError:
            purge_subtree(rel_dir)
            return

        for entry in entries:
            if entry.name == ".mdc":
                continue
            child_rel = _join_rel_dir(rel_dir, entry.name)
            if entry.is_dir(follow_symlinks=False):
                discovered_child_dirs.add(child_rel)
                continue
            if not entry.is_file(follow_symlinks=False):
                continue
            if not entry.name.endswith(".mdoc"):
                continue
            seen_files.add(child_rel)

            known_state = known_file_states.get(child_rel)
            if not changed and known_state is not None:
                continue

            try:
                entry_stat = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            current_state = (int(entry_stat.st_mtime_ns), int(entry_stat.st_size))
            if known_state == current_state:
                continue
            upsert_mdoc_row(
                root=root,
                conn=conn,
                file_path=Path(entry.path),
                commit=False,
            )
            known_file_states[child_rel] = current_state

        for stale_path in sorted(files_by_parent.get(rel_dir, set()) - seen_files):
            delete_indexed_path(conn, stale_path=stale_path)
            known_file_states.pop(stale_path, None)

        for stale_dir in sorted(
            child_dirs_by_parent.get(rel_dir, set()) - discovered_child_dirs
        ):
            purge_subtree(stale_dir)

        conn.execute(
            """
            INSERT INTO mdoc_dirs (path, mtime_ns)
            VALUES (?, ?)
            ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns
            """,
            (rel_dir, current_mtime_ns),
        )
        known_dirs[rel_dir] = current_mtime_ns
        child_dirs_by_parent[rel_dir] = discovered_child_dirs
        files_by_parent[rel_dir] = seen_files

        for child_dir in sorted(child_dirs_by_parent.get(rel_dir, set())):
            scan_dir(child_dir)

    scan_dir("")

    for stale_dir in sorted(set(known_dirs) - seen_dirs, reverse=True):
        purge_subtree(stale_dir)


def _scan_workspace_dirs(root: Path) -> list[tuple[str, int]]:
    rows: list[tuple[str, int]] = []

    def walk_dir(dir_path: Path, *, rel_dir: str) -> None:
        if rel_dir and (dir_path / ".mdc").is_dir():
            return
        rows.append((rel_dir, int(dir_path.stat().st_mtime_ns)))
        try:
            entries = sorted(os.scandir(dir_path), key=lambda entry: entry.name)
        except OSError:
            return
        for entry in entries:
            if entry.name == ".mdc":
                continue
            if not entry.is_dir(follow_symlinks=False):
                continue
            walk_dir(Path(entry.path), rel_dir=_join_rel_dir(rel_dir, entry.name))

    walk_dir(root.resolve(), rel_dir="")
    return rows


def _dir_mtimes(conn: sqlite3.Connection) -> dict[str, int]:
    rows = conn.execute("SELECT path, mtime_ns FROM mdoc_dirs").fetchall()
    return {str(row[0]): int(row[1]) for row in rows}


def _file_states(conn: sqlite3.Connection) -> dict[str, tuple[int, int]]:
    rows = conn.execute("SELECT path, mtime_ns, size FROM mdoc_files").fetchall()
    return {str(row[0]): (int(row[1]), int(row[2])) for row in rows}


def _group_dirs_by_parent(dir_mtimes: dict[str, int]) -> dict[str, set[str]]:
    grouped: dict[str, set[str]] = defaultdict(set)
    for rel_dir in dir_mtimes:
        grouped.setdefault(rel_dir, set())
        if not rel_dir:
            continue
        grouped[_parent_dir(rel_dir)].add(rel_dir)
    grouped.setdefault("", set())
    return grouped


def _group_files_by_parent(
    file_states: dict[str, tuple[int, int]],
) -> dict[str, set[str]]:
    grouped: dict[str, set[str]] = defaultdict(set)
    for rel_path in file_states:
        grouped[_parent_dir(rel_path)].add(rel_path)
    grouped.setdefault("", set())
    return grouped


def _join_rel_dir(parent: str, name: str) -> str:
    return name if not parent else f"{parent}/{name}"


def _parent_dir(rel_path: str) -> str:
    parent = str(PurePosixPath(rel_path).parent)
    return "" if parent == "." else parent
