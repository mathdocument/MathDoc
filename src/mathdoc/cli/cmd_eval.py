import argparse

from ..depgraph.exceptions import DependencyCycleError
from .common import UI, load_source_graph, render_dependency_report
from .presenters import cycle_view, eval_block_view


def cmd_eval(args: argparse.Namespace) -> int:
    env = load_source_graph(
        source=args.source,
        action="prepare eval index",
    )
    if env is None:
        return 1
    _, cache, graph, source_item = env

    report = render_dependency_report(
        cache=cache,
        graph=graph,
        source_item=source_item,
        count_label="depens",
        refresh_action="dependencies were inspected",
        inspect_error_message="failed to inspect dependencies",
        load_items=lambda: graph.dependency_items(depth=args.depth),
        for_eval=True,
        show_missing_referrers=True,
    )
    if report is None:
        return 1
    _, broken_summary = report

    if broken_summary.total > 0:
        return 1

    if not graph.root_has_blocks():
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
        graph.eval_blocks(
            depth=args.depth,
            progress=UI.info,
            on_start=_on_eval_start,
            on_result=_on_eval_result,
        )
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(cycle_view(graph, exc.cycle)))
        return 1
    except ValueError as exc:
        UI.error(f"failed to eval mdoc: {exc}")
        return 1
    return 1 if failed else 0
