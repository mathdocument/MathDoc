import argparse
import os
import shlex
import sqlite3
import subprocess
from pathlib import Path

from .depgraph import DepGraph
from .indcache import IndCache
from .mdocnode import DependencyItem
from .mdocnode import MdocNode
from .utils import (
    STYLE,
    colorize,
    find_mdcroot,
    format_mdoc_item,
    select_indices_interactive,
    to_rel_path,
    warn_index_failure,
)


def _get_mdcroot_or_none() -> Path | None:
    mdcroot = find_mdcroot(Path.cwd())
    if mdcroot is None:
        print("Error: not inside an mdoc directory, run `mdc init` first")
        return None
    return mdcroot


def _load_mdoc_from_ref(cache: IndCache, ref: str) -> tuple[MdocNode, str]:
    _, _, src_path = cache.resolve_ref(ref, cwd=Path.cwd())
    node = MdocNode(mdcroot=cache.root, path=src_path, title="")
    node.load()
    return node, to_rel_path(cache.root, src_path)


def _print_index_error(*, action: str, exc: Exception) -> None:
    print(f"Error: failed to {action}: {exc}")
    print(
        "Hint: run `mdc sync` to rebuild the index; "
        "if it still fails, remove `.mdc/index.db` and retry."
    )


def _bootstrap_cache(cache: IndCache, *, action: str) -> bool:
    try:
        cache.bootstrap_if_needed()
    except (OSError, ValueError, sqlite3.Error) as exc:
        _print_index_error(action=action, exc=exc)
        return False
    return True


def _is_missing_dependency_item(item: DependencyItem, *, graph: DepGraph) -> bool:
    return item.fnode in graph.missing_fnodes


def _format_dependency_line(item: DependencyItem, *, graph: DepGraph) -> str:
    line = f"[{item.depth}] {format_mdoc_item(item.fnode, item.title, item.rel_path)}"
    if _is_missing_dependency_item(item, graph=graph):
        return colorize(line, STYLE["red"])
    return line


