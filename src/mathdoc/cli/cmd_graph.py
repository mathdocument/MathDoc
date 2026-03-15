import argparse
import sqlite3

from ..depgraph import DepGraph
from .common import UI, prepare_cache_context
from .presenters import graph_check_view


def cmd_graph_check(_: argparse.Namespace) -> int:
    context = prepare_cache_context(action="prepare graph index")
    if context is None:
        return 1

    graph = DepGraph(mdcroot=context.mdcroot, cache=context.cache)
    try:
        report = graph.graph_check_report()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(UI.render_graph_check_lines(graph_check_view(graph, report)))
    return 1 if (report.missing or report.invalid or report.cycles) else 0
