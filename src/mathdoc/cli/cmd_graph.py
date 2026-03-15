import argparse
import sqlite3

from ..depgraph import DepGraph
from ..indcache import IndCache
from .common import UI, bootstrap_cache, get_mdcroot_or_none
from .presenters import graph_check_view


def cmd_graph_check(_: argparse.Namespace) -> int:
    mdcroot = get_mdcroot_or_none()
    if mdcroot is None:
        return 1

    cache = IndCache(mdcroot)
    if not bootstrap_cache(cache, action="prepare graph index"):
        return 1

    graph = DepGraph(mdcroot=mdcroot, cache=cache)
    try:
        report = graph.graph_check_report()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(UI.render_graph_check_lines(graph_check_view(graph, report)))
    return 1 if (report.missing or report.invalid or report.cycles) else 0
