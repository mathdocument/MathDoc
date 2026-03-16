import argparse
import sqlite3

from ..depgraph import DepGraph
from .common import UI, prepare_cache_env
from .presenters import graph_check_view, graph_roots_view


def cmd_graph_check(_: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    mdcroot, cache = env

    graph = DepGraph(mdcroot=mdcroot, cache=cache)
    try:
        report = graph.graph_check_report()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(UI.render_graph_check_lines(graph_check_view(graph, report)))
    return 1 if (report.missing or report.invalid or report.cycles) else 0


def cmd_graph_roots(_: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    mdcroot, cache = env

    graph = DepGraph(mdcroot=mdcroot, cache=cache)
    try:
        items = graph.global_root_items()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph roots: {exc}")
        return 1

    UI.write_lines(UI.render_graph_roots_lines(graph_roots_view(graph, items)))
    return 0
