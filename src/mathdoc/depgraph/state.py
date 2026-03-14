from dataclasses import dataclass
from dataclasses import field

from ..mdocnode import MdocNode
from .models import GraphIssue


@dataclass(slots=True)
class GraphState:
    root_fnode: str = ""
    dep_graph: dict[str, list[str]] = field(default_factory=dict)
    nodes_by_fnode: dict[str, MdocNode] = field(default_factory=dict)
    missing_fnodes: set[str] = field(default_factory=set)
    invalid_fnodes: set[str] = field(default_factory=set)
    broken_issues: dict[str, GraphIssue] = field(default_factory=dict)
    invalid_file_issues: list[GraphIssue] = field(default_factory=list)
    scanned_file_count: int = 0

    def reset_graph(self) -> None:
        self.dep_graph.clear()
        self.nodes_by_fnode.clear()
        self.missing_fnodes.clear()
        self.invalid_fnodes.clear()
        self.broken_issues.clear()
        self.invalid_file_issues.clear()
        self.scanned_file_count = 0
