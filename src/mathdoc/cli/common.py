from dataclasses import dataclass
import sqlite3
from pathlib import Path
from uuid import uuid4

from ..depgraph import DepGraph
from ..indcache import IndCache
from ..ui import NodeRef, TerminalUI, prompt_new_mdoc_interactive
from ..ui.theme import short_fnode
from ..utils import find_mdcroot, to_rel_path
from .presenters import node_ref_from_item


UI = TerminalUI()


@dataclass(slots=True, frozen=True)
class CacheContext:
    mdcroot: Path
    cache: IndCache


@dataclass(slots=True, frozen=True)
class SourceGraphContext:
    mdcroot: Path
    cache: IndCache
    graph: DepGraph
    source_item: NodeRef


@dataclass(slots=True, frozen=True)
class TargetGraphContext:
    mdcroot: Path
    cache: IndCache
    graph: DepGraph
    target_item: NodeRef


def get_mdcroot_or_none() -> Path | None:
    mdcroot = find_mdcroot(Path.cwd())
    if mdcroot is None:
        UI.error("not inside an mdoc directory, run `mdc init` first")
        return None
    return mdcroot


def get_cache_context_or_none() -> CacheContext | None:
    mdcroot = get_mdcroot_or_none()
    if mdcroot is None:
        return None
    return CacheContext(mdcroot=mdcroot, cache=IndCache(mdcroot))


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


def prepare_cache_context(*, action: str) -> CacheContext | None:
    context = get_cache_context_or_none()
    if context is None:
        return None
    if not bootstrap_cache(context.cache, action=action):
        return None
    return context


def load_source_graph_context(
    *,
    source: str,
    action: str,
    error_prefix: str = "failed to load mdoc",
) -> SourceGraphContext | None:
    context = prepare_cache_context(action=action)
    if context is None:
        return None
    try:
        graph, src_rel = load_graph_from_ref(context.cache, source)
    except (FileNotFoundError, OSError, ValueError, sqlite3.Error) as exc:
        UI.error(f"{error_prefix}: {exc}")
        return None
    return SourceGraphContext(
        mdcroot=context.mdcroot,
        cache=context.cache,
        graph=graph,
        source_item=node_ref_from_item(graph.root_item(), rel_path=src_rel),
    )


def load_target_graph_context(
    *,
    target: str,
    action: str,
    resolve_error_prefix: str = "failed to resolve mdoc",
) -> TargetGraphContext | None:
    context = prepare_cache_context(action=action)
    if context is None:
        return None
    try:
        target_item = resolve_ref_item(context.cache, target)
    except (ValueError, sqlite3.Error) as exc:
        UI.error(f"{resolve_error_prefix}: {exc}")
        return None
    return TargetGraphContext(
        mdcroot=context.mdcroot,
        cache=context.cache,
        graph=DepGraph(
            mdcroot=context.mdcroot,
            root_fnode=target_item.fnode,
            cache=context.cache,
        ),
        target_item=target_item,
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
