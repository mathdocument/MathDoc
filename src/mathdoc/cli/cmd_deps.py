import argparse
import sqlite3

from ..depgraph.exceptions import DependencyCycleError
from ..ui import DepAddView, DepRmView, NodeRef, select_indices_interactive
from .common import (
    UI,
    load_source_graph_context,
    load_target_graph_context,
    prompt_create_dependency_row,
    refresh_rows_or_warn,
    search_match_rows,
)
from .presenters import (
    broken_dependency_summary,
    chain_view,
    cycle_view,
    missing_referrer_views,
    node_ref,
    node_ref_from_item,
    node_ref_from_row,
)


def cmd_dep_add(args: argparse.Namespace) -> int:
    context = load_source_graph_context(
        source=args.source,
        action="prepare dependency index",
    )
    if context is None:
        return 1

    raw_match_rows = search_match_rows(
        context.cache,
        query=args.query,
        action="search dependency candidates",
    )
    if raw_match_rows is None:
        return 1

    excluded_fnodes = {
        context.source_item.fnode,
        *context.graph.direct_dependency_fnodes(),
    }
    match_rows = [row for row in raw_match_rows if row[0] not in excluded_fnodes][
        : args.max_results
    ]

    if not match_rows:
        if raw_match_rows:
            UI.write(f"No new dependency candidates for: {args.query}")
            return 0
        try:
            created_row = prompt_create_dependency_row(
                mdcroot=context.mdcroot,
                cache=context.cache,
            )
        except ValueError as exc:
            UI.error(str(exc))
            return 1
        except FileExistsError as exc:
            UI.error(str(exc))
            return 1
        except OSError as exc:
            UI.error(f"failed to save mdoc file: {exc}")
            return 1
        if created_row is None:
            UI.write(f"No dependency candidates for: {args.query}")
            return 0
        selected_rows = [created_row]
    else:
        matches = [node_ref_from_row(row) for row in match_rows]

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
        context.cache.refresh_rows(list(selected_by_fnode.values()))
        refreshed_by_fnode = context.cache.lookup_by_fnode(selected_fnodes)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("dependencies were inspected", exc)
        refreshed_by_fnode = {}

    for dep_fnode in selected_fnodes:
        refreshed = refreshed_by_fnode.get(dep_fnode)
        if refreshed is None:
            continue
        selected_by_fnode[dep_fnode] = (dep_fnode, refreshed[0], refreshed[1])

    try:
        added, _, _ = context.graph.add_direct_dependencies(list(selected_by_fnode))
    except OSError as exc:
        UI.error(f"failed to save mdoc: {exc}")
        return 1

    UI.write_lines(
        UI.render_dep_add_lines(
            DepAddView(
                source=context.source_item,
                added=tuple(
                    node_ref_from_row(selected_by_fnode[dep_fnode], broken=False)
                    for dep_fnode in added
                ),
            )
        )
    )
    return 0


def cmd_dep_show(args: argparse.Namespace) -> int:
    context = load_source_graph_context(
        source=args.source,
        action="prepare dependency index",
    )
    if context is None:
        return 1

    try:
        dep_items = context.graph.dependency_items(depth=args.depth)
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(cycle_view(context.graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=context.source_item,
                message=f"failed to inspect dependencies: {exc}",
            )
        )
        return 1

    dep_rows = [(item.fnode, item.title, item.rel_path) for item in dep_items]
    refresh_rows_or_warn(
        context.cache,
        dep_rows,
        action="dependencies were inspected",
    )

    UI.write_lines(
        UI.render_chain_lines(
            chain_view(
                anchor_label="source",
                anchor=context.source_item,
                count_label="depens",
                items=dep_items,
                graph=context.graph,
            )
        )
    )
    missing_lines = UI.render_missing_referrer_lines(
        missing_referrer_views(dep_items, context.graph)
    )
    if missing_lines:
        UI.write_lines(missing_lines)
    broken_lines = UI.render_broken_dependency_warning_lines(
        summary=broken_dependency_summary(dep_items, context.graph),
        for_eval=False,
    )
    if broken_lines:
        UI.write_lines(broken_lines)
    return 0


