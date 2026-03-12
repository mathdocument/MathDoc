import tomllib
from dataclasses import dataclass
from pathlib import Path

DEFAULT_LATEX_PREAMBLE = (
    "\\documentclass[varwidth=true, border=5pt, crop]{standalone}\n"
    "\\begin{document}\n"
)
DEFAULT_LATEX_POSTAMBLE = "\\end{document}\n"


@dataclass(slots=True)
class LatexConfig:
    preamble: str = DEFAULT_LATEX_PREAMBLE
    postamble: str = DEFAULT_LATEX_POSTAMBLE


def _default_latex_config() -> LatexConfig:
    return LatexConfig(
        preamble=DEFAULT_LATEX_PREAMBLE,
        postamble=DEFAULT_LATEX_POSTAMBLE,
    )


def _toml_literal_multiline(value: str) -> str:
    normalized = value.replace("\r\n", "\n").replace("\r", "\n")
    content = normalized.rstrip("\n")
    return "'''\n" + content + "\n'''"


def build_default_config_toml() -> str:
    latex = _default_latex_config()
    return (
        "[src.latex]\n"
        f"preamble = {_toml_literal_multiline(latex.preamble)}\n"
        f"postamble = {_toml_literal_multiline(latex.postamble)}\n"
    )


def load_latex_config(mdoc_root: Path) -> LatexConfig:
    default = _default_latex_config()
    config_path = mdoc_root / ".mdc" / "config.toml"
    if not config_path.is_file():
        return default

    try:
        payload = config_path.read_text(encoding="utf-8")
    except OSError as exc:
        raise OSError(f"failed to read {config_path}: {exc}") from exc

    try:
        raw = tomllib.loads(payload)
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid TOML in {config_path}: {exc}") from exc

    src = raw.get("src")
    if src is None:
        return default
    if not isinstance(src, dict):
        raise ValueError("config key 'src' must be a table")

    latex = src.get("latex")
    if latex is None:
        return default
    if not isinstance(latex, dict):
        raise ValueError("config key 'src.latex' must be a table")

    preamble = latex.get("preamble", default.preamble)
    postamble = latex.get("postamble", default.postamble)
    if not isinstance(preamble, str):
        raise ValueError("config key 'src.latex.preamble' must be a string")
    if not isinstance(postamble, str):
        raise ValueError("config key 'src.latex.postamble' must be a string")

    return LatexConfig(preamble=preamble, postamble=postamble)


def init_mdoc_config(mdoc_root: Path) -> None:
    config_path = mdoc_root / ".mdc" / "config.toml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(build_default_config_toml(), encoding="utf-8")
