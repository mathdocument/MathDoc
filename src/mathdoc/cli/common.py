from collections.abc import Callable
import sqlite3
from pathlib import Path
from uuid import uuid4

from ..depgraph import DepGraph, DependencyItem
from ..depgraph.exceptions import DependencyCycleError
from ..indcache import IndCache
from ..ui import BrokenDependencySummary, NodeRef, TerminalUI, prompt_new_mdoc_interactive
from ..ui.theme import short_fnode
from ..utils import find_mdcroot, to_rel_path
from .presenters import (
    broken_dependency_summary,
    chain_view,
    cycle_view,
    missing_referrer_views,
    node_ref_from_item,
)


UI = TerminalUI()


def get_mdcroot_or_none() -> Path | None:
    mdcroot = find_mdcroot(Path.cwd())
    if mdcroot is None:
        UI.error("not inside an mdoc directory, run `mdc init` first")
        return None
    return mdcroot


def get_cache_env_or_none() -> tuple[Path, IndCache] | None:
    mdcroot = get_mdcroot_or_none()
    if mdcroot is None:
        return None
    return mdcroot, IndCache(mdcroot)


def load_graph_from_ref(cache: IndCache, ref: str) -> tuple[DepGraph, str]:
    return DepGraph.from_ref(cache=cache, ref=ref, cwd=Path.cwd())


def resolve_ref_item(cache: IndCache, ref: str) -> NodeRef:
    fnode, title, path = cache.resolve_ref(ref, cwd=Path.cwd())
    return NodeRef(
        fnode=fnode,
        title=title,
        rel_path=to_rel_path(cache.root, path),
    )


def bootstrap_cache(cache: IndCache, *, action: str) -> bool:
    try:
        cache.bootstrap_if_needed()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action=action, exc=exc))
        return False
    return True


def prepare_cache_env(*, action: str) -> tuple[Path, IndCache] | None:
    env = get_cache_env_or_none()
    if env is None:
        return None
    mdcroot, cache = env
    if not bootstrap_cache(cache, action=action):
        return None
    return mdcroot, cache


def load_source_graph(
    *,
    source: str,
    action: str,
    error_prefix: str = "failed to load mdoc",
) -> tuple[Path, IndCache, DepGraph, NodeRef] | None:
    env = prepare_cache_env(action=action)
    if env is None:
        return None
    mdcroot, cache = env
    try:
        graph, src_rel = load_graph_from_ref(cache, source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"{error_prefix}: {exc}")
        return None
    return (
        mdcroot,
        cache,
        graph,
        node_ref_from_item(graph.root_item(), rel_path=src_rel),
    )


def load_target_graph(
    *,
    target: str,
    action: str,
    resolve_error_prefix: str = "failed to resolve mdoc",
) -> tuple[Path, IndCache, DepGraph, NodeRef] | None:
    env = prepare_cache_env(action=action)
    if env is None:
        return None
    mdcroot, cache = env
    try:
        target_item = resolve_ref_item(cache, target)
    except (ValueError, sqlite3.Error) as exc:
        UI.error(f"{resolve_error_prefix}: {exc}")
        return None
    return (
        mdcroot,
        cache,
        DepGraph(
            mdcroot=mdcroot,
            root_fnode=target_item.fnode,
            cache=cache,
        ),
        target_item,
    )


def refresh_rows_or_warn(
    cache: IndCache,
    rows: list[tuple[str, str, str]],
    *,
    action: str,
) -> None:
    if not rows:
        return
    try:
        cache.refresh_rows(rows)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure(action, exc)


def render_dependency_report(
    *,
    cache: IndCache,
    graph: DepGraph,
    source_item: NodeRef,
    count_label: str,
    refresh_action: str,
    inspect_error_message: str,
    load_items: Callable[[], list[DependencyItem]],
    for_eval: bool,
    show_missing_referrers: bool,
) -> tuple[list[DependencyItem], BrokenDependencySummary] | None:
    try:
        items = load_items()
    except DependencyCycleError as exc:
        UI.write_lines(UI.render_cycle_lines(cycle_view(graph, exc.cycle)))
        return None
    except ValueError as exc:
        UI.write_lines(
            UI.render_anchor_error_lines(
                label="source",
                item=source_item,
                message=f"{inspect_error_message}: {exc}",
            )
        )
        return None

    refresh_rows_or_warn(
        cache,
        [(item.fnode, item.title, item.rel_path) for item in items],
        action=refresh_action,
    )
    UI.write_lines(
        UI.render_chain_lines(
            chain_view(
                anchor_label="source",
                anchor=source_item,
                count_label=count_label,
                items=items,
                graph=graph,
            )
        )
    )
    if show_missing_referrers:
        missing_lines = UI.render_missing_referrer_lines(
            missing_referrer_views(items, graph)
        )
        if missing_lines:
            UI.write_lines(missing_lines)

    summary = broken_dependency_summary(items, graph)
    if summary.total > 0:
        UI.write_lines(
            UI.render_broken_dependency_warning_lines(
                summary=summary,
                for_eval=for_eval,
            )
        )
    return items, summary


def search_match_rows(
    cache: IndCache,
    *,
    query: str,
    action: str,
    max_results: int | None = None,
    exclude_fnodes: set[str] | None = None,
) -> list[tuple[str, str, str]] | None:
    normalized = query.strip()
    if not normalized:
        UI.error("query cannot be empty")
        return None
    if max_results is not None and max_results < 1:
        UI.error("--max-results must be >= 1")
        return None

    try:
        rows = cache.search(normalized)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action=action, exc=exc))
        return None

    if exclude_fnodes:
        rows = [row for row in rows if row[0] not in exclude_fnodes]
    if max_results is None:
        return rows
    return rows[:max_results]


def create_mdoc(
    *,
    mdcroot: Path,
    cache: IndCache,
    file_path: str = ".",
    title: str = "Untitled",
    fnode: str | None = None,
) -> tuple[DepGraph, str]:
    graph, rel_path = DepGraph.create_root(
        mdcroot=mdcroot,
        file_path=file_path,
        title=title,
        fnode=fnode,
        cache=cache,
    )
    try:
        cache.bootstrap_if_needed()
        cache.upsert_path(graph.root_path())
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("mdoc was created", exc)
    return graph, rel_path


def prompt_create_dependency_row(
    *,
    mdcroot: Path,
    cache: IndCache,
) -> tuple[str, str, str] | None:
    pending_fnode = str(uuid4())
    try:
        creation_input = prompt_new_mdoc_interactive(
            default_filename_display=f"{short_fnode(pending_fnode)}...",
        )
    except RuntimeError:
        return None

    if creation_input is None:
        return None

    file_path, title = creation_input
    created_graph, created_rel = create_mdoc(
        mdcroot=mdcroot,
        cache=cache,
        file_path=file_path,
        title=title,
        fnode=pending_fnode,
    )
    created_item = created_graph.root_item()
    return (created_item.fnode, created_item.title, created_rel)
