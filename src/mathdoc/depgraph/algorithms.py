from collections.abc import Mapping
from collections.abc import Sequence


def find_cycle(
    dep_graph: Mapping[str, Sequence[str]],
    *,
    root_fnode: str | None = None,
) -> list[str] | None:
    state: dict[str, int] = {}
    stack: list[str] = []
    stack_idx: dict[str, int] = {}

    def dfs(fnode: str) -> list[str] | None:
        state[fnode] = 1
        stack_idx[fnode] = len(stack)
        stack.append(fnode)

        for dep_fnode in dep_graph.get(fnode, []):
            dep_state = state.get(dep_fnode, 0)
            if dep_state == 0:
                cycle = dfs(dep_fnode)
                if cycle is not None:
                    return cycle
            elif dep_state == 1:
                start = stack_idx[dep_fnode]
                return stack[start:] + [dep_fnode]

        stack.pop()
        stack_idx.pop(fnode, None)
        state[fnode] = 2
        return None

    roots = [root_fnode] if root_fnode is not None else list(dep_graph)
    for fnode in roots:
        if fnode not in dep_graph:
            continue
        if state.get(fnode, 0) != 0:
            continue
        cycle = dfs(fnode)
        if cycle is not None:
            return cycle
    return None


def topo_dependencies_first(
    dep_graph: Mapping[str, Sequence[str]],
    *,
    root_fnode: str,
) -> list[str]:
    visited: set[str] = set()
    order: list[str] = []

    def dfs(fnode: str) -> None:
        if fnode in visited:
            return
        visited.add(fnode)
        for dep_fnode in dep_graph.get(fnode, []):
            dfs(dep_fnode)
        order.append(fnode)

    dfs(root_fnode)
    return order


def strongly_connected_components(
    dep_graph: Mapping[str, Sequence[str]],
) -> list[list[str]]:
    index = 0
    stack: list[str] = []
    on_stack: set[str] = set()
    indices: dict[str, int] = {}
    lowlinks: dict[str, int] = {}
    components: list[list[str]] = []

    def strongconnect(fnode: str) -> None:
        nonlocal index
        indices[fnode] = index
        lowlinks[fnode] = index
        index += 1
        stack.append(fnode)
        on_stack.add(fnode)

        for dep_fnode in dep_graph.get(fnode, []):
            if dep_fnode not in indices:
                strongconnect(dep_fnode)
                lowlinks[fnode] = min(lowlinks[fnode], lowlinks[dep_fnode])
            elif dep_fnode in on_stack:
                lowlinks[fnode] = min(lowlinks[fnode], indices[dep_fnode])

        if lowlinks[fnode] != indices[fnode]:
            return

        component: list[str] = []
        while stack:
            member = stack.pop()
            on_stack.discard(member)
            component.append(member)
            if member == fnode:
                break
        components.append(component)

    for fnode in list(dep_graph):
        if fnode not in indices:
            strongconnect(fnode)
    return components


def component_has_cycle(
    dep_graph: Mapping[str, Sequence[str]],
    component: list[str],
) -> bool:
    if len(component) > 1:
        return True
    if not component:
        return False
    fnode = component[0]
    return fnode in dep_graph.get(fnode, [])


def representative_cycle(
    dep_graph: Mapping[str, Sequence[str]],
    component: list[str],
) -> list[str] | None:
    if not component:
        return None
    if len(component) == 1:
        fnode = component[0]
        if fnode in dep_graph.get(fnode, []):
            return [fnode, fnode]
        return None

    component_set = set(component)
    visited: set[str] = set()
    stack: list[str] = []
    stack_index: dict[str, int] = {}

    def dfs(fnode: str) -> list[str] | None:
        visited.add(fnode)
        stack_index[fnode] = len(stack)
        stack.append(fnode)

        for dep_fnode in dep_graph.get(fnode, []):
            if dep_fnode not in component_set:
                continue
            if dep_fnode not in visited:
                cycle = dfs(dep_fnode)
                if cycle is not None:
                    return cycle
            elif dep_fnode in stack_index:
                start = stack_index[dep_fnode]
                return stack[start:] + [dep_fnode]

        stack.pop()
        stack_index.pop(fnode, None)
        return None

    for fnode in sorted(component):
        if fnode in visited:
            continue
        cycle = dfs(fnode)
        if cycle is not None:
            return cycle
    return None
