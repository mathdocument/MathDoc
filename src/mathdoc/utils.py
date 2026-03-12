import os
import select
import shutil
import sys
import termios
import tty
from pathlib import Path

from .indcache import IndCache
from .mdocnode import MdocNode

ANSI_RESET = "\x1b[0m"
ANSI_BOLD = "\x1b[1m"
FG_BLACK = "\x1b[30m"
FG_WHITE = "\x1b[37m"
FG_RED = "\x1b[31m"
FG_ORANGE = "\x1b[38;5;208m"
FG_YELLOW = "\x1b[33m"
FG_GREEN = "\x1b[32m"
FG_CYAN = "\x1b[36m"
FG_BLUE = "\x1b[34m"
FG_PURPLE = "\x1b[35m"
GLYPH_UNSELECTED = "\U000F0131"
GLYPH_SELECTED = "\U000F0136"


def short_fnode(fnode: str) -> str:
    return fnode[:8]


def to_rel_path(root: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(path)


def format_mdoc_item(fnode: str, title: str, path: str, marker: str = "-") -> str:
    prefix = f"{marker} " if marker else ""
    return f"{prefix}{short_fnode(fnode)}\t{title} ({path})"


def warn_index_failure(action: str, exc: Exception) -> None:
    print(f"Warning: {action}, but index refresh failed: {exc}")
    print("Warning: search results may be stale, run `mdc sync` to rebuild the index.")


def find_mdoc_root(start: Path) -> Path | None:
    for candidate in [start, *start.parents]:
        if (candidate / ".mdc").is_dir():
            return candidate
    return None


def load_mdoc_from_ref(cache: IndCache, ref: str) -> tuple[MdocNode, str]:
    _, _, src_path = cache.resolve_ref(ref, cwd=Path.cwd())
    node = MdocNode(path=src_path, title="")
    node.load()
    return node, to_rel_path(cache.root, src_path)


def _supports_color() -> bool:
    term = os.environ.get("TERM", "")
    return term not in ("", "dumb") and os.environ.get("NO_COLOR") is None


def _style(text: str, codes: tuple[str, ...], enabled: bool) -> str:
    if not enabled or not codes:
        return text
    return "".join(codes) + text + ANSI_RESET


def _read_next_byte(fd: int, timeout_sec: float) -> bytes:
    ready, _, _ = select.select([fd], [], [], timeout_sec)
    if not ready:
        return b""
    return os.read(fd, 1)


def _read_key(fd: int) -> str:
    ch = os.read(fd, 1)
    if not ch:
        return ""
    if ch in (b"\r", b"\n"):
        return "enter"
    if ch == b" ":
        return "space"

    lower = ch.lower()
    keymap = {
        b"q": "quit",
        b"a": "all",
        b"c": "clear",
        b"j": "down",
        b"k": "up",
        b"n": "pagedown",
        b"p": "pageup",
    }
    if lower in keymap:
        return keymap[lower]

    if ch == b"\x1b":
        second = _read_next_byte(fd, 0.01)
        if second != b"[":
            return "quit"
        third = _read_next_byte(fd, 0.01)
        if third == b"A":
            return "up"
        if third == b"B":
            return "down"
        if third == b"5":
            _ = _read_next_byte(fd, 0.01)
            return "pageup"
        if third == b"6":
            _ = _read_next_byte(fd, 0.01)
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


def select_indices_interactive(matches: list[tuple[str, str, str]]) -> list[int] | None:
    if not matches:
        return []

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError("interactive selection requires a TTY")

    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    use_color = _supports_color()

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
            max_visible = max(1, min(len(matches), 10, term_size.lines - 6))

            if current < top:
                top = current
            elif current >= top + max_visible:
                top = current - max_visible + 1

            lines: list[str] = []
            header_plain = _clip(
                "Select deps: up/down(j/k) space toggle a all c clear enter confirm q cancel",
                width,
            )
            if use_color and header_plain.startswith("Select deps:"):
                header_title = "Select deps:"
                header_hint = header_plain[len(header_title):]
                lines.append(
                    _style(header_title, (ANSI_BOLD, FG_CYAN), True)
                    + _style(header_hint, (FG_WHITE,), True)
                )
            else:
                lines.append(header_plain)

            end = min(len(matches), top + max_visible)
            for item_index in range(top, end):
                fnode, title, path = matches[item_index]
                marker = ">>" if item_index == current else "  "
                checked = GLYPH_SELECTED if item_index in selected else GLYPH_UNSELECTED
                raw_line = f"{marker} {item_index + 1:>3}. {checked} {short_fnode(fnode)}  {title}  ({path})"
                line = _clip(raw_line, width)
                base_codes: tuple[str, ...] = (FG_BLUE,) if item_index in selected else ()
                if item_index == current:
                    lines.append(_style(line, (ANSI_BOLD, *base_codes), use_color))
                else:
                    lines.append(_style(line, base_codes, use_color))

            if len(matches) > max_visible:
                lines.append(
                    _style(
                        _clip(
                            f"showing {top + 1}-{end} of {len(matches)}", width),
                        (FG_BLUE,),
                        use_color,
                    )
                )

            footer = _clip(
                f"selected {len(selected)}/{len(matches)}",
                width,
            )
            lines.append(_style(footer, (ANSI_BOLD, FG_CYAN), use_color))

            rendered_lines = _render_block(lines, rendered_lines)

            key = _read_key(fd)
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
