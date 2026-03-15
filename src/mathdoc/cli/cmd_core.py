import argparse
import os
import shlex
import sqlite3
import subprocess
from pathlib import Path

from ..utils import to_rel_path
from .common import (
    UI,
    create_mdoc,
    get_cache_context_or_none,
    prepare_cache_context,
    search_match_rows,
)
from .presenters import node_ref_from_item, node_ref_from_row


def cmd_init(_: argparse.Namespace) -> int:
    mdcroot = Path.cwd()
    local_mdc = mdcroot / ".mdc"
    config_path = local_mdc / "config.toml"

    if local_mdc.is_dir():
        UI.write(f"Already initialized as mdoc directory: {local_mdc}")
        return 0

    local_mdc.mkdir(parents=False, exist_ok=False)
    try:
        config_path.write_text("", encoding="utf-8")
    except OSError as exc:
        UI.error(f"failed to write config.toml: {exc}")
        return 1
    UI.write("mdoc folder initialized")
    return 0


def cmd_new(args: argparse.Namespace) -> int:
    context = get_cache_context_or_none()
    if context is None:
        return 1

    try:
        graph, _ = create_mdoc(
            mdcroot=context.mdcroot,
            cache=context.cache,
            file_path=args.file,
            title=args.title,
        )
    except ValueError as exc:
        UI.error(str(exc))
        return 1
    except FileExistsError as exc:
        UI.error(str(exc))
        return 1
    except OSError as exc:
        UI.error(f"failed to save mdoc file: {exc}")
        return 1

    root_item = graph.root_item()
    UI.write_lines(
        UI.render_created_lines(
            path=str(graph.root_path()),
            root_item=node_ref_from_item(root_item),
        )
    )
    return 0


def cmd_search(args: argparse.Namespace) -> int:
    context = prepare_cache_context(action="prepare search index")
    if context is None:
        return 1

    matches = search_match_rows(
        context.cache,
        query=args.query,
        max_results=args.max_results,
        action="search mdocs",
    )
    if matches is None:
        return 1

    UI.write_lines(
        UI.render_search_results_lines(
            query=args.query,
            matches=[node_ref_from_row(row) for row in matches],
        )
    )
    return 0


def cmd_sync(_: argparse.Namespace) -> int:
    context = get_cache_context_or_none()
    if context is None:
        return 1

    try:
        context.cache.refresh_all()
        total = context.cache.count()
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.write_lines(UI.render_index_error_lines(action="sync index", exc=exc))
        return 1
    UI.write_lines(UI.render_synced_lines(total))
    return 0


def cmd_edit(args: argparse.Namespace) -> int:
    context = prepare_cache_context(action="prepare edit index")
    if context is None:
        return 1

    try:
        src_path = context.cache.resolve_edit_target_path(args.source, cwd=Path.cwd())
    except (ValueError, sqlite3.Error) as exc:
        UI.error(str(exc))
        return 1

    editor_raw = os.environ.get("EDITOR", "").strip()
    if not editor_raw:
        UI.error("$EDITOR is not set")
        return 1
    editor_cmd = shlex.split(editor_raw)
    if not editor_cmd:
        UI.error("$EDITOR is empty")
        return 1

    try:
        edit_proc = subprocess.run([*editor_cmd, str(src_path)], check=False)
    except OSError as exc:
        UI.error(f"failed to launch $EDITOR: {exc}")
        return 1

    if edit_proc.returncode != 0:
        UI.error(f"editor exited with code {edit_proc.returncode}")
        return edit_proc.returncode

    try:
        context.cache.upsert_path(src_path)
    except (OSError, ValueError, sqlite3.Error) as exc:
        UI.warn_index_failure("mdoc was edited", exc)

    UI.write_lines(UI.render_edited_lines(to_rel_path(context.mdcroot, src_path)))
    return 0
