import os
from collections.abc import Iterator
from pathlib import Path


def to_rel_path(root: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(path)


def find_mdcroot(start: Path) -> Path | None:
    for candidate in [start, *start.parents]:
        if (candidate / ".mdc").is_dir():
            return candidate
    return None


def find_nested_mdcroot(root: Path, path: Path) -> Path | None:
    root_resolved = root.resolve()
    target = path.resolve()

    try:
        target.relative_to(root_resolved)
    except ValueError:
        return None

    for candidate in [target, *target.parents]:
        if candidate == root_resolved:
            return None
        if (candidate / ".mdc").is_dir():
            return candidate
    return None


def iter_workspace_mdoc_files(root: Path) -> Iterator[Path]:
    root_resolved = root.resolve()

    for current_root, dirnames, filenames in os.walk(root_resolved):
        current = Path(current_root)
        if current != root_resolved and ".mdc" in dirnames:
            dirnames[:] = []
            continue

        dirnames[:] = [dirname for dirname in dirnames if dirname != ".mdc"]
        for filename in filenames:
            if filename.endswith(".mdoc"):
                yield current / filename
