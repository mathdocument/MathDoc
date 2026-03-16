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


def leaf_items_from_graph(
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

        dep_fnodes = state.dep_graph.get(fnode, [])
        if not dep_fnodes:
            items.append(
                dependency_item_for_fnode(
                    mdcroot=mdcroot,
                    state=state,
                    fnode=fnode,
                    depth=node_depth,
                )
            )
            continue

        for dep_fnode in dep_fnodes:
            queue.append((dep_fnode, node_depth + 1))

    return items

