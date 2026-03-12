import argparse
import sqlite3
from pathlib import Path

from .mdocnode import MdocNode


def _find_mdoc_root(start: Path) -> Path | None:
    for candidate in [start, *start.parents]:
        if (candidate / ".mdc").is_dir():
            return candidate
    return None


def _iter_mdoc_files(root: Path):
    for file_path in root.rglob("*.mdoc"):
        if ".mdc" in file_path.parts:
            continue
        if file_path.is_file():
            yield file_path


def _index_db_path(root: Path) -> Path:
    return root / ".mdc" / "index.db"


def _ensure_index_schema(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS cards (
            fnode TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            title_lc TEXT NOT NULL,
            mtime_sec INTEGER NOT NULL,
            size INTEGER NOT NULL
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_cards_title_lc ON cards(title_lc)"
    )


def _read_card_head(file_path: Path) -> tuple[str, str] | None:
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

    if not title:
        return None
    return fnode, title


def _refresh_search_index(conn: sqlite3.Connection, root: Path) -> None:
    rows = conn.execute(
        "SELECT path, fnode, mtime_sec, size FROM cards"
    ).fetchall()
    cached_by_path = {
        row[0]: (row[1], row[2], row[3])
        for row in rows
    }

    seen_paths: set[str] = set()
    for file_path in _iter_mdoc_files(root):
        rel_path = file_path.relative_to(root).as_posix()
        seen_paths.add(rel_path)

        try:
            stat = file_path.stat()
        except OSError:
            continue

        mtime_sec = int(stat.st_mtime)
        size = int(stat.st_size)
        cached = cached_by_path.get(rel_path)
        if cached and cached[1] == mtime_sec and cached[2] == size:
            continue

        head = _read_card_head(file_path)
        if head is None:
            conn.execute("DELETE FROM cards WHERE path = ?", (rel_path,))
            continue

        fnode, title = head
        conn.execute(
            "DELETE FROM cards WHERE path = ? AND fnode != ?", (rel_path, fnode))
        conn.execute(
            """
            INSERT INTO cards (fnode, path, title, title_lc, mtime_sec, size)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(fnode) DO UPDATE SET
                path = excluded.path,
                title = excluded.title,
                title_lc = excluded.title_lc,
                mtime_sec = excluded.mtime_sec,
                size = excluded.size
            """,
            (fnode, rel_path, title, title.casefold(), mtime_sec, size),
        )

    stale_paths = set(cached_by_path.keys()) - seen_paths
    for stale_path in stale_paths:
        conn.execute("DELETE FROM cards WHERE path = ?", (stale_path,))

    conn.commit()


def _search_cards(conn: sqlite3.Connection, query: str) -> list[tuple[str, str, str]]:
    query_lc = query.casefold()
    like = f"%{query_lc}%"
    rows = conn.execute(
        """
        SELECT fnode, title, path
        FROM cards
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


def _cmd_init(_: argparse.Namespace) -> int:
    local_mdc = Path.cwd() / ".mdc"

    if local_mdc.is_dir():
        print(f"Already initialized as mdoc directory: {local_mdc}")
        return 0

    local_mdc.mkdir(parents=False, exist_ok=False)
    (local_mdc / "config.toml").touch(exist_ok=True)
    print("mdoc folder initialized")
    return 0


def _cmd_new(args: argparse.Namespace) -> int:
    mdoc_root = _find_mdoc_root(Path.cwd())
    if mdoc_root is None:
        print("Error: not inside an mdoc directory, run `mdc init` first")
        return 1

    target = Path(args.folder).resolve()
    try:
        target.relative_to(mdoc_root.resolve())
    except ValueError:
        print(
            f"Error: target path must be under mdoc root {mdoc_root}")
        return 1

    if target.exists() and not target.is_dir():
        print(f"Error: target folder is a file: {target}")
        return 1

    node = MdocNode.create(args.folder, args.title)
    try:
        node.save()
    except OSError as exc:
        print(f"Error: failed to save mdoc file: {exc}")
        return 1

    print(f"created: {node.path}")
    print(f"fnode: {node.fnode}")
    print(f"title: {node.title}")
    return 0


def _cmd_search(args: argparse.Namespace) -> int:
    mdoc_root = _find_mdoc_root(Path.cwd())
    if mdoc_root is None:
        print("Error: not inside an mdoc directory, run `mdc init` first")
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1

    db_path = _index_db_path(mdoc_root)
    with sqlite3.connect(db_path) as conn:
        _ensure_index_schema(conn)
        _refresh_search_index(conn, mdoc_root)
        matches = _search_cards(conn, query)

    if not matches:
        print(f"No results for: {args.query}")
        return 0

    for fnode, title, _ in matches:
        print(f"{fnode:.8}\t{title}")

    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdc", description="MathDoc CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser(
        "init", help="Initialize a new MathDoc folder")
    init_parser.set_defaults(func=_cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument("-t", "--title", default="Untitled",
                            help="Title of the new card (optional)")
    new_parser.add_argument("-f", "--folder", default=".",
                            help="Output folder for the card file (optional)")
    new_parser.set_defaults(func=_cmd_new)

    search_parser = subparsers.add_parser(
        "search", help="Search cards by title")
    search_parser.add_argument("query", help="Title query")
    search_parser.set_defaults(func=_cmd_search)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
