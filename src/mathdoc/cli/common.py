import sqlite3
from pathlib import Path
from uuid import uuid4

from ..depgraph import DepGraph
from ..indcache import IndCache
from ..ui import NodeRef, TerminalUI, prompt_new_mdoc_interactive
from ..ui.theme import short_fnode
from ..utils import find_mdcroot, to_rel_path


UI = TerminalUI()


def get_mdcroot_or_none() -> Path | None:
    mdcroot = find_mdcroot(Path.cwd())
    if mdcroot is None:
        UI.error("not inside an mdoc directory, run `mdc init` first")
        return None
    return mdcroot


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
