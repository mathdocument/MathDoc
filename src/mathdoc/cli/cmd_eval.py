import argparse

from ..depgraph.exceptions import DependencyCycleError
from .common import UI, load_source_graph_context
from .presenters import (
    broken_dependency_summary,
    chain_view,
    cycle_view,
    eval_block_view,
    missing_referrer_views,
)


def cmd_eval(args: argparse.Namespace) -> int:
    context = load_source_graph_context(
        source=args.source,
        action="prepare eval index",
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

    broken_summary = broken_dependency_summary(dep_items, context.graph)
    if broken_summary.total > 0:
        UI.write_lines(
            UI.render_broken_dependency_warning_lines(
                summary=broken_summary,
                for_eval=True,
            )
        )
        return 1

    if not context.graph.root_has_blocks():
        UI.write("No blocks to eval")
        return 0

    failed = 0

    def _on_eval_start(index: int, total: int, srctype: str) -> None:
        UI.write_lines(
            UI.render_eval_block_start_lines(
                index=index,
                total=total,
                srctype=srctype,
            )
        )

    def _on_eval_result(
        index: int,
        total: int,
        srctype: str,
        result: object,
    ) -> None:
        nonlocal failed
        block_view = eval_block_view(
            index=index,
            total=total,
            srctype=srctype,
            result=result,
        )
        if not block_view.ok:
            failed += 1
        UI.write_lines(UI.render_eval_block_finish_lines(block_view))
        if index < total:
            UI.write()

    try:
        context.graph.eval_blocks(
            depth=args.depth,
            progress=UI.info,
            on_start=_on_eval_start,
            on_result=_on_eval_result,
        )
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(cycle_view(context.graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.error(f"failed to eval mdoc: {exc}")
        return 1
    return 1 if failed else 0
