import argparse
import os
import shlex
import sqlite3
import subprocess
from pathlib import Path

from .depgraph import DepGraph
from .depgraph import DependencyItem
from .depgraph import GraphCheckReport
from .depgraph import GraphIssue
from .depgraph.exceptions import DependencyCycleError
from .indcache import IndCache
from .ui import BrokenDependencySummary
from .ui import ChainView
from .ui import CycleView
from .ui import DepAddView
from .ui import DepRmView
from .ui import EvalBlockView
from .ui import EvalReportView
from .ui import GraphCheckView
from .ui import IssueView
from .ui import NodeRef
from .ui import TerminalUI
from .ui import select_indices_interactive
from .utils import (
    find_mdcroot,
    to_rel_path,
)


UI = TerminalUI()


def _node_ref(
    *,
    fnode: str,
    title: str,
    rel_path: str,
    depth: int | None = None,
    broken: bool = False,
) -> NodeRef:
    return NodeRef(
        fnode=fnode,
        title=title,
        rel_path=rel_path,
        depth=depth,
        broken=broken,
    )


def _node_ref_from_item(
    item: DependencyItem,
    *,
    rel_path: str | None = None,
    broken: bool = False,
) -> NodeRef:
    return _node_ref(
        fnode=item.fnode,
        title=item.title,
        rel_path=rel_path or item.rel_path,
        depth=item.depth,
        broken=broken,
    )


def _node_ref_from_row(
    row: tuple[str, str, str],
    *,
    depth: int | None = None,
    broken: bool = False,
) -> NodeRef:
    return _node_ref(
        fnode=row[0],
        title=row[1],
        rel_path=row[2],
        depth=depth,
        broken=broken,
    )


def _issue_view(issue: GraphIssue) -> IssueView:
    return IssueView(
        ref=_node_ref(
            fnode=issue.fnode,
            title=issue.title,
            rel_path=issue.rel_path,
            broken=True,
        ),
        error=issue.error,
    )


def _cycle_view(graph: DepGraph, cycle: list[str]) -> CycleView:
    cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
    return CycleView(
        nodes=tuple(
            _node_ref_from_item(
                graph.ref_item_for_fnode(fnode),
                broken=graph.is_broken_fnode(fnode),
            )
            for fnode in cycle_nodes
        )
    )


def _graph_check_view(graph: DepGraph, report: GraphCheckReport) -> GraphCheckView:
    return GraphCheckView(
        nodes=report.nodes,
        edges=report.edges,
        missing=tuple(_issue_view(issue) for issue in report.missing),
        invalid=tuple(_issue_view(issue) for issue in report.invalid),
        cycles=tuple(_cycle_view(graph, cycle) for cycle in report.cycles),
    )


def _chain_view(
    *,
    anchor_label: str,
    anchor: NodeRef,
    count_label: str,
    items: list[DependencyItem],
    graph: DepGraph,
) -> ChainView:
    return ChainView(
        anchor_label=anchor_label,
        anchor=anchor,
        count_label=count_label,
        items=tuple(
            _node_ref_from_item(
                item,
                broken=graph.is_broken_fnode(item.fnode),
            )
            for item in items
        ),
    )


def _broken_dependency_summary(
    dep_items: list[DependencyItem],
    graph: DepGraph,
) -> BrokenDependencySummary:
    missing = 0
    invalid = 0
    for item in dep_items:
        issue = graph.issue_for_fnode(item.fnode)
        if issue is None:
            continue
        if issue.kind == "missing":
            missing += 1
        elif issue.kind == "invalid":
            invalid += 1
    return BrokenDependencySummary(missing=missing, invalid=invalid)


def _eval_report(
    block_results: list[tuple[str, object]],
) -> EvalReportView:
    blocks: list[EvalBlockView] = []
    failed = 0

    for index, (srctype, result) in enumerate(block_results, start=1):
        ok = bool(getattr(result, "result"))
        if not ok:
            failed += 1
        blocks.append(
            EvalBlockView(
                index=index,
                srctype=srctype,
                ok=ok,
                rtcode=int(getattr(result, "rtcode")),
                stdout=str(getattr(result, "stdout")),
                stderr=str(getattr(result, "stderr")),
            )
        )

    return EvalReportView(blocks=tuple(blocks), failed=failed)


