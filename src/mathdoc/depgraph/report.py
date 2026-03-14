from pathlib import Path

from .state import GraphState
from .models import GraphCheckReport
from .issues import dedupe_issues, sorted_issues
from .algorithms import (
    component_has_cycle,
    representative_cycle,
    strongly_connected_components,
)


class GraphReporter:
    def __init__(self, *, mdcroot: Path, state: GraphState) -> None:
        self.mdcroot = Path(mdcroot).resolve()
        self.state = state

    def graph_check_report(self) -> GraphCheckReport:
        edges = sum(len(dep_fnodes) for dep_fnodes in self.state.dep_graph.values())
        missing = sorted_issues(
            [
                issue
                for issue in self.state.broken_issues.values()
                if issue.kind == "missing"
            ]
        )
        invalid = sorted_issues(
            dedupe_issues(
                [
                    *[
                        issue
                        for issue in self.state.broken_issues.values()
                        if issue.kind == "invalid"
                    ],
                    *self.state.invalid_file_issues,
                ]
            )
        )
        cycles = self._cycles_by_component()
        return GraphCheckReport(
            nodes=self.state.scanned_file_count,
            edges=edges,
            missing=missing,
            invalid=invalid,
            cycles=cycles,
        )

    def _cycles_by_component(self) -> list[list[str]]:
        components = strongly_connected_components(self.state.dep_graph)
        cycles: list[list[str]] = []
        for component in components:
            if not component_has_cycle(self.state.dep_graph, component):
                continue
            cycle = representative_cycle(self.state.dep_graph, component)
            if cycle is not None:
                cycles.append(cycle)
        cycles.sort(key=lambda cycle: tuple(cycle))
        return cycles
