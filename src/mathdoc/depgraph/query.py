from collections import deque
from pathlib import Path

from ..core import DependencyItem
from .issues import dependency_item_for_fnode
from .state import GraphState


def dependency_items_from_graph(
    *,
    mdcroot: Path,
    state: GraphState,
    root_fnode: str,
    leaf_only: bool = False,
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
        if leaf_only and dep_fnodes:
            for dep_fnode in dep_fnodes:
                queue.append((dep_fnode, node_depth + 1))
            continue

        items.append(
            dependency_item_for_fnode(
                mdcroot=mdcroot,
                state=state,
                fnode=fnode,
                depth=node_depth,
            )
        )

        if not leaf_only:
            for dep_fnode in dep_fnodes:
                queue.append((dep_fnode, node_depth + 1))

    return items
