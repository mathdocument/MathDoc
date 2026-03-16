import argparse
import sqlite3

from ..depgraph.exceptions import DependencyCycleError
from .common import UI, emit_dependency_report, load_source_graph, refresh_rows_or_warn
from .presenters import cycle_view, eval_block_view


def cmd_eval(args: argparse.Namespace) -> int:
    env = load_source_graph(
        source=args.source,
        action="prepare eval index",
    )
    if env is None:
        return 1
    _, cache, graph, source_item = env

    try:
        cache.upsert_path(graph.root_path())
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("source mdoc was inspected", exc)

    try:
        report = cache.dependency_report(
            root_fnode=source_item.fnode,
            depth=args.depth,
        )
    except DependencyCycleError as exc:
        cycle_rows = cache.lookup_by_fnode(list(dict.fromkeys(exc.cycle)))
        UI.write_lines(
            UI.render_cycle_lines(
                cycle_view(
                    exc.cycle,
                    ref_rows_by_fnode=dict(cycle_rows),
                )
            )
        )
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

    refresh_rows_or_warn(
        cache,
        [(item.fnode, item.title, item.rel_path) for item in report.items],
        action="dependencies were inspected",
    )

    try:
        report = cache.dependency_report(
            root_fnode=source_item.fnode,
            depth=args.depth,
        )
    except DependencyCycleError as exc:
        cycle_rows = cache.lookup_by_fnode(list(dict.fromkeys(exc.cycle)))
        UI.write_lines(
            UI.render_cycle_lines(
                cycle_view(
                    exc.cycle,
                    ref_rows_by_fnode=dict(cycle_rows),
                )
            )
        )
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

    broken_summary = emit_dependency_report(
        source_item=source_item,
        count_label="depens",
        report=report,
        for_eval=True,
        show_missing_referrers=True,
    )
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
            dep_items=report.items,
            progress=UI.info,
            on_start=_on_eval_start,
            on_result=_on_eval_result,
        )
    except DependencyCycleError as exc:
        UI.write_lines(
            UI.render_cycle_lines(cycle_view(exc.cycle, graph=graph))
        )
        return 1
    except ValueError as exc:
        UI.error(f"failed to eval mdoc: {exc}")
        return 1
    return 1 if failed else 0
