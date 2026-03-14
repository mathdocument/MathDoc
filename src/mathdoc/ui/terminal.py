from .models import BrokenDependencySummary
from .models import ChainView
from .models import CycleView
from .models import DepAddView
from .models import DepRmView
from .models import EvalReportView
from .models import GraphCheckView
from .models import IssueView
from .models import NodeRef
from .theme import STYLE
from .theme import colorize
from .theme import short_fnode


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

    def format_node_ref(
        self,
        ref: NodeRef,
        *,
        marker: str = "-",
        include_depth: bool = False,
    ) -> str:
        prefix = f"{marker} " if marker else ""
        line = f"{prefix}{short_fnode(ref.fnode)}    {ref.title} ({ref.rel_path})"
        if include_depth and ref.depth is not None:
            line = f"[{ref.depth}] {line}"
        if ref.broken:
            return colorize(line, STYLE["red"])
        return line

    def format_issue(self, issue: IssueView) -> str:
        return self.format_node_ref(issue.ref)

    def render_anchor_error_lines(
        self,
        *,
        label: str,
        item: NodeRef,
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
        item: NodeRef,
        message: str,
    ) -> list[str]:
        return [
            f"{label}: {self.format_node_ref(item, marker='')}",
            message,
        ]

    def render_chain_lines(self, chain: ChainView) -> list[str]:
        lines = [
            f"{chain.anchor_label}: {self.format_node_ref(chain.anchor, marker='')}",
            f"{chain.count_label}: {len(chain.items)}",
        ]
        for item in chain.items:
            lines.append(self.format_node_ref(item, include_depth=True))
        return lines

    def render_broken_dependency_warning_lines(
        self,
        *,
        summary: BrokenDependencySummary,
        for_eval: bool,
    ) -> list[str]:
        if summary.total <= 0:
            return []

        parts: list[str] = []
        if summary.missing:
            parts.append(f"{summary.missing} missing")
        if summary.invalid:
            parts.append(f"{summary.invalid} invalid")
        detail = f" ({', '.join(parts)})" if parts else ""

        if for_eval:
            return [
                f"Error: broken dependency targets detected{detail}; "
                "remove the broken references with `mdc dep rm` before eval."
            ]
        return [
            f"Warning: detected {summary.total} broken dependency reference(s){detail}; "
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
        matches: list[NodeRef],
    ) -> list[str]:
        if not matches:
            return [f"No results for: {query}"]
        lines = [f"results: {len(matches)}"]
        for match in matches:
            lines.append(self.format_node_ref(match))
        return lines

    def render_created_lines(self, *, path: str, root_item: NodeRef) -> list[str]:
        return [
            f"created: {path}",
            f"fnode: {root_item.fnode}",
            f"title: {root_item.title}",
        ]

    def render_dep_add_lines(self, report: DepAddView) -> list[str]:
        lines = [
            f"source: {self.format_node_ref(report.source, marker='')}",
            f"added: {len(report.added)}",
        ]
        for dep in report.added:
            lines.append(self.format_node_ref(dep, marker="+"))
        if report.skipped_existing:
            lines.append(f"skipped existing: {report.skipped_existing}")
        if report.skipped_self:
            lines.append(f"skipped self: {report.skipped_self}")
        return lines

    def render_dep_rm_lines(self, report: DepRmView) -> list[str]:
        lines = [
            f"source: {self.format_node_ref(report.source, marker='')}",
            f"removed: {len(report.removed)}",
        ]
        for dep in report.removed:
            lines.append(self.format_node_ref(dep, marker="-"))
        return lines

    def render_cycle_lines(self, cycle: CycleView) -> list[str]:
        if not cycle.nodes:
            return ["dependency cycle detected"]

        lines = ["dependency cycle detected:"]
        total = len(cycle.nodes)
        for idx, item in enumerate(cycle.nodes):
            rendered = self.format_node_ref(item, marker="+")
            if total == 1 or idx == 0:
                prefix = "┌─➤"
            elif idx == total - 1:
                prefix = "└──"
            else:
                prefix = "│  "
            lines.append(f"{prefix} {rendered}")
        return lines

    def render_graph_check_lines(self, report: GraphCheckView) -> list[str]:
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
                lines.extend(self.render_cycle_lines(cycle))

        return lines

    def render_eval_results_lines(self, report: EvalReportView) -> list[str]:
        lines = [f"blocks: {len(report.blocks)}", "result:"]

        for block in report.blocks:
            if block.ok:
                lines.append(
                    colorize(f"[{block.index}] {block.srctype}: ok", STYLE["grn"])
                )
            else:
                lines.append(
                    colorize(
                        f"[{block.index}] {block.srctype}: failed ({block.rtcode})",
                        STYLE["red"],
                    )
                )

            if block.stdout:
                for line in block.stdout.rstrip("\n").splitlines():
                    lines.append(f"    {line}")
            if block.stderr:
                for line in block.stderr.rstrip("\n").splitlines():
                    lines.append(f"    ! {line}")
            lines.append("")

        summary_color = STYLE["grn"] if report.failed == 0 else STYLE["red"]
        lines.append(colorize(f"failed: {report.failed}", summary_color))
        return lines

    def render_synced_lines(self, total: int) -> list[str]:
        return [f"synced: {total}"]

    def render_edited_lines(self, rel_path: str) -> list[str]:
        return [f"edited: {rel_path}"]
