from dataclasses import dataclass


@dataclass(slots=True, frozen=True)
class DependencyItem:
    depth: int
    fnode: str
    title: str
    rel_path: str


@dataclass(slots=True, frozen=True)
class GraphRootItem:
    fnode: str
    title: str
    rel_path: str
    component_size: int
    broken: bool = False


@dataclass(slots=True, frozen=True)
class GraphIssue:
    kind: str
    fnode: str
    title: str
    rel_path: str
    error: str


@dataclass(slots=True, frozen=True)
class GraphCheckReport:
    nodes: int
    edges: int
    missing: list[GraphIssue]
    invalid: list[GraphIssue]
    cycles: list[list[str]]


@dataclass(slots=True)
class DependencyTraversalReport:
    root_fnode: str
    items: list[DependencyItem]
    dep_graph: dict[str, list[str]]
    issues_by_fnode: dict[str, GraphIssue]
