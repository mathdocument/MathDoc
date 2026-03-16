import argparse
import sqlite3

from ..core import DependencyCycleError, DependencyTraversalReport
from ..indcache import IndCache
from .common import UI, emit_dependency_report, load_source_graph
from .presenters import cycle_view, eval_block_view


def _report_signature(
    report: DependencyTraversalReport,
) -> tuple[
    tuple[tuple[int, str, str, str], ...],
    tuple[tuple[str, tuple[str, ...]], ...],
    tuple[tuple[str, str, str, str, str], ...],
]:
    return (
        tuple(
            (item.depth, item.fnode, item.title, item.rel_path)
            for item in report.items
        ),
        tuple(
            (fnode, tuple(dep_fnodes))
            for fnode, dep_fnodes in sorted(report.dep_graph.items())
        ),
        tuple(
            sorted(
                (
                    fnode,
                    issue.kind,
                    issue.title,
                    issue.rel_path,
                    issue.error,
                )
                for fnode, issue in report.issues_by_fnode.items()
            )
        ),
    )


def _stabilized_eval_report(
    *,
    cache: IndCache,
    root_fnode: str,
    depth: int,
) -> DependencyTraversalReport:
    previous_signature: object | None = None

    for _ in range(32):
        report = cache.dependency_report(
            root_fnode=root_fnode,
            depth=depth,
        )
        signature = _report_signature(report)
        if signature == previous_signature:
            return report

        rows = [
            (item.fnode, item.title, item.rel_path)
            for item in report.items
            if not (item.rel_path.startswith("<") and item.rel_path.endswith(">"))
        ]
        if rows:
            try:
                cache.refresh_rows(rows)
            except (OSError, ValueError, sqlite3.Error) as exc:
                raise ValueError(
                    f"failed to refresh reachable dependency index: {exc}"
                ) from exc

        previous_signature = signature

    raise ValueError(
        "dependency index did not converge after refreshing reachable dependencies; "
        "run `mdc sync` and retry"
    )


def cmd_eval(args: argparse.Namespace) -> int:
    env = load_source_graph(
        source=args.source,
        action="prepare eval index",
        discover_changes=True,
    )
    if env is None:
        return 1
    _, cache, graph, source_item = env

    try:
        cache.upsert_path(graph.root_path())
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("source mdoc was inspected", exc)
    try:
        cache.refresh_reachable_from_path(
            root_path=graph.root_path(),
            depth=args.depth,
        )
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(
            UI.render_index_error_lines(
                action="refresh dependency index",
                exc=exc,
            )
        )
        return 1

    try:
        report = _stabilized_eval_report(
            cache=cache,
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
