from typing import TYPE_CHECKING

from ..depgraph import DependencyItem
from ..depgraph import GraphCheckReport
from ..depgraph import GraphIssue
from .theme import STYLE
from .theme import colorize
from .theme import short_fnode

if TYPE_CHECKING:
    from ..depgraph import DepGraph
    from ..depgraph.exceptions import DependencyCycleError


class TerminalUI:
    def write(self, text: str = "") -> None:
        print(text)

    def write_lines(self, lines: list[str]) -> None:
        for line in lines:
            self.write(line)

    def error(self, message: str) -> None:
        self.write(f"Error: {message}")

    def warning(self, message: str) -> None:
        self.write(f"Warning: {message}")

    def hint(self, message: str) -> None:
        self.write(f"Hint: {message}")

    def format_ref(
        self,
        *,
        fnode: str,
        title: str,
        rel_path: str,
        marker: str = "-",
        depth: int | None = None,
        broken: bool = False,
    ) -> str:
        prefix = f"{marker} " if marker else ""
        line = f"{prefix}{short_fnode(fnode)}    {title} ({rel_path})"
        if depth is not None:
            line = f"[{depth}] {line}"
        if broken:
            return colorize(line, STYLE["red"])
        return line

    def format_item(
        self,
        item: DependencyItem,
        *,
        graph: "DepGraph | None" = None,
        marker: str = "-",
        include_depth: bool = False,
    ) -> str:
        broken = graph.is_broken_fnode(item.fnode) if graph is not None else False
        return self.format_ref(
            fnode=item.fnode,
            title=item.title,
            rel_path=item.rel_path,
            marker=marker,
            depth=item.depth if include_depth else None,
            broken=broken,
        )

    def format_issue(self, issue: GraphIssue) -> str:
        return self.format_ref(
            fnode=issue.fnode,
            title=issue.title,
            rel_path=issue.rel_path,
            broken=True,
        )

    def render_anchor_error_lines(
        self,
        *,
        label: str,
        item: DependencyItem,
        message: str,
    ) -> list[str]:
        return self.render_anchor_message_lines(
            label=label,
            item=item,
            message=f"Error: {message}",
        )

    def render_anchor_message_lines(
        self,
        *,
        label: str,
        item: DependencyItem,
        message: str,
    ) -> list[str]:
        return [
            f"{label}: {self.format_item(item, marker='')}",
            message,
        ]

    def render_dependency_chain_lines(
        self,
        *,
        source_item: DependencyItem,
        dep_items: list[DependencyItem],
        graph: "DepGraph",
    ) -> list[str]:
        return self._render_chain_lines(
            anchor_label="source",
            anchor_item=source_item,
            count_label="dependencies",
            items=dep_items,
            graph=graph,
        )

    def render_referrer_chain_lines(
        self,
        *,
        target_item: DependencyItem,
        ref_items: list[DependencyItem],
        graph: "DepGraph",
    ) -> list[str]:
        return self._render_chain_lines(
            anchor_label="target",
            anchor_item=target_item,
            count_label="referrers",
            items=ref_items,
            graph=graph,
        )

    def _render_chain_lines(
        self,
        *,
        anchor_label: str,
        anchor_item: DependencyItem,
        count_label: str,
        items: list[DependencyItem],
        graph: "DepGraph",
    ) -> list[str]:
        lines = [
            f"{anchor_label}: {self.format_item(anchor_item, marker='')}",
            f"{count_label}: {len(items)}",
        ]
        for item in items:
            lines.append(self.format_item(item, graph=graph, include_depth=True))
        return lines

    def render_broken_dependency_warning_lines(
        self,
        *,
        dep_items: list[DependencyItem],
        graph: "DepGraph",
        for_eval: bool,
    ) -> list[str]:
        missing_count, invalid_count = self._broken_dependency_counts(
            dep_items=dep_items,
            graph=graph,
        )
        total = missing_count + invalid_count
        if total <= 0:
            return []

        parts: list[str] = []
        if missing_count:
            parts.append(f"{missing_count} missing")
        if invalid_count:
            parts.append(f"{invalid_count} invalid")
        detail = f" ({', '.join(parts)})" if parts else ""

        if for_eval:
            return [
                f"Error: broken dependency targets detected{detail}; "
                "remove the broken references with `mdc dep rm` before eval."
            ]
        return [
            f"Warning: detected {total} broken dependency reference(s){detail}; "
            "broken rows are highlighted in red when the terminal supports color."
        ]

    def render_index_error_lines(self, *, action: str, exc: Exception) -> list[str]:
        return [
            f"Error: failed to {action}: {exc}",
            "Hint: run `mdc sync` to rebuild the index; "
            "if it still fails, remove `.mdc/index.db` and retry.",
        ]

    def warn_index_failure(self, action: str, exc: Exception) -> None:
        self.write_lines(
            [
                f"Warning: {action}, but index refresh failed: {exc}",
                "Warning: search results may be stale, run `mdc sync` to rebuild the index.",
            ]
        )

    def render_search_results_lines(
        self,
        *,
        query: str,
        matches: list[tuple[str, str, str]],
    ) -> list[str]:
        if not matches:
            return [f"No results for: {query}"]
        lines = [f"results: {len(matches)}"]
        for fnode, title, rel_path in matches:
            lines.append(
                self.format_ref(
                    fnode=fnode,
                    title=title,
                    rel_path=rel_path,
                )
            )
        return lines

    def render_created_lines(self, *, path: str, root_item: DependencyItem) -> list[str]:
        return [
            f"created: {path}",
            f"fnode: {root_item.fnode}",
            f"title: {root_item.title}",
        ]

    def render_dep_add_lines(
        self,
        *,
        source_item: DependencyItem,
        added: list[str],
        selected_by_fnode: dict[str, tuple[str, str, str]],
        skipped_existing: list[str],
        skipped_self: list[str],
    ) -> list[str]:
        lines = [
            f"source: {self.format_item(source_item, marker='')}",
            f"added: {len(added)}",
        ]
        for dep_fnode in added:
            dep_row = selected_by_fnode[dep_fnode]
            lines.append(
                self.format_ref(
                    fnode=dep_fnode,
                    title=dep_row[1],
                    rel_path=dep_row[2],
                    marker="+",
                )
            )
        if skipped_existing:
            lines.append(f"skipped existing: {len(skipped_existing)}")
        if skipped_self:
            lines.append(f"skipped self: {len(skipped_self)}")
        return lines

    def render_dep_rm_lines(
        self,
        *,
        source_item: DependencyItem,
        removed_fnodes: list[str],
        selected_rows_by_fnode: dict[str, tuple[str, str, str]],
        graph: "DepGraph",
    ) -> list[str]:
        lines = [
            f"source: {self.format_item(source_item, marker='')}",
            f"removed: {len(removed_fnodes)}",
        ]
        for dep_fnode in removed_fnodes:
            row = selected_rows_by_fnode.get(dep_fnode)
            if row is None:
                continue
            lines.append(
                self.format_ref(
                    fnode=row[0],
                    title=row[1],
                    rel_path=row[2],
                    marker="-",
                    broken=graph.is_broken_fnode(dep_fnode),
                )
            )
        return lines

    def render_cycle_lines(
        self,
        *,
        graph: "DepGraph",
        cycle: list[str],
    ) -> list[str]:
        if not cycle:
            return ["dependency cycle detected"]

        lines = ["dependency cycle detected:"]
        cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
        total = len(cycle_nodes)
        for idx, fnode in enumerate(cycle_nodes):
            item = graph.ref_item_for_fnode(fnode)
            rendered = self.format_item(item, graph=graph, marker="+")
            if total == 1:
                prefix = "┌─➤"
            elif idx == 0:
                prefix = "┌─➤"
            elif idx == total - 1:
                prefix = "└──"
            else:
                prefix = "│  "
            lines.append(f"{prefix} {rendered}")
        return lines

    def render_cycle_error_lines(
        self,
        *,
        graph: "DepGraph",
        exc: "DependencyCycleError",
    ) -> list[str]:
        return self.render_cycle_lines(graph=graph, cycle=exc.cycle)

    def render_graph_check_lines(
        self,
        *,
        report: GraphCheckReport,
        graph: "DepGraph",
    ) -> list[str]:
        lines = [
            f"nodes: {report.nodes}",
            f"edges: {report.edges}",
            f"missing: {len(report.missing)}",
            f"invalid: {len(report.invalid)}",
            f"cycles: {len(report.cycles)}",
        ]

        if report.missing:
            lines.append("missing dependencies:")
            for issue in report.missing:
                lines.append(self.format_issue(issue))
                lines.append(f"    ! {issue.error}")

        if report.invalid:
            lines.append("invalid mdocs:")
            for issue in report.invalid:
                lines.append(self.format_issue(issue))
                lines.append(f"    ! {issue.error}")

        if report.cycles:
            lines.append("cycles:")
            for index, cycle in enumerate(report.cycles, start=1):
                lines.append(f"[{index}]")
                lines.extend(self.render_cycle_lines(graph=graph, cycle=cycle))

        return lines

    def render_eval_results_lines(
        self,
        *,
        block_results: list[tuple[str, object]],
    ) -> list[str]:
        lines = [f"blocks: {len(block_results)}", "result:"]
        failed = 0

        for index, block in enumerate(block_results, start=1):
            srctype, result = block
            result_ok = bool(getattr(result, "result"))
            stdout = str(getattr(result, "stdout"))
            stderr = str(getattr(result, "stderr"))
            rtcode = int(getattr(result, "rtcode"))

            if result_ok:
                lines.append(colorize(f"[{index}] {srctype}: ok", STYLE["grn"]))
            else:
                failed += 1
                lines.append(
                    colorize(
                        f"[{index}] {srctype}: failed ({rtcode})",
                        STYLE["red"],
                    )
                )

            if stdout:
                for line in stdout.rstrip("\n").splitlines():
                    lines.append(f"    {line}")
            if stderr:
                for line in stderr.rstrip("\n").splitlines():
                    lines.append(f"    ! {line}")
            lines.append("")

        summary_color = STYLE["grn"] if failed == 0 else STYLE["red"]
        lines.append(colorize(f"failed: {failed}", summary_color))
        return lines

    def render_synced_lines(self, total: int) -> list[str]:
        return [f"synced: {total}"]

    def render_edited_lines(self, rel_path: str) -> list[str]:
        return [f"edited: {rel_path}"]

    @staticmethod
    def _broken_dependency_counts(
        *,
        dep_items: list[DependencyItem],
        graph: "DepGraph",
    ) -> tuple[int, int]:
        missing_count = 0
        invalid_count = 0
        for item in dep_items:
            issue = graph.issue_for_fnode(item.fnode)
            if issue is None:
                continue
            if issue.kind == "missing":
                missing_count += 1
            elif issue.kind == "invalid":
                invalid_count += 1
        return missing_count, invalid_count
