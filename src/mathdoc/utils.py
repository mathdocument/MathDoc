import os
import select
import shutil
import sys
import termios
import tty
from pathlib import Path

from .indcache import IndCache
from .mdocnode import MdocNode

STYLE: dict[str, str] = {
    # ANSI style/color codes (aligned with user's shell env naming)
    "rst": "\033[0m",
    "bld": "\033[1m",
    "udl": "\033[4m",
    "blk": "\033[30m",
    "wht": "\033[37m",
    "red": "\033[31m",
    "org": "\033[38;5;208m",
    "ylw": "\033[33m",
    "grn": "\033[32m",
    "cyn": "\033[36m",
    "blu": "\033[34m",
    "ppl": "\033[35m",
    # Nerd font symbols
    "glyph_unselected": "\U000F0131",
    "glyph_selected": "\U000F0C52",
}


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


def colorize(text: str, *codes: str, enabled: bool | None = None) -> str:
    if enabled is None:
        enabled = _supports_color() and sys.stdout.isatty()
    if not enabled or not codes:
        return text
    return "".join(codes) + text + STYLE["rst"]


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

    if ch == b"\033":
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
        out.append(f"\033[{prev_count}A")

    total = max(prev_count, len(lines))
    for idx in range(total):
        out.append("\r\033[2K")
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
    out: list[str] = [f"\033[{prev_count}A", "\r", f"\033[{prev_count}M"]
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
        sys.stdout.write("\033[?25l")
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
                    colorize(
                        header_title, STYLE["bld"], STYLE["cyn"], enabled=True
                    )
                    + colorize(header_hint, STYLE["wht"], enabled=True)
                )
            else:
                lines.append(header_plain)

            end = min(len(matches), top + max_visible)
            for item_index in range(top, end):
                fnode, title, path = matches[item_index]
                marker = ">>" if item_index == current else "  "
                checked = (
                    STYLE["glyph_selected"]
                    if item_index in selected
                    else STYLE["glyph_unselected"]
                )
                raw_line = f"{marker} {item_index + 1:>3}. {checked} {short_fnode(fnode)}  {title}  ({path})"
                line = _clip(raw_line, width)
                base_codes: tuple[str, ...] = (
                    STYLE["blu"],) if item_index in selected else ()
                if item_index == current:
                    lines.append(
                        colorize(
                            line, STYLE["bld"], *base_codes, enabled=use_color
                        )
                    )
                else:
                    lines.append(
                        colorize(line, *base_codes, enabled=use_color))

            if len(matches) > max_visible:
                lines.append(
                    colorize(
                        _clip(
                            f"showing {top + 1}-{end} of {len(matches)}", width),
                        STYLE["blu"],
                        enabled=use_color,
                    )
                )

            footer = _clip(
                f"selected {len(selected)}/{len(matches)}",
                width,
            )
            lines.append(
                colorize(
                    footer, STYLE["bld"], STYLE["cyn"], enabled=use_color
                )
            )

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
        sys.stdout.write("\033[?25h")
        sys.stdout.flush()
