import argparse

from .filenode import FileNode


def _cmd_init(args: argparse.Namespace) -> int:
    return 0


def _cmd_new(args: argparse.Namespace) -> int:
    node = FileNode.create(args.path, args.title)
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
    init_parser.set_defaults(handler=_cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument("-t", "--title", default="Untitled",
                            help="Title of the new card (optional)")
    new_parser.add_argument("-p", "--path", default="",
                            help="Output path for the card file (optional)")
    new_parser.set_defaults(handler=_cmd_new)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)


def test():
    from pathlib import Path
    import json
    A = FileNode(Path("data/test2.mdoc"), title="Test Card")
    A.load()
    A.path = Path("data/test4.mdoc")
    A.save()
    print(json.dumps(A.to_dict(), indent=2))
