import os
import select
import shutil
import sys
import termios
import tty

from .models import NodeRef
from .theme import STYLE, colorize, short_fnode, supports_color


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
    out: list[str] = [f"\033[{prev_count}A", "\r", f"\033[{prev_count}M"]
    sys.stdout.write("".join(out))
    sys.stdout.flush()


def _require_tty() -> None:
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError("interactive prompt requires a TTY")


def _render_confirm_prompt(prompt: str, suffix: str, *, use_color: bool) -> str:
    if not use_color:
        return f"{prompt} {suffix}: "
    return (
        colorize(prompt, STYLE["bld"], STYLE["cyn"], enabled=True)
        + " "
        + colorize(suffix, STYLE["dim"], STYLE["blu"], enabled=True)
        + ": "
    )


def _render_text_prompt(
    prompt: str,
    *,
    default_display: str,
    use_color: bool,
) -> str:
    if not use_color:
        return f"{prompt} [{default_display}]: "
    return (
        colorize(prompt, STYLE["bld"], STYLE["cyn"], enabled=True)
        + colorize(f" [{default_display}]", STYLE["dim"], STYLE["blu"], enabled=True)
        + ": "
    )


def _confirm_interactive_with_count(
    prompt: str,
    *,
    default: bool = False,
) -> tuple[bool, int]:
    _require_tty()
    use_color = supports_color()
    suffix = "[Y/n]" if default else "[y/N]"
    printed_lines = 0

    while True:
        sys.stdout.write(_render_confirm_prompt(prompt, suffix, use_color=use_color))
        sys.stdout.flush()
        raw = sys.stdin.readline()
        printed_lines += 1
        if raw == "":
            return default, printed_lines

        value = raw.strip().casefold()
        if not value:
            return default, printed_lines
        if value in {"y", "yes"}:
            return True, printed_lines
        if value in {"n", "no"}:
            return False, printed_lines

        sys.stdout.write("Please answer y or n.\n")
        sys.stdout.flush()
        printed_lines += 1


def confirm_interactive(prompt: str, *, default: bool = False) -> bool:
    value, _ = _confirm_interactive_with_count(prompt, default=default)
    return value


def _prompt_text_interactive_with_count(
    prompt: str,
    *,
    default: str = "",
    default_display: str | None = None,
) -> tuple[str, int]:
    _require_tty()
    use_color = supports_color()
    rendered_default = default if default_display is None else default_display

    sys.stdout.write(
        _render_text_prompt(
            prompt,
            default_display=rendered_default,
            use_color=use_color,
        )
    )
    sys.stdout.flush()
    raw = sys.stdin.readline()
    printed_lines = 1
    if raw == "":
        return default, printed_lines

    value = raw.rstrip("\r\n")
    return (value if value else default), printed_lines


def prompt_text_interactive(
    prompt: str,
    *,
    default: str = "",
    default_display: str | None = None,
) -> str:
    value, _ = _prompt_text_interactive_with_count(
        prompt,
        default=default,
        default_display=default_display,
    )
    return value


def prompt_new_mdoc_interactive(
    *,
    default_filename_display: str,
    default_title: str = "Untitled",
) -> tuple[str, str] | None:
    printed_lines = 0
    try:
        should_create, used_lines = _confirm_interactive_with_count(
            "No dependency candidates found. Create a new mdoc and add it as a dependency",
            default=False,
        )
        printed_lines += used_lines
        if not should_create:
            return None

        filename, used_lines = _prompt_text_interactive_with_count(
            "Filename",
            default=".",
            default_display=default_filename_display,
        )
        printed_lines += used_lines

        title, used_lines = _prompt_text_interactive_with_count(
            "Title",
            default=default_title,
        )
        printed_lines += used_lines
        return (filename, title)
    finally:
        _clear_block(printed_lines)


def select_indices_interactive(
    matches: list[NodeRef],
    *,
    error_indices: set[int] | None = None,
) -> list[int] | None:
    if not matches:
        return []

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError("interactive selection requires a TTY")

    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    use_color = supports_color()

    current = 0
    top = 0
    selected: set[int] = set()
    bad_rows = error_indices or set()
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
                header_hint = header_plain[len(header_title) :]
                lines.append(
                    colorize(header_title, STYLE["bld"], STYLE["cyn"], enabled=True)
                    + colorize(header_hint, STYLE["wht"], enabled=True)
                )
            else:
                lines.append(header_plain)

            end = min(len(matches), top + max_visible)
            for item_index in range(top, end):
                item = matches[item_index]
                marker = ">>" if item_index == current else "  "
                checked = (
                    STYLE["glyph_selected"]
                    if item_index in selected
                    else STYLE["glyph_unselected"]
                )
                raw_line = (
                    f"{marker} {item_index + 1:>3}. {checked} "
                    f"{short_fnode(item.fnode)}  {item.title}  ({item.rel_path})"
                )
                line = _clip(raw_line, width)
                if item_index in bad_rows:
                    if item_index == current:
                        lines.append(
                            colorize(
                                line, STYLE["bld"], STYLE["red"], enabled=use_color
                            )
                        )
                    else:
                        lines.append(colorize(line, STYLE["red"], enabled=use_color))
                else:
                    base_codes: tuple[str, ...] = (
                        (STYLE["blu"],) if item_index in selected else ()
                    )
                    if item_index == current:
                        lines.append(
                            colorize(line, STYLE["bld"], *base_codes, enabled=use_color)
                        )
                    else:
                        lines.append(colorize(line, *base_codes, enabled=use_color))

            if len(matches) > max_visible:
                lines.append(
                    colorize(
                        _clip(f"showing {top + 1}-{end} of {len(matches)}", width),
                        STYLE["blu"],
                        enabled=use_color,
                    )
                )

            footer = _clip(f"selected {len(selected)}/{len(matches)}", width)
            lines.append(
                colorize(footer, STYLE["bld"], STYLE["cyn"], enabled=use_color)
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
                selected = set(range(len(matches)))
            elif key == "clear":
                selected.clear()
            elif key == "enter":
                return sorted(selected)
            elif key == "quit":
                return None
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)
        _clear_block(rendered_lines)
        sys.stdout.write("\033[?25h")
        sys.stdout.flush()