def _get_mdcroot_or_none() -> Path | None:
    mdcroot = find_mdcroot(Path.cwd())
    if mdcroot is None:
        UI.error("not inside an mdoc directory, run `mdc init` first")
        return None
    return mdcroot


def _load_graph_from_ref(cache: IndCache, ref: str) -> tuple[DepGraph, str]:
    return DepGraph.from_ref(cache=cache, ref=ref, cwd=Path.cwd())


def _resolve_ref_item(cache: IndCache, ref: str) -> NodeRef:
    fnode, title, path = cache.resolve_ref(ref, cwd=Path.cwd())
    return _node_ref(
        fnode=fnode,
        title=title,
        rel_path=to_rel_path(cache.root, path),
    )


def _bootstrap_cache(cache: IndCache, *, action: str) -> bool:
    try:
        cache.bootstrap_if_needed()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action=action, exc=exc))
        return False
    return True


def _cmd_init(_: argparse.Namespace) -> int:
    mdcroot = Path.cwd()
    local_mdc = mdcroot / ".mdc"
    config_path = local_mdc / "config.toml"

    if local_mdc.is_dir():
        UI.write(f"Already initialized as mdoc directory: {local_mdc}")
        return 0

    local_mdc.mkdir(parents=False, exist_ok=False)
    try:
        config_path.write_text("", encoding="utf-8")
    except OSError as exc:
        UI.error(f"failed to write config.toml: {exc}")
        return 1
    UI.write("mdoc folder initialized")
    return 0


def _cmd_new(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    target = Path(args.folder).resolve()
    try:
        target.relative_to(mdcroot.resolve())
    except ValueError:
        UI.error(f"target path must be under mdoc root {mdcroot}")
        return 1

    if target.exists() and not target.is_dir():
        UI.error(f"target folder is a file: {target}")
        return 1

    try:
        graph, _ = DepGraph.create_root(
            mdcroot=mdcroot,
            folder=args.folder,
            title=args.title,
        )
    except OSError as exc:
        UI.error(f"failed to save mdoc file: {exc}")
        return 1

    cache = IndCache(mdcroot)
    try:
        cache.bootstrap_if_needed()
        cache.upsert_path(graph.root_path())
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("mdoc was created", exc)

    root_item = graph.root_item()
    UI.write_lines(
        UI.render_created_lines(
            path=str(graph.root_path()),
            root_item=_node_ref_from_item(root_item),
        )
    )
    return 0


def _cmd_search(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    query = args.query.strip()
    if not query:
        UI.error("query cannot be empty")
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare search index"):
        return 1

    try:
        matches = cache.search(query)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action="search mdocs", exc=exc))
        return 1
    UI.write_lines(
        UI.render_search_results_lines(
            query=args.query,
            matches=[_node_ref_from_row(row) for row in matches],
        )
    )
    return 0


