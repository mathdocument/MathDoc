from collections import deque
from pathlib import Path

from .issues import dependency_item_for_fnode
from .issues import dedupe_issues
from .issues import sorted_issues
from .models import DependencyItem, GraphRootItem
from .state import GraphState
from ..utils import to_rel_path


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


def global_root_items_from_graph(
    *,
    mdcroot: Path,
    state: GraphState,
) -> list[GraphRootItem]:
    incoming: set[str] = set()
    for dep_fnodes in state.dep_graph.values():
        incoming.update(dep_fnodes)

    component_sizes = _component_sizes_from_graph(state)
    items: list[GraphRootItem] = []
    for node in sorted(
        state.nodes_by_fnode.values(),
        key=lambda node: (to_rel_path(mdcroot, node.path), node.fnode),
    ):
        if node.fnode in incoming:
            continue
        items.append(
            GraphRootItem(
                fnode=node.fnode,
                title=node.title,
                rel_path=to_rel_path(mdcroot, node.path),
                component_size=component_sizes.get(node.fnode, 1),
            )
        )

    for issue in sorted_issues(dedupe_issues(state.invalid_file_issues)):
        if not (issue.fnode.startswith("<") and issue.fnode.endswith(">")):
            if issue.fnode in incoming:
                continue
        items.append(
            GraphRootItem(
                fnode=issue.fnode,
                title=issue.title,
                rel_path=issue.rel_path,
                component_size=component_sizes.get(issue.fnode, 1),
            )
        )

    items.sort(
        key=lambda item: (
            -item.component_size,
            item.rel_path,
            item.title,
            item.fnode,
        )
    )
    return items


def _component_sizes_from_graph(state: GraphState) -> dict[str, int]:
    adjacency: dict[str, set[str]] = {}

    for src_fnode, dep_fnodes in state.dep_graph.items():
        if src_fnode in state.missing_fnodes:
            continue
        adjacency.setdefault(src_fnode, set())
        for dep_fnode in dep_fnodes:
            if dep_fnode in state.missing_fnodes:
                continue
            adjacency.setdefault(dep_fnode, set())
            adjacency[src_fnode].add(dep_fnode)
            adjacency[dep_fnode].add(src_fnode)

    sizes: dict[str, int] = {}
    seen: set[str] = set()

    for start_fnode in sorted(adjacency):
        if start_fnode in seen:
            continue
        queue: deque[str] = deque([start_fnode])
        component: list[str] = []

        while queue:
            fnode = queue.popleft()
            if fnode in seen:
                continue
            seen.add(fnode)
            component.append(fnode)
            for neighbor in adjacency.get(fnode, ()):
                if neighbor not in seen:
                    queue.append(neighbor)

        size = len(component)
        for fnode in component:
            sizes[fnode] = size

    return sizes
