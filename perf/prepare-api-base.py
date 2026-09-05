#!/usr/bin/env python3
"""Install the current API benchmark in a base checkout without changing its source."""
from pathlib import Path
import shutil
import sys
import tomllib


def prepare(base: Path) -> None:
    current = Path(__file__).resolve().parent.parent
    manifest = base / "Cargo.toml"
    text = manifest.read_text()
    benches = tomllib.loads(text).get("bench", [])
    registered = next((bench for bench in benches if bench["name"] == "api"), None)
    if registered is None:
        manifest.write_text(text + '\n[[bench]]\nname = "api"\nharness = false\n')
    elif registered.get("harness", True) or registered.get("path", "benches/api.rs") != "benches/api.rs":
        raise ValueError("base API benchmark has an incompatible target declaration")
    for relative in ["benches/api.rs", "perf/api-budgets.json"]:
        destination = base / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(current / relative, destination)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: prepare-api-base.py BASE_CHECKOUT")
    prepare(Path(sys.argv[1]).resolve())
