import argparse
import os
import select
import shutil
import sqlite3
import sys
import termios
import tty
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
        CREATE TABLE IF NOT EXISTS mdocs (
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
        "CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc)"
    )


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

    if not title:
        return None
    return fnode, title


def _refresh_search_index(conn: sqlite3.Connection, root: Path) -> None:
    rows = conn.execute(
        "SELECT path, fnode, mtime_sec, size FROM mdocs"
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

        head = _read_mdoc_head(file_path)
        if head is None:
            conn.execute("DELETE FROM mdocs WHERE path = ?", (rel_path,))
            continue

        fnode, title = head
        conn.execute(
            "DELETE FROM mdocs WHERE path = ? AND fnode != ?", (rel_path, fnode))
        conn.execute(
            """
            INSERT INTO mdocs (fnode, path, title, title_lc, mtime_sec, size)
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
        conn.execute("DELETE FROM mdocs WHERE path = ?", (stale_path,))

    conn.commit()


def _search_mdocs(conn: sqlite3.Connection, query: str) -> list[tuple[str, str, str]]:
    query_lc = query.casefold()
    like = f"%{query_lc}%"
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


def _resolve_mdoc_by_ref(
    conn: sqlite3.Connection, root: Path, ref: str
) -> tuple[str, str, Path]:
    raw_ref = ref.strip()
    if not raw_ref:
        raise ValueError("mdoc reference cannot be empty")

    root_resolved = root.resolve()
    maybe_path = (
        "/" in raw_ref) or raw_ref.endswith(".mdoc") or raw_ref.startswith(".")
    if maybe_path:
        raw_path = Path(raw_ref)
        candidates: list[Path] = []
        if raw_path.is_absolute():
            candidates.append(raw_path.resolve())
        else:
            candidates.append((Path.cwd() / raw_path).resolve())
            candidates.append((root / raw_path).resolve())

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

            head = _read_mdoc_head(candidate)
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
            f"ambiguous mdoc reference '{raw_ref}', matches: {preview}")

    rel_path = str(row[2])
    return str(row[0]), str(row[1]), root / rel_path


def _select_indices_interactive(matches: list[tuple[str, str, str]]) -> list[int] | None:
    if not matches:
        return []

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError("interactive selection requires a TTY")

    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)

    def _read_next_byte(timeout_sec: float) -> bytes:
        ready, _, _ = select.select([fd], [], [], timeout_sec)
        if not ready:
            return b""
        return os.read(fd, 1)

    def _read_key() -> str:
        ch = os.read(fd, 1)
        if not ch:
            return ""
        if ch in (b"\r", b"\n"):
            return "enter"
        if ch == b" ":
            return "space"
        if ch in (b"q", b"Q"):
            return "quit"
        if ch in (b"a", b"A"):
            return "all"
        if ch in (b"c", b"C"):
            return "clear"
        if ch in (b"j", b"J"):
            return "down"
        if ch in (b"k", b"K"):
            return "up"
        if ch in (b"n", b"N"):
            return "pagedown"
        if ch in (b"p", b"P"):
            return "pageup"
        if ch == b"\x1b":
            second = _read_next_byte(0.01)
            if second != b"[":
                return "quit"
            third = _read_next_byte(0.01)
            if third == b"A":
                return "up"
            if third == b"B":
                return "down"
            if third == b"5":
                _ = _read_next_byte(0.01)
                return "pageup"
            if third == b"6":
                _ = _read_next_byte(0.01)
                return "pagedown"
            return ""
        return ""

    def _clip(text: str, width: int) -> str:
        if width <= 0:
            return ""
        if len(text) <= width:
            return text
        if width <= 3:
            return text[:width]
        return text[: width - 3] + "..."

    def _render_block(lines: list[str], prev_count: int) -> int:
        out: list[str] = []
        if prev_count > 0:
            out.append(f"\x1b[{prev_count}A")

        total = max(prev_count, len(lines))
        for idx in range(total):
            out.append("\r\x1b[2K")
            if idx < len(lines):
                out.append(lines[idx])
            out.append("\n")

        sys.stdout.write("".join(out))
        sys.stdout.flush()
        return len(lines)

    def _clear_block(prev_count: int) -> None:
        if prev_count <= 0:
            return
        # Move to the block start and delete those lines so terminal content
        # below shifts up instead of leaving cleared blank lines.
        out: list[str] = [f"\x1b[{prev_count}A", "\r", f"\x1b[{prev_count}M"]
        sys.stdout.write("".join(out))
        sys.stdout.flush()

    current = 0
    top = 0
    selected: set[int] = set()
    rendered_lines = 0

    try:
        tty.setraw(fd)
        sys.stdout.write("\x1b[?25l")
        sys.stdout.flush()

        while True:
            term_size = shutil.get_terminal_size(fallback=(120, 30))
            width = max(20, term_size.columns)
            max_visible = max(1, min(len(matches), 10, term_size.lines - 5))

            if current < top:
                top = current
            elif current >= top + max_visible:
                top = current - max_visible + 1

            lines: list[str] = []
            lines.append(
                _clip(
                    "Select deps: Up/Down(j/k), Space toggle, a all, c clear, Enter confirm, q cancel",
                    width,
                )
            )

            end = min(len(matches), top + max_visible)
            for item_index in range(top, end):
                fnode, title, path = matches[item_index]
                marker = ">" if item_index == current else " "
                checked = "[x]" if item_index in selected else "[ ]"
                raw_line = f"{marker} {item_index + 1:>3}. {checked} {fnode[:8]}  {title}  ({path})"
                line = _clip(raw_line, width)
                if item_index == current:
                    lines.append(f"\x1b[7m{line}\x1b[0m")
                else:
                    lines.append(line)

            if len(matches) > max_visible:
                lines.append(
                    _clip(f"showing {top + 1}-{end} of {len(matches)}", width))
            lines.append(
                _clip(f"{len(selected)} selected / {len(matches)}", width))

            rendered_lines = _render_block(lines, rendered_lines)

            key = _read_key()
            if key == "up":
                current = max(0, current - 1)
            elif key == "down":
                current = min(len(matches) - 1, current + 1)
            elif key == "pageup":
                current = max(0, current - max_visible)
            elif key == "pagedown":
                current = min(len(matches) - 1, current + max_visible)
            elif key == "space":
                if current in selected:
                    selected.remove(current)
                else:
                    selected.add(current)
            elif key == "all":
                if len(selected) == len(matches):
                    selected.clear()
                else:
                    selected = set(range(len(matches)))
            elif key == "clear":
                selected.clear()
            elif key == "enter":
                _clear_block(rendered_lines)
                return sorted(selected)
            elif key == "quit":
                _clear_block(rendered_lines)
                return None
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)
        sys.stdout.write("\x1b[?25h")
        sys.stdout.flush()


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
        matches = _search_mdocs(conn, query)

    if not matches:
        print(f"No results for: {args.query}")
        return 0

    for fnode, title, rel_path in matches:
        print(f"{fnode:.8}  {title} ({rel_path})")

    return 0


def _cmd_dep_add(args: argparse.Namespace) -> int:
    mdoc_root = _find_mdoc_root(Path.cwd())
    if mdoc_root is None:
        print("Error: not inside an mdoc directory, run `mdc init` first")
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1
    if args.max_results < 1:
        print("Error: --max-results must be >= 1")
        return 1

    db_path = _index_db_path(mdoc_root)
    with sqlite3.connect(db_path) as conn:
        _ensure_index_schema(conn)
        _refresh_search_index(conn, mdoc_root)
        try:
            src_fnode, _, src_path = _resolve_mdoc_by_ref(
                conn, mdoc_root, args.source)
        except ValueError as exc:
            print(f"Error: {exc}")
            return 1
        matches = _search_mdocs(conn, query)

    matches = [row for row in matches if row[0] != src_fnode]
    matches = matches[:args.max_results]
    if not matches:
        print(f"No dependency candidates for: {args.query}")
        return 0

    try:
        selected_indices = _select_indices_interactive(matches)
    except RuntimeError as exc:
        print(f"Error: {exc}")
        return 1

    if selected_indices is None:
        print("Canceled")
        return 0
    if not selected_indices:
        print("No dependencies selected")
        return 0

    selected_rows = [matches[idx] for idx in selected_indices]
    selected_by_fnode = {row[0]: row for row in selected_rows}

    node = MdocNode(path=src_path, title="")
    try:
        node.load()
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    added: list[str] = []
    skipped_existing: list[str] = []
    skipped_self: list[str] = []
    for dep_fnode in selected_by_fnode:
        if dep_fnode == node.fnode:
            skipped_self.append(dep_fnode)
            continue
        if dep_fnode in node.depens:
            skipped_existing.append(dep_fnode)
            continue
        node.add_dependency(dep_fnode)
        added.append(dep_fnode)

    if added:
        try:
            node.save()
        except OSError as exc:
            print(f"Error: failed to save mdoc: {exc}")
            return 1

    print(f"source: {node.fnode[:8]}\t{node.title}")
    print(f"added: {len(added)}")
    for dep_fnode in added:
        dep_row = selected_by_fnode[dep_fnode]
        print(f"+ {dep_fnode[:8]}\t{dep_row[1]}")
    if skipped_existing:
        print(f"skipped existing: {len(skipped_existing)}")
    if skipped_self:
        print(f"skipped self: {len(skipped_self)}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdc", description="MathDoc CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser(
        "init", help="Initialize a new MathDoc folder")
    init_parser.set_defaults(func=_cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument("-t", "--title", default="Untitled",
                            help="Title of the new mdoc (optional)")
    new_parser.add_argument("-f", "--folder", default=".",
                            help="Output folder for the mdoc file (optional)")
    new_parser.set_defaults(func=_cmd_new)

    search_parser = subparsers.add_parser(
        "search", help="Search mdocs by title or fnode")
    search_parser.add_argument("query", help="Query by title or fnode")
    search_parser.set_defaults(func=_cmd_search)

    dep_parser = subparsers.add_parser("dep", help="Manage mdoc dependencies")
    dep_subparsers = dep_parser.add_subparsers(
        dest="dep_command", required=True)

    dep_add_parser = dep_subparsers.add_parser(
        "add", help="Search and add dependencies to a mdoc")
    dep_add_parser.add_argument(
        "source",
        help="Source modc to modify (fnode or .mdoc path)",
    )
    dep_add_parser.add_argument(
        "query",
        help="Search query for dependency mdocs",
    )
    dep_add_parser.add_argument(
        "-n",
        "--max-results",
        type=int,
        default=200,
        help="Maximum dependency candidates to show (default: 200)",
    )
    dep_add_parser.set_defaults(func=_cmd_dep_add)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