def _cmd_eval(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare eval index"):
        return 1

    try:
        graph, src_rel = _load_graph_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to load mdoc: {exc}")
        return 1

    root_item = graph.root_item()
    source_item = _node_ref_from_item(root_item, rel_path=src_rel)
    try:
        dep_items = graph.dependency_items(depth=args.depth)
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(_cycle_view(graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=source_item,
                message=f"failed to inspect dependencies: {exc}",
            )
        )
        return 1

    UI.write_lines(
        UI.render_chain_lines(
            _chain_view(
                anchor_label="source",
                anchor=source_item,
                count_label="dependencies",
                items=dep_items,
                graph=graph,
            )
        )
    )

    broken_summary = _broken_dependency_summary(dep_items, graph)
    if broken_summary.total > 0:
        UI.write_lines(
            UI.render_broken_dependency_warning_lines(
                summary=broken_summary,
                for_eval=True,
            )
        )
        return 1

    if not graph.root_has_blocks():
        UI.write("No blocks to eval")
        return 0

    try:
        block_results = graph.eval_blocks(
            depth=args.depth,
            reverse_depens=args.reverse,
        )
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(_cycle_view(graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.error(f"failed to eval mdoc: {exc}")
        return 1
    report = _eval_report(block_results)
    UI.write_lines(UI.render_eval_results_lines(report))
    return 1 if report.failed else 0


def _cmd_dep_add(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    query = args.query.strip()
    if not query:
        UI.error("query cannot be empty")
        return 1
    if args.max_results < 1:
        UI.error("--max-results must be >= 1")
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        graph, src_rel = _load_graph_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to load mdoc: {exc}")
        return 1
    root_item = graph.root_item()
    source_item = _node_ref_from_item(root_item, rel_path=src_rel)
    try:
        match_rows = cache.search(query)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(
            UI.render_index_error_lines(action="search dependency candidates", exc=exc)
        )
        return 1
    match_rows = [row for row in match_rows if row[0] != source_item.fnode]
    match_rows = match_rows[: args.max_results]
    if not match_rows:
        UI.write(f"No dependency candidates for: {args.query}")
        return 0

    matches = [_node_ref_from_row(row) for row in match_rows]

    try:
        selected_indices = select_indices_interactive(matches)
    except RuntimeError as exc:
        UI.error(str(exc))
        return 1

    if selected_indices is None:
        UI.write("Canceled")
        return 0
    if not selected_indices:
        UI.write("No dependencies selected")
        return 0

    selected_rows = [match_rows[idx] for idx in selected_indices]
    selected_by_fnode = {row[0]: row for row in selected_rows}
    selected_fnodes = list(selected_by_fnode.keys())

    try:
        cache.refresh_rows(list(selected_by_fnode.values()))
        refreshed_by_fnode = cache.lookup_by_fnode(selected_fnodes)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("dependencies were inspected", exc)
        refreshed_by_fnode = {}

    for dep_fnode in selected_fnodes:
        refreshed = refreshed_by_fnode.get(dep_fnode)
        if refreshed is None:
            continue
        selected_by_fnode[dep_fnode] = (dep_fnode, refreshed[0], refreshed[1])

    try:
        added, skipped_existing, skipped_self = graph.add_direct_dependencies(
            list(selected_by_fnode),
        )
    except OSError as exc:
        UI.error(f"failed to save mdoc: {exc}")
        return 1

    UI.write_lines(
        UI.render_dep_add_lines(
            DepAddView(
                source=source_item,
                added=tuple(
                    _node_ref_from_row(selected_by_fnode[dep_fnode], broken=False)
                    for dep_fnode in added
                ),
                skipped_existing=len(skipped_existing),
                skipped_self=len(skipped_self),
            )
        )
    )
    return 0


def _cmd_dep_show(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        graph, src_rel = _load_graph_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to load mdoc: {exc}")
        return 1

    root_item = graph.root_item()
    source_item = _node_ref_from_item(root_item, rel_path=src_rel)
    try:
        dep_items = graph.dependency_items(depth=args.depth)
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(_cycle_view(graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=source_item,
                message=f"failed to inspect dependencies: {exc}",
            )
        )
        return 1

    dep_rows = [(item.fnode, item.title, item.rel_path) for item in dep_items]
    if dep_rows:
        try:
            cache.refresh_rows(dep_rows)
        except (OSError, ValueError, sqlite3.Error) as exc:
            UI.warn_index_failure("dependencies were inspected", exc)

    UI.write_lines(
        UI.render_chain_lines(
            _chain_view(
                anchor_label="source",
                anchor=source_item,
                count_label="dependencies",
                items=dep_items,
                graph=graph,
            )
        )
    )
    broken_lines = UI.render_broken_dependency_warning_lines(
        summary=_broken_dependency_summary(dep_items, graph),
        for_eval=False,
    )
    if broken_lines:
        UI.write_lines(broken_lines)
    return 0


def _cmd_dep_rm(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        graph, src_rel = _load_graph_from_ref(cache, args.source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to load mdoc: {exc}")
        return 1

    root_item = graph.root_item()
    source_item = _node_ref_from_item(root_item, rel_path=src_rel)
    try:
        dep_items = graph.direct_dependency_items()
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=source_item,
                message=f"failed to inspect dependencies: {exc}",
            )
        )
        return 1

    if not dep_items:
        UI.write_lines(
            UI.render_anchor_message_lines(
                label="source",
                item=source_item,
                message="No dependencies to remove",
            )
        )
        return 0

    dep_refs = [
        _node_ref_from_item(item, broken=graph.is_broken_fnode(item.fnode))
        for item in dep_items
    ]
    error_indices = {
        idx
        for idx, item in enumerate(dep_refs)
        if item.broken
    }
    broken_lines = UI.render_broken_dependency_warning_lines(
        summary=_broken_dependency_summary(dep_items, graph),
        for_eval=False,
    )
    if broken_lines:
        UI.write_lines(broken_lines)

    try:
        selected_indices = select_indices_interactive(
            dep_refs,
            error_indices=error_indices,
        )
    except RuntimeError as exc:
        UI.error(str(exc))
        return 1

    if selected_indices is None:
        UI.write("Canceled")
        return 0
    if not selected_indices:
        UI.write("No dependencies selected")
        return 0

    selected_fnodes: list[str] = []
    selected_set: set[str] = set()
    selected_rows_by_fnode: dict[str, NodeRef] = {}
    for idx in selected_indices:
        row = dep_refs[idx]
        dep_fnode = row.fnode
        if dep_fnode in selected_set:
            continue
        selected_set.add(dep_fnode)
        selected_fnodes.append(dep_fnode)
        selected_rows_by_fnode[dep_fnode] = row

    try:
        cache.refresh_rows(
            [
                (row.fnode, row.title, row.rel_path)
                for row in selected_rows_by_fnode.values()
            ]
        )
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("dependencies were inspected", exc)

    try:
        removed_fnodes = graph.remove_direct_dependencies(selected_fnodes)
    except OSError as exc:
        UI.error(f"failed to save mdoc: {exc}")
        return 1
    if not removed_fnodes:
        UI.write("No dependencies removed")
        return 0

    UI.write_lines(
        UI.render_dep_rm_lines(
            DepRmView(
                source=source_item,
                removed=tuple(
                    selected_rows_by_fnode[dep_fnode]
                    for dep_fnode in removed_fnodes
                    if dep_fnode in selected_rows_by_fnode
                ),
            )
        )
    )
    return 0


def _cmd_dep_refs(args: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare dependency index"):
        return 1

    try:
        target_item = _resolve_ref_item(cache, args.target)
    except (ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to resolve mdoc: {exc}")
        return 1

    graph = DepGraph(mdcroot=mdcroot, root_fnode=target_item.fnode, cache=cache)
    try:
        ref_items = graph.referrer_items(depth=args.depth)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect referrers: {exc}")
        return 1

    broken_target = graph.issue_for_fnode(target_item.fnode)
    if broken_target is not None:
        target_item = _node_ref(
            fnode=broken_target.fnode,
            title=broken_target.title,
            rel_path=broken_target.rel_path,
            broken=True,
        )

    UI.write_lines(
        UI.render_chain_lines(
            _chain_view(
                anchor_label="target",
                anchor=target_item,
                count_label="referrers",
                items=ref_items,
                graph=graph,
            )
        )
    )
    return 0


def _cmd_graph_check(_: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not _bootstrap_cache(cache, action="prepare graph index"):
        return 1

    graph = DepGraph(mdcroot=mdcroot, cache=cache)
    try:
        report = graph.graph_check_report()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(UI.render_graph_check_lines(_graph_check_view(graph, report)))

    return 1 if (report.missing or report.invalid or report.cycles) else 0


def _cmd_sync(_: argparse.Namespace) -> int:
    mdcroot = _get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    try:
        cache.refresh_all()
        total = cache.count()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action="sync index", exc=exc))
        return 1
    UI.write_lines(UI.render_synced_lines(total))
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
        UI.error(str(exc))
        return 1

    editor_raw = os.environ.get("EDITOR", "").strip()
    if not editor_raw:
        UI.error("$EDITOR is not set")
        return 1
    editor_cmd = shlex.split(editor_raw)
    if not editor_cmd:
        UI.error("$EDITOR is empty")
        return 1

    try:
        edit_proc = subprocess.run([*editor_cmd, str(src_path)], check=False)
    except OSError as exc:
        UI.error(f"failed to launch $EDITOR: {exc}")
        return 1

    if edit_proc.returncode != 0:
        UI.error(f"editor exited with code {edit_proc.returncode}")
        return edit_proc.returncode

    try:
        cache.upsert_path(src_path)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("mdoc was edited", exc)

    UI.write_lines(UI.render_edited_lines(to_rel_path(mdcroot, src_path)))
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
    graph_check_parser.set_defaults(func=_cmd_graph_check)

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
    dep_refs_parser.set_defaults(func=_cmd_dep_refs)

    return parser


def main() -> int:
    args = _build_parser().parse_args()
    return args.func(args)
