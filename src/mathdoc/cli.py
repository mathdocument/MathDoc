import argparse
import os
import shlex
import sqlite3
import subprocess
from pathlib import Path

from .config import init_mdoc_config
from .indcache import IndCache
from .mdocnode import MdocNode
from .utils import (
    STYLE,
    colorize,
    find_mdoc_root,
    format_mdoc_item,
    load_mdoc_from_ref,
    select_indices_interactive,
    to_rel_path,
    warn_index_failure,
)


def _get_mdoc_root_or_none() -> Path | None:
    mdoc_root = find_mdoc_root(Path.cwd())
    if mdoc_root is None:
        print("Error: not inside an mdoc directory, run `mdc init` first")
        return None
    return mdoc_root


def _cmd_init(_: argparse.Namespace) -> int:
    mdoc_root = Path.cwd()
    local_mdc = mdoc_root / ".mdc"

    if local_mdc.is_dir():
        print(f"Already initialized as mdoc directory: {local_mdc}")
        return 0

    local_mdc.mkdir(parents=False, exist_ok=False)
    try:
        init_mdoc_config(mdoc_root)
    except OSError as exc:
        print(f"Error: failed to write default config.toml: {exc}")
        return 1
    print("mdoc folder initialized")
    return 0


def _cmd_new(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    target = Path(args.folder).resolve()
    try:
        target.relative_to(mdoc_root.resolve())
    except ValueError:
        print(
            f"Error: target path must be under mdoc root {mdoc_root}")
        return 1

    if target.exists() and not target.is_dir():
        print(f"Error: target folder is a file: {target}")
        return 1

    node = MdocNode.create(args.folder, args.title)
    try:
        node.save()
    except OSError as exc:
        print(f"Error: failed to save mdoc file: {exc}")
        return 1

    cache = IndCache(mdoc_root)
    try:
        cache.bootstrap_if_needed()
        cache.upsert_path(node.path)
    except (OSError, ValueError, sqlite3.Error) as exc:
        warn_index_failure("mdoc was created", exc)

    print(f"created: {node.path}")
    print(f"fnode: {node.fnode}")
    print(f"title: {node.title}")
    return 0


def _cmd_search(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    matches = cache.search(query)

    if not matches:
        print(f"No results for: {args.query}")
        return 0

    print(f"results: {len(matches)}")
    for fnode, title, rel_path in matches:
        print(format_mdoc_item(fnode, title, rel_path))

    return 0


def _cmd_eval(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    try:
        node, src_rel = load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    print(
        f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    if not node.blocks:
        print("No blocks to eval")
        return 0

    print(f"blocks: {len(node.blocks)}")
    print("result:")
    failed = 0
    for index, block in enumerate(node.blocks, start=1):
        result = block.compile(mdoc_root=mdoc_root, fnode=node.fnode)
        if result.ok:
            print(colorize(f"[{index}] {block.codetype}: ok", STYLE["grn"]))
        else:
            failed += 1
            print(
                colorize(
                    f"[{index}] {block.codetype}: failed ({result.returncode})",
                    STYLE["red"],
                )
            )

        if result.stdout:
            for line in result.stdout.rstrip("\n").splitlines():
                if line.startswith("\x1b"):
                    print(f"    {line}")
                else:
                    print(f"    {line}")
        if result.stderr:
            for line in result.stderr.rstrip("\n").splitlines():
                print(f"    ! {line}")
        print("")

    summary_color = STYLE["grn"] if failed == 0 else STYLE["red"]
    print(colorize(f"failed: {failed}", summary_color))
    return 1 if failed else 0


def _cmd_dep_add(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1
    if args.max_results < 1:
        print("Error: --max-results must be >= 1")
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    try:
        node, src_rel = load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: {exc}")
        return 1
    matches = cache.search(query)
    matches = [row for row in matches if row[0] != node.fnode]
    matches = matches[:args.max_results]
    if not matches:
        print(f"No dependency candidates for: {args.query}")
        return 0

    try:
        selected_indices = select_indices_interactive(matches)
    except RuntimeError as exc:
        print(f"Error: {exc}")
        return 1

    if selected_indices is None:
        print("Canceled")
        return 0
    if not selected_indices:
        print("No dependencies selected")
        return 0

    selected_rows = [matches[idx] for idx in selected_indices]
    selected_by_fnode = {row[0]: row for row in selected_rows}
    selected_fnodes = list(selected_by_fnode.keys())

    try:
        cache.refresh_rows(list(selected_by_fnode.values()))
        refreshed_by_fnode = cache.lookup_by_fnode(selected_fnodes)
    except (OSError, ValueError, sqlite3.Error) as exc:
        warn_index_failure("dependencies were inspected", exc)
        refreshed_by_fnode = {}

    for dep_fnode in selected_fnodes:
        refreshed = refreshed_by_fnode.get(dep_fnode)
        if refreshed is None:
            continue
        selected_by_fnode[dep_fnode] = (dep_fnode, refreshed[0], refreshed[1])

    added: list[str] = []
    skipped_existing: list[str] = []
    skipped_self: list[str] = []
    for dep_fnode in selected_by_fnode:
        if dep_fnode == node.fnode:
            skipped_self.append(dep_fnode)
            continue
        if dep_fnode in node.depens:
            skipped_existing.append(dep_fnode)
            continue
        node.add_dependency(dep_fnode)
        added.append(dep_fnode)

    if added:
        try:
            node.save()
        except OSError as exc:
            print(f"Error: failed to save mdoc: {exc}")
            return 1

    print(
        f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    print(f"added: {len(added)}")
    for dep_fnode in added:
        dep_row = selected_by_fnode[dep_fnode]
        print(format_mdoc_item(dep_fnode, dep_row[1], dep_row[2], marker="+"))
    if skipped_existing:
        print(f"skipped existing: {len(skipped_existing)}")
    if skipped_self:
        print(f"skipped self: {len(skipped_self)}")
    return 0


def _cmd_dep_show(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    try:
        node, src_rel = load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    dep_rows = cache.dep_rows(node.depens)
    if dep_rows:
        try:
            cache.refresh_rows(dep_rows)
            dep_rows = cache.dep_rows(node.depens)
        except (OSError, ValueError, sqlite3.Error) as exc:
            warn_index_failure("dependencies were inspected", exc)

    print(
        f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    print(f"dependencies: {len(node.depens)}")
    for dep_fnode, dep_title, dep_path in dep_rows:
        print(format_mdoc_item(dep_fnode, dep_title, dep_path))
    return 0


def _cmd_dep_rm(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    try:
        node, src_rel = load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    if not node.depens:
        print(
            f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
        print("No dependencies to remove")
        return 0

    dep_rows = cache.dep_rows(node.depens)

    try:
        selected_indices = select_indices_interactive(dep_rows)
    except RuntimeError as exc:
        print(f"Error: {exc}")
        return 1

    if selected_indices is None:
        print("Canceled")
        return 0
    if not selected_indices:
        print("No dependencies selected")
        return 0

    selected_fnodes: list[str] = []
    selected_set: set[str] = set()
    selected_rows_by_fnode: dict[str, tuple[str, str, str]] = {}
    for idx in selected_indices:
        row = dep_rows[idx]
        dep_fnode = row[0]
        if dep_fnode in selected_set:
            continue
        selected_set.add(dep_fnode)
        selected_fnodes.append(dep_fnode)
        selected_rows_by_fnode[dep_fnode] = row

    try:
        cache.refresh_rows(list(selected_rows_by_fnode.values()))
        refreshed_by_fnode = cache.lookup_by_fnode(selected_fnodes)
    except (OSError, ValueError, sqlite3.Error) as exc:
        warn_index_failure("dependencies were inspected", exc)
        refreshed_by_fnode = {}

    for dep_fnode in selected_fnodes:
        refreshed = refreshed_by_fnode.get(dep_fnode)
        if refreshed is None:
            continue
        selected_rows_by_fnode[dep_fnode] = (
            dep_fnode, refreshed[0], refreshed[1])

    old_len = len(node.depens)
    node.depens = [dep for dep in node.depens if dep not in selected_set]
    removed_count = old_len - len(node.depens)
    if removed_count <= 0:
        print("No dependencies removed")
        return 0

    try:
        node.save()
    except OSError as exc:
        print(f"Error: failed to save mdoc: {exc}")
        return 1

    print(
        f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    print(f"removed: {removed_count}")
    for dep_fnode in selected_fnodes:
        row = selected_rows_by_fnode.get(dep_fnode)
        if row is None:
            continue
        print(format_mdoc_item(row[0], row[1], row[2], marker="-"))
    return 0


def _cmd_sync(_: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    cache = IndCache(mdoc_root)
    cache.refresh_all()
    total = cache.count()
    print(f"synced: {total}")
    return 0


def _cmd_edit(args: argparse.Namespace) -> int:
    mdoc_root = _get_mdoc_root_or_none()
    if mdoc_root is None:
        return 1

    cache = IndCache(mdoc_root)
    cache.bootstrap_if_needed()
    try:
        src_path = cache.resolve_edit_target_path(args.source, cwd=Path.cwd())
    except ValueError as exc:
        print(f"Error: {exc}")
        return 1

    editor_raw = os.environ.get("EDITOR", "").strip()
    if not editor_raw:
        print("Error: $EDITOR is not set")
        return 1
    editor_cmd = shlex.split(editor_raw)
    if not editor_cmd:
        print("Error: $EDITOR is empty")
        return 1

    try:
        edit_proc = subprocess.run([*editor_cmd, str(src_path)], check=False)
    except OSError as exc:
        print(f"Error: failed to launch $EDITOR: {exc}")
        return 1

    if edit_proc.returncode != 0:
        print(f"Error: editor exited with code {edit_proc.returncode}")
        return edit_proc.returncode

    try:
        cache.upsert_path(src_path)
    except (OSError, ValueError, sqlite3.Error) as exc:
        warn_index_failure("mdoc was edited", exc)

    print(f"edited: {to_rel_path(mdoc_root, src_path)}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdc", description="MathDoc CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser(
        "init", help="Initialize a new MathDoc folder")
    init_parser.set_defaults(func=_cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument("-t", "--title", default="Untitled",
                            help="Title of the new mdoc (optional)")
    new_parser.add_argument("-f", "--folder", default=".",
                            help="Output folder for the mdoc file (optional)")
    new_parser.set_defaults(func=_cmd_new)

    edit_parser = subparsers.add_parser(
        "edit", help="Open a mdoc with $EDITOR and refresh its index entry")
    edit_parser.add_argument(
        "source",
        help="Source mdoc to edit (fnode or .mdoc path)",
    )
    edit_parser.set_defaults(func=_cmd_edit)

    sync_parser = subparsers.add_parser(
        "sync", help="Force refresh all index entries")
    sync_parser.set_defaults(func=_cmd_sync)

    search_parser = subparsers.add_parser(
        "search", help="Search mdocs by title or fnode")
    search_parser.add_argument("query", help="Query by title or fnode")
    search_parser.set_defaults(func=_cmd_search)

    eval_parser = subparsers.add_parser(
        "eval", help="Compile and run all blocks in a mdoc")
    eval_parser.add_argument(
        "source",
        help="Source mdoc to evaluate (fnode or .mdoc path)",
    )
    eval_parser.set_defaults(func=_cmd_eval)

    dep_parser = subparsers.add_parser("dep", help="Manage mdoc dependencies")
    dep_subparsers = dep_parser.add_subparsers(
        dest="dep_command", required=True)

    dep_add_parser = dep_subparsers.add_parser(
        "add", help="Search and add dependencies to a mdoc")
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
    dep_add_parser.set_defaults(func=_cmd_dep_add)

    dep_show_parser = dep_subparsers.add_parser(
        "show", help="Show dependencies of a mdoc")
    dep_show_parser.add_argument(
        "source",
        help="Source mdoc to inspect (fnode or .mdoc path)",
    )
    dep_show_parser.set_defaults(func=_cmd_dep_show)

    dep_rm_parser = dep_subparsers.add_parser(
        "rm", help="Interactively remove dependencies from a mdoc")
    dep_rm_parser.add_argument(
        "source",
        help="Source mdoc to modify (fnode or .mdoc path)",
    )
    dep_rm_parser.set_defaults(func=_cmd_dep_rm)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