def _print_dependency_chain(
    *,
    node: MdocNode,
    src_rel: str,
    dep_items: list[DependencyItem],
    graph: DepGraph,
) -> None:
    print(f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    print(f"dependencies: {len(dep_items)}")
    for item in dep_items:
        print(_format_dependency_line(item, graph=graph))


def _print_missing_dependency_warning(
    *,
    dep_items: list[DependencyItem],
    graph: DepGraph,
    for_eval: bool,
) -> None:
    missing_count = sum(
        1 for item in dep_items if _is_missing_dependency_item(item, graph=graph)
    )
    if missing_count <= 0:
        return

    if for_eval:
        print(
            "Error: missing dependency targets detected; "
            "remove the broken references with `mdc dep rm` before eval."
        )
    else:
        print(
            f"Warning: detected {missing_count} broken dependency reference(s); "
            "broken rows are highlighted in red when the terminal supports color."
        )


def _cmd_init(_: argparse.Namespace) -> int:
    mdcroot = Path.cwd()
    local_mdc = mdcroot / ".mdc"
    config_path = local_mdc / "config.toml"

    if local_mdc.is_dir():
        print(f"Already initialized as mdoc directory: {local_mdc}")
        return 0

    local_mdc.mkdir(parents=False, exist_ok=False)
    try:
        config_path.write_text("", encoding="utf-8")
    except OSError as exc:
        print(f"Error: failed to write config.toml: {exc}")
        return 1
    print("mdoc folder initialized")
    return 0


def _cmd_new(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    target = Path(args.folder).resolve()
    try:
        target.relative_to(mdcroot.resolve())
    except ValueError:
        print(f"Error: target path must be under mdoc root {mdcroot}")
        return 1

    if target.exists() and not target.is_dir():
        print(f"Error: target folder is a file: {target}")
        return 1

    node = MdocNode.create(
        mdcroot=mdcroot,
        folder=args.folder,
        title=args.title,
    )
    try:
        node.save()
    except OSError as exc:
        print(f"Error: failed to save mdoc file: {exc}")
        return 1

    cache = IndCache(mdcroot)
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
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare search index"):
        return 1

    try:
        matches = cache.search(query)
    except (OSError, ValueError, sqlite3.Error) as exc:
        _print_index_error(action="search mdocs", exc=exc)
        return 1

    if not matches:
        print(f"No results for: {args.query}")
        return 0

    print(f"results: {len(matches)}")
    for fnode, title, rel_path in matches:
        print(format_mdoc_item(fnode, title, rel_path))

    return 0


def _cmd_eval(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare eval index"):
        return 1

    try:
        node, src_rel = _load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    graph = DepGraph(mdcroot=mdcroot, root_node=node, cache=cache)
    try:
        dep_items = graph.dependency_items(depth=args.depth)
    except ValueError as exc:
        print(f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
        print(f"Error: failed to inspect dependencies: {exc}")
        return 1

    _print_dependency_chain(
        node=node,
        src_rel=src_rel,
        dep_items=dep_items,
        graph=graph,
    )

    if not node.blocks:
        print("No blocks to eval")
        return 0

    if any(_is_missing_dependency_item(item, graph=graph) for item in dep_items):
        _print_missing_dependency_warning(
            dep_items=dep_items,
            graph=graph,
            for_eval=True,
        )
        return 1

    try:
        block_results = graph.eval_blocks(
            depth=args.depth,
            reverse_depens=args.reverse,
        )
    except ValueError as exc:
        print(f"Error: failed to eval mdoc: {exc}")
        return 1
    print(f"blocks: {len(block_results)}")
    print("result:")
    failed = 0
    for index, block in enumerate(block_results, start=1):
        result = block[1]
        if result.result:
            print(colorize(f"[{index}] {block[0]}: ok", STYLE["grn"]))
        else:
            failed += 1
            print(
                colorize(
                    f"[{index}] {block[0]}: failed ({result.rtcode})",
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
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    query = args.query.strip()
    if not query:
        print("Error: query cannot be empty")
        return 1
    if args.max_results < 1:
        print("Error: --max-results must be >= 1")
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        node, src_rel = _load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        print(f"Error: {exc}")
        return 1
    graph = DepGraph(mdcroot=mdcroot, root_node=node, cache=cache)
    try:
        matches = cache.search(query)
    except (OSError, ValueError, sqlite3.Error) as exc:
        _print_index_error(action="search dependency candidates", exc=exc)
        return 1
    matches = [row for row in matches if row[0] != node.fnode]
    matches = matches[: args.max_results]
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
    existing_depens = set(graph.direct_dependency_fnodes())
    for dep_fnode in selected_by_fnode:
        if dep_fnode == node.fnode:
            skipped_self.append(dep_fnode)
            continue
        if dep_fnode in existing_depens:
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

    print(f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
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
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        node, src_rel = _load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    graph = DepGraph(mdcroot=mdcroot, root_node=node, cache=cache)
    try:
        dep_items = graph.dependency_items(depth=args.depth)
    except ValueError as exc:
        print(f"Error: failed to inspect dependencies: {exc}")
        return 1

    dep_rows = [(item.fnode, item.title, item.rel_path) for item in dep_items]
    if dep_rows:
        try:
            cache.refresh_rows(dep_rows)
        except (OSError, ValueError, sqlite3.Error) as exc:
            warn_index_failure("dependencies were inspected", exc)

    _print_dependency_chain(
        node=node,
        src_rel=src_rel,
        dep_items=dep_items,
        graph=graph,
    )
    _print_missing_dependency_warning(
        dep_items=dep_items,
        graph=graph,
        for_eval=False,
    )
    return 0


def _cmd_dep_rm(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        node, src_rel = _load_mdoc_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        print(f"Error: failed to load mdoc: {exc}")
        return 1

    graph = DepGraph(mdcroot=mdcroot, root_node=node, cache=cache)
    try:
        dep_items = graph.direct_dependency_items()
    except ValueError as exc:
        print(f"Error: failed to inspect dependencies: {exc}")
        return 1

    if not dep_items:
        print(f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
        print("No dependencies to remove")
        return 0

    dep_rows = [(item.fnode, item.title, item.rel_path) for item in dep_items]
    error_indices = {
        idx
        for idx, item in enumerate(dep_items)
        if _is_missing_dependency_item(item, graph=graph)
    }
    _print_missing_dependency_warning(
        dep_items=dep_items,
        graph=graph,
        for_eval=False,
    )

    try:
        selected_indices = select_indices_interactive(
            dep_rows,
            error_indices=error_indices,
        )
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
    except (OSError, ValueError, sqlite3.Error) as exc:
        warn_index_failure("dependencies were inspected", exc)

    old_len = len(node.depens)
    for dep_fnode in selected_fnodes:
        node.rmv_dependency(dep_fnode)
    removed_count = old_len - len(node.depens)
    if removed_count <= 0:
        print("No dependencies removed")
        return 0

    try:
        node.save()
    except OSError as exc:
        print(f"Error: failed to save mdoc: {exc}")
        return 1

    print(f"source: {format_mdoc_item(node.fnode, node.title, src_rel, marker='')}")
    print(f"removed: {removed_count}")
    for dep_fnode in selected_fnodes:
        row = selected_rows_by_fnode.get(dep_fnode)
        if row is None:
            continue
        line = format_mdoc_item(row[0], row[1], row[2], marker="-")
        if dep_fnode in graph.missing_fnodes:
            print(colorize(line, STYLE["red"]))
        else:
            print(line)
    return 0


def _cmd_sync(_: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    try:
        cache.refresh_all()
        total = cache.count()
    except (OSError, ValueError, sqlite3.Error) as exc:
        _print_index_error(action="sync index", exc=exc)
        return 1
    print(f"synced: {total}")
    return 0


def _cmd_edit(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare edit index"):
        return 1

    try:
        src_path = cache.resolve_edit_target_path(args.source, cwd=Path.cwd())
    except (ValueError, sqlite3.Error) as exc:
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

    print(f"edited: {to_rel_path(mdcroot, src_path)}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mdc", description="MathDoc CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser("init", help="Initialize a new MathDoc folder")
    init_parser.set_defaults(func=_cmd_init)

    new_parser = subparsers.add_parser("new", help="Create a new mdoc file")
    new_parser.add_argument(
        "-t", "--title", default="Untitled", help="Title of the new mdoc (optional)"
    )
    new_parser.add_argument(
        "-f", "--folder", default=".", help="Output folder for the mdoc file (optional)"
    )
    new_parser.set_defaults(func=_cmd_new)

    edit_parser = subparsers.add_parser(
        "edit", help="Open a mdoc with $EDITOR and refresh its index entry"
    )
    edit_parser.add_argument(
        "source",
        help="Source mdoc to edit (fnode or .mdoc path)",
    )
    edit_parser.set_defaults(func=_cmd_edit)

    sync_parser = subparsers.add_parser("sync", help="Force refresh all index entries")
    sync_parser.set_defaults(func=_cmd_sync)

    search_parser = subparsers.add_parser(
        "search", help="Search mdocs by title or fnode"
    )
    search_parser.add_argument("query", help="Query by title or fnode")
    search_parser.set_defaults(func=_cmd_search)

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
    eval_parser.add_argument(
        "-r",
        "--reverse",
        action="store_true",
        help="Reverse merged dependency order for depens-enabled block types",
    )
    eval_parser.set_defaults(func=_cmd_eval)

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
    dep_add_parser.set_defaults(func=_cmd_dep_add)

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
    dep_show_parser.set_defaults(func=_cmd_dep_show)

    dep_rm_parser = dep_subparsers.add_parser(
        "rm", help="Interactively remove dependencies from a mdoc"
    )
    dep_rm_parser.add_argument(
        "source",
        help="Source mdoc to modify (fnode or .mdoc path)",
    )
    dep_rm_parser.set_defaults(func=_cmd_dep_rm)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
