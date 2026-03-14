from collections import deque
from pathlib import Path

from .issues import dependency_item_for_fnode
from .models import DependencyItem
from .state import GraphState


def dependency_items_from_graph(
    *,
    mdcroot: Path,
    state: GraphState,
    root_fnode: str,
) -> list[DependencyItem]:
    items: list[DependencyItem] = []
    seen: set[str] = set()
    queue: deque[tuple[str, int]] = deque(
        (dep_fnode, 1) for dep_fnode in state.dep_graph.get(root_fnode, [])
    )

    while queue:
        fnode, node_depth = queue.popleft()
        if fnode in seen:
            continue
        seen.add(fnode)

        items.append(
            dependency_item_for_fnode(
                mdcroot=mdcroot,
                state=state,
                fnode=fnode,
                depth=node_depth,
            )
        )

        for dep_fnode in state.dep_graph.get(fnode, []):
            queue.append((dep_fnode, node_depth + 1))

    return items


def referrer_items_from_graph(
    *,
    mdcroot: Path,
    state: GraphState,
    target_fnode: str,
    depth: int,
) -> list[DependencyItem]:
    reverse_graph: dict[str, list[str]] = {}
    for src_fnode, dep_fnodes in state.dep_graph.items():
        for dep_fnode in dep_fnodes:
            reverse_graph.setdefault(dep_fnode, []).append(src_fnode)

    items: list[DependencyItem] = []
    seen: set[str] = {target_fnode}
    queue: deque[tuple[str, int]] = deque(
        (ref_fnode, 1) for ref_fnode in reverse_graph.get(target_fnode, [])
    )

    while queue:
        fnode, item_depth = queue.popleft()
        if fnode in seen:
            continue
        seen.add(fnode)
        items.append(
            dependency_item_for_fnode(
                mdcroot=mdcroot,
                state=state,
                fnode=fnode,
                depth=item_depth,
            )
        )

        if depth != -1 and item_depth >= depth:
            continue
        for ref_fnode in reverse_graph.get(fnode, []):
            if ref_fnode == target_fnode:
                continue
            queue.append((ref_fnode, item_depth + 1))

    return items