def cmd_dep_leaf(args: argparse.Namespace) -> int:
    context = load_source_graph_context(
        source=args.source,
        action="prepare dependency index",
    )
    if context is None:
        return 1

    try:
        leaf_items = context.graph.leaf_dependency_items()
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(cycle_view(context.graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=context.source_item,
                message=f"failed to inspect leaf dependencies: {exc}",
            )
        )
        return 1

    leaf_rows = [(item.fnode, item.title, item.rel_path) for item in leaf_items]
    refresh_rows_or_warn(
        context.cache,
        leaf_rows,
        action="leaf dependencies were inspected",
    )

    UI.write_lines(
        UI.render_chain_lines(
            chain_view(
                anchor_label="source",
                anchor=context.source_item,
                count_label="leaves",
                items=leaf_items,
                graph=context.graph,
            )
        )
    )
    broken_lines = UI.render_broken_dependency_warning_lines(
        summary=broken_dependency_summary(leaf_items, context.graph),
        for_eval=False,
    )
    if broken_lines:
        UI.write_lines(broken_lines)
    return 0


def cmd_dep_rm(args: argparse.Namespace) -> int:
    context = load_source_graph_context(
        source=args.source,
        action="prepare dependency index",
    )
    if context is None:
        return 1

    try:
        dep_items = context.graph.direct_dependency_items()
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=context.source_item,
                message=f"failed to inspect dependencies: {exc}",
            )
        )
        return 1

    if not dep_items:
        UI.write_lines(
            UI.render_anchor_message_lines(
                label="source",
                item=context.source_item,
                message="No dependencies to remove",
            )
        )
        return 0

    dep_refs = [
        node_ref_from_item(item, broken=context.graph.is_broken_fnode(item.fnode))
        for item in dep_items
    ]
    error_indices = {idx for idx, item in enumerate(dep_refs) if item.broken}
    broken_lines = UI.render_broken_dependency_warning_lines(
        summary=broken_dependency_summary(dep_items, context.graph),
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

    refresh_rows_or_warn(
        context.cache,
        [
            (row.fnode, row.title, row.rel_path)
            for row in selected_rows_by_fnode.values()
        ],
        action="dependencies were inspected",
    )

    try:
        removed_fnodes = context.graph.remove_direct_dependencies(selected_fnodes)
    except OSError as exc:
        UI.error(f"failed to save mdoc: {exc}")
        return 1
    if not removed_fnodes:
        UI.write("No dependencies removed")
        return 0

    UI.write_lines(
        UI.render_dep_rm_lines(
            DepRmView(
                source=context.source_item,
                removed=tuple(
                    selected_rows_by_fnode[dep_fnode]
                    for dep_fnode in removed_fnodes
                    if dep_fnode in selected_rows_by_fnode
                ),
            )
        )
    )
    return 0


def cmd_dep_refs(args: argparse.Namespace) -> int:
    context = load_target_graph_context(
        target=args.target,
        action="prepare dependency index",
    )
    if context is None:
        return 1

    try:
        ref_items = context.graph.referrer_items(depth=args.depth)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect referrers: {exc}")
        return 1

    target_item = context.target_item
    broken_target = context.graph.issue_for_fnode(target_item.fnode)
    if broken_target is not None:
        target_item = node_ref(
            fnode=broken_target.fnode,
            title=broken_target.title,
            rel_path=broken_target.rel_path,
            broken=True,
        )

    UI.write_lines(
        UI.render_chain_lines(
            chain_view(
                anchor_label="target",
                anchor=target_item,
                count_label="refers",
                items=ref_items,
                graph=context.graph,
            )
        )
    )
    return 0
