from dataclasses import dataclass


@dataclass(slots=True, frozen=True)
class DependencyItem:
    depth: int
    fnode: str
    title: str
    rel_path: str


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
