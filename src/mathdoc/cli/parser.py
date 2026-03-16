import argparse

from .cmd_core import cmd_edit, cmd_init, cmd_new, cmd_search, cmd_sync
from .cmd_deps import (
    cmd_dep_add,
    cmd_dep_leaf,
    cmd_dep_refs,
    cmd_dep_rm,
    cmd_dep_show,
)
from .cmd_eval import cmd_eval
from .cmd_graph import cmd_graph_check, cmd_graph_roots


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdc", description="MathDoc CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser("init", help="Initialize a new MathDoc folder")
    init_parser.set_defaults(func=cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument(
        "-t", "--title", default="Untitled", help="Title of the new mdoc (optional)"
    )
    new_parser.add_argument(
        "-f",
        "--file",
        default=".",
        help="Relative output file path without the forced .mdoc suffix (default: auto fnode at root)",
    )
    new_parser.set_defaults(func=cmd_new)

    edit_parser = subparsers.add_parser(
        "edit", help="Open a mdoc with $EDITOR and refresh its index entry"
    )
    edit_parser.add_argument(
        "source",
        help="Source mdoc to edit (fnode or .mdoc path)",
    )
    edit_parser.set_defaults(func=cmd_edit)

    sync_parser = subparsers.add_parser("sync", help="Force refresh all index entries")
    sync_parser.set_defaults(func=cmd_sync)

    search_parser = subparsers.add_parser(
        "search", help="Search mdocs by title or fnode"
    )
    search_parser.add_argument("query", help="Query by title or fnode")
    search_parser.add_argument(
        "-n",
        "--max-results",
        type=int,
        default=200,
        help="Maximum search results to show (default: 200)",
    )
    search_parser.set_defaults(func=cmd_search)

    graph_parser = subparsers.add_parser(
        "graph", help="Inspect the global dependency graph"
    )
    graph_subparsers = graph_parser.add_subparsers(
        dest="graph_command",
        required=True,
    )

    graph_check_parser = graph_subparsers.add_parser(
        "check",
        help="Scan the whole repo and report graph issues",
    )
    graph_check_parser.add_argument(
        "--full",
        action="store_true",
        help="Refresh the workspace index before checking the graph",
    )
    graph_check_parser.set_defaults(func=cmd_graph_check)

    graph_roots_parser = graph_subparsers.add_parser(
        "roots",
        help="List all global root nodes with no incoming dependencies",
    )
    graph_roots_parser.add_argument(
        "--refresh",
        action="store_true",
        help="Refresh the workspace index before reading cached roots",
    )
    graph_roots_parser.set_defaults(func=cmd_graph_roots)

    eval_parser = subparsers.add_parser(
        "eval", help="Compile and run all blocks in a mdoc"
    )
    eval_parser.add_argument(
        "source",
        help="Source mdoc to evaluate (fnode or .mdoc path)",
    )
    eval_parser.add_argument(
        "-d",
        "--depth",
        type=int,
        default=1,
        help="Dependency traversal depth (-1 for unlimited, default: 1)",
    )
    eval_parser.set_defaults(func=cmd_eval)

    dep_parser = subparsers.add_parser("dep", help="Manage mdoc dependencies")
    dep_subparsers = dep_parser.add_subparsers(dest="dep_command", required=True)

    dep_add_parser = dep_subparsers.add_parser(
        "add", help="Search and add dependencies to a mdoc"
    )
    dep_add_parser.add_argument(
        "source",
        help="Source mdoc to modify (fnode or .mdoc path)",
    )
    dep_add_parser.add_argument(
        "query",
        help="Search query for dependency mdocs",
    )
    dep_add_parser.add_argument(
        "-n",
        "--max-results",
        type=int,
        default=200,
        help="Maximum dependency candidates to show (default: 200)",
    )
    dep_add_parser.set_defaults(func=cmd_dep_add)

    dep_show_parser = dep_subparsers.add_parser(
        "show", help="Show dependencies of a mdoc"
    )
    dep_show_parser.add_argument(
        "source",
        help="Source mdoc to inspect (fnode or .mdoc path)",
    )
    dep_show_parser.add_argument(
        "-d",
        "--depth",
        type=int,
        default=1,
        help="Dependency traversal depth (-1 for unlimited, default: 1)",
    )
    dep_show_parser.set_defaults(func=cmd_dep_show)

    dep_leaf_parser = dep_subparsers.add_parser(
        "leaf", help="Show all leaf dependencies of a mdoc"
    )
    dep_leaf_parser.add_argument(
        "source",
        help="Source mdoc to inspect (fnode or .mdoc path)",
    )
    dep_leaf_parser.set_defaults(func=cmd_dep_leaf)

    dep_rm_parser = dep_subparsers.add_parser(
        "rm", help="Interactively remove dependencies from a mdoc"
    )
    dep_rm_parser.add_argument(
        "source",
        help="Source mdoc to modify (fnode or .mdoc path)",
    )
    dep_rm_parser.set_defaults(func=cmd_dep_rm)

    dep_refs_parser = dep_subparsers.add_parser(
        "refs", help="Show reverse dependencies of a mdoc"
    )
    dep_refs_parser.add_argument(
        "target",
        help="Target mdoc to inspect (fnode or .mdoc path)",
    )
    dep_refs_parser.add_argument(
        "-d",
        "--depth",
        type=int,
        default=1,
        help="Reverse dependency traversal depth (-1 for unlimited, default: 1)",
    )
    dep_refs_parser.add_argument(
        "--refresh",
        action="store_true",
        help="Refresh the workspace index before reading cached reverse dependencies",
    )
    dep_refs_parser.set_defaults(func=cmd_dep_refs)

    return parser
