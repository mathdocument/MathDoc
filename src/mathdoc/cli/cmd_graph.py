import argparse
import sqlite3

from .common import UI, prepare_cache_env
from .presenters import graph_check_view, graph_roots_view


def cmd_graph_check(args: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    _, cache = env

    try:
        if args.full:
            cache.refresh_all()
        report = cache.graph_check_report()
        cycle_fnodes = sorted(
            {
                fnode
                for cycle in report.cycles
                for fnode in (cycle[:-1] if len(cycle) > 1 else cycle)
            }
        )
        cycle_rows_by_fnode = cache.lookup_by_fnode(cycle_fnodes)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(
        UI.render_graph_check_lines(
            graph_check_view(report, cycle_rows_by_fnode=cycle_rows_by_fnode)
        )
    )
    return 1 if (report.missing or report.invalid or report.cycles) else 0


def cmd_graph_roots(args: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    _, cache = env

    try:
        if args.refresh:
            cache.refresh_all()
        items = cache.global_root_items()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph roots: {exc}")
        return 1

    UI.write_lines(UI.render_graph_roots_lines(graph_roots_view(items)))
    return 0
