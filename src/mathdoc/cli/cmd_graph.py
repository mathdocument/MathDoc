import argparse
import sqlite3

from .common import UI, prepare_cache_env
from .presenters import graph_check_view_from_cache, graph_roots_view_from_cache


def cmd_graph_check(_: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    _, cache = env

    try:
        cache.refresh_all()
        report = cache.graph_check_report()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph: {exc}")
        return 1

    UI.write_lines(UI.render_graph_check_lines(graph_check_view_from_cache(cache, report)))
    return 1 if (report.missing or report.invalid or report.cycles) else 0


def cmd_graph_roots(_: argparse.Namespace) -> int:
    env = prepare_cache_env(action="prepare graph index")
    if env is None:
        return 1
    _, cache = env

    try:
        cache.refresh_all()
        items = cache.global_root_items()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"failed to inspect graph roots: {exc}")
        return 1

    UI.write_lines(UI.render_graph_roots_lines(graph_roots_view_from_cache(cache, items)))
    return 0
