import argparse
from pathlib import Path

from .mdocnode import MdocNode


def _find_mdoc_root(start: Path) -> Path | None:
    for candidate in [start, *start.parents]:
        if (candidate / ".mdc").is_dir():
            return candidate
    return None


def _cmd_init(_: argparse.Namespace) -> int:
    local_mdc = Path.cwd() / ".mdc"

    if local_mdc.is_dir():
        print(f"Already intialized as mdoc directory: {local_mdc}")
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

    node = MdocNode.create(args.folder, args.title)
    node.save()

    print(f"created: {node.path}")
    print(f"fnode: {node.fnode}")
    print(f"title: {node.title}")
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

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
