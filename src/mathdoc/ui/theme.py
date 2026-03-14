import os
import sys


STYLE: dict[str, str] = {
    "rst": "\033[0m",
    "bld": "\033[1m",
    "dim": "\033[2m",
    "udl": "\033[4m",
    "blk": "\033[30m",
    "wht": "\033[37m",
    "red": "\033[38;5;196m",
    "org": "\033[38;5;208m",
    "ylw": "\033[33m",
    "grn": "\033[38;5;41m",
    "cyn": "\033[38;5;38m",
    "blu": "\033[38;5;68m",
    "ppl": "\033[35m",
    "glyph_unselected": "\U000f0131",
    "glyph_selected": "\U000f0c52",
}


LAYOUT: dict[str, int] = {
    "ref_fnode_width": 10,
    "ref_title_width": 28,
}


def short_fnode(fnode: str) -> str:
    if fnode.startswith("<") and fnode.endswith(">"):
        return fnode
    return fnode[:8]


def supports_color() -> bool:
    term = os.environ.get("TERM", "")
    return term not in ("", "dumb") and os.environ.get("NO_COLOR") is None


def colorize(text: str, *codes: str, enabled: bool | None = None) -> str:
    if enabled is None:
        enabled = supports_color() and sys.stdout.isatty()
    if not enabled or not codes:
        return text
    return "".join(codes) + text + STYLE["rst"]
