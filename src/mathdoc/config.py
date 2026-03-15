import copy
import tomllib
from pathlib import Path
from typing import Any

DEFAULT_CONFIG: dict[str, Any] = {
    "src": {
        "natl": {
            "depens": True,
            "reverse_depens": True,
        },
        "latex": {
            "depens": True,
            "reverse_depens": True,
            "timeout_sec": 30,
            "preamble": "\\documentclass{article}\n\\begin{document}\n",
            "postamble": "\\end{document}\n",
        },
        "py": {
            "depens": False,
            "reverse_depens": False,
            "timeout_sec": 30,
        },
    }
}


def _merge_dict(base: dict[str, Any], override: dict[str, Any]) -> None:
    for key, value in override.items():
        current = base.get(key)
        if isinstance(current, dict) and isinstance(value, dict):
            _merge_dict(current, value)
            continue
        base[key] = value


def load_config(mdcroot: Path) -> dict[str, Any]:
    config = copy.deepcopy(DEFAULT_CONFIG)
    config_path = mdcroot / ".mdc" / "config.toml"
    if not config_path.is_file():
        return config

    try:
        payload = config_path.read_text(encoding="utf-8")
    except OSError as exc:
        raise OSError(f"failed to read {config_path}: {exc}") from exc

    try:
        override = tomllib.loads(payload) if payload.strip() else {}
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid TOML in {config_path}: {exc}") from exc

    if not isinstance(override, dict):
        raise ValueError("config.toml root must be a table")

    _merge_dict(config, override)
    return config
