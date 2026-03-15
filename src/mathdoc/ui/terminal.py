from .theme import short_fnode, colorize, STYLE, LAYOUT
from .models import (
    NodeRef,
    IssueView,
    GraphCheckView,
    EvalReportView,
    EvalBlockView,
    DepRmView,
    DepAddView,
    CycleView,
    ChainView,
    BrokenDependencySummary,
    MissingReferrerView,
)


def _label_width(*labels: str) -> int:
    return max((len(label) + 1 for label in labels), default=0)


def _clip_text(text: str, width: int) -> str:
    if width <= 0:
        return ""
    if len(text) <= width:
        return text.ljust(width)
    if width <= 1:
        return text[:width]
    return text[: width - 1] + "…"


def _ref_prefix(*, marker: str, depth: int | None) -> str:
    if depth is None:
        prefix = f"{marker} " if marker else ""
        return prefix.ljust(4)
    return f"[{depth}] {marker} "


class TerminalUI:
    def write(self, text: str = "") -> None:
        print(text)

    def write_lines(self, lines: list[str]) -> None:
        for line in lines:
            self.write(line)

    def error(self, message: str) -> None:
        self.write(f"{self._status_tag('Error', STYLE['red'])} {message}")

    def warning(self, message: str) -> None:
        self.write(f"{self._status_tag('Warning', STYLE['org'])} {message}")

    def hint(self, message: str) -> None:
        self.write(f"{self._status_tag('Hint', STYLE['blu'])} {message}")

    def info(self, message: str) -> None:
        self.write(f"{self._status_tag('Info', STYLE['cyn'])} {message}")

    def _status_tag(self, text: str, tone: str) -> str:
        return colorize(f"{text}:", STYLE["bld"], tone)

    def _label(
        self,
        text: str,
        width: int,
        *,
        tone: str = STYLE["cyn"],
        align: str = "right",
    ) -> str:
        raw = f"{text}:"
        if align == "right":
            cell = raw.rjust(width)
        else:
            cell = raw.ljust(width)
        return colorize(cell, STYLE["bld"], tone)

    def _metric(
        self,
        label: str,
        value: int | str,
        detail: str,
        *,
        width: int,
        tone: str,
    ) -> str:
        value_text = colorize(str(value), STYLE["bld"], tone)
        head = f"{self._label(label, width, align='right')}  {value_text}"
        return f"{head}  {detail}" if detail else head

    def _render_fnode(self, fnode_text: str) -> str:
        return colorize(fnode_text, STYLE["dim"])

    def format_node_ref(
        self,
        ref: NodeRef,
        *,
        marker: str = "-",
        include_depth: bool = False,
    ) -> str:
        prefix = _ref_prefix(
            marker=marker,
            depth=ref.depth if include_depth else None,
        )
        fnode_text = f"{short_fnode(ref.fnode):<{LAYOUT['ref_fnode_width']}}"
        title_text = _clip_text(ref.title, LAYOUT["ref_title_width"])
        line = f"{prefix}{fnode_text}{title_text}  ({ref.rel_path})"
        if ref.broken:
            return colorize(line, STYLE["bld"], STYLE["red"])

        rendered_prefix = colorize(prefix, STYLE["blu"], STYLE["bld"]) if prefix else ""
        rendered_fnode = self._render_fnode(fnode_text)
        rendered_title = colorize(title_text, STYLE["bld"])
        rendered_path = colorize(f"({ref.rel_path})", STYLE["dim"], STYLE["blu"])
        return f"{rendered_prefix}{rendered_fnode}{rendered_title}  {rendered_path}"

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
            message=f"{self._status_tag('Error', STYLE['red'])} {message}",
        )

    def render_anchor_message_lines(
        self,
        *,
        label: str,
        item: NodeRef,
        message: str,
    ) -> list[str]:
        width = _label_width(label)
        return [
            f"{self._label(label, width)} {self.format_node_ref(item, marker='')}",
            message,
        ]

    def render_chain_lines(self, chain: ChainView) -> list[str]:
        width = _label_width(chain.anchor_label, chain.count_label)
        indent = " " * (width + 1)
        lines = [
            f"{self._label(chain.anchor_label, width)}   {self.format_node_ref(chain.anchor, marker='')}",
            self._metric(
                chain.count_label,
                len(chain.items),
                "reachable node(s)",
                width=width,
                tone=STYLE["blu"],
            ),
        ]
        for item in chain.items:
            lines.append(f"{indent}{self.format_node_ref(item, include_depth=True)}")
        return lines

    def render_missing_referrer_lines(
        self,
        reports: tuple[MissingReferrerView, ...],
    ) -> list[str]:
        if not reports:
            return []

        width = _label_width("missing")
        indent = " " * (width + 1)
        lines = [
            self._metric(
                "missing",
                len(reports),
                "unresolved target(s)",
                width=width,
                tone=STYLE["org"],
            )
        ]

        for index, report in enumerate(reports):
            if index:
                lines.append("")
            lines.append(f"{indent}{self.format_node_ref(report.target, marker='-')}")
            lines.append(
                f"{indent}{colorize('referred by:', STYLE['bld'], STYLE['blu'])}"
            )
            for referrer in report.referrers:
                lines.append(f"{indent}{self.format_node_ref(referrer, marker='-')}")
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
                f"{self._status_tag('Error', STYLE['red'])} broken dependency targets detected{detail}; "
                "remove the broken references with `mdc dep rm` before eval."
            ]
        return [
            f"{self._status_tag('Warning', STYLE['org'])} detected {summary.total} broken dependency reference(s){detail}; "
            "broken rows are highlighted in red when the terminal supports color."
        ]

    def render_index_error_lines(self, *, action: str, exc: Exception) -> list[str]:
        return [
            f"{self._status_tag('Error', STYLE['red'])} failed to {action}: {exc}",
            f"{self._status_tag('Hint', STYLE['blu'])} run `mdc sync` to rebuild the index; "
            "if it still fails, remove `.mdc/index.db` and retry.",
        ]

    def warn_index_failure(self, action: str, exc: Exception) -> None:
        self.warning(f"{action}, but index refresh failed: {exc}")
        self.warning(
            "search results may be stale, run `mdc sync` to rebuild the index."
        )

    def render_search_results_lines(
        self,
        *,
        query: str,
        matches: list[NodeRef],
    ) -> list[str]:
        if not matches:
            return [f"No results for: {query}"]
        lines = [
            self._metric(
                "results",
                len(matches),
                "matching mdoc(s)",
                width=_label_width("results"),
                tone=STYLE["grn"],
            )
        ]
        for match in matches:
            lines.append(self.format_node_ref(match))
        return lines

    def render_created_lines(self, *, path: str, root_item: NodeRef) -> list[str]:
        width = _label_width("created", "fnode", "title")
        return [
            f"{self._label('created', width)} {colorize(path, STYLE['blu'], STYLE['bld'])}",
            f"{self._label('fnode', width)} {self._render_fnode(root_item.fnode)}",
            f"{self._label('title', width)} {colorize(root_item.title, STYLE['bld'])}",
        ]

    def render_dep_add_lines(self, report: DepAddView) -> list[str]:
        width = _label_width("source", "added")
        indent = " " * (width + 1)
        lines = [
            f"{self._label('source', width)} {self.format_node_ref(report.source, marker='')}",
            self._metric(
                "added",
                len(report.added),
                "direct dependency update(s)",
                width=width,
                tone=STYLE["grn"],
            ),
        ]
        for dep in report.added:
            lines.append(f"{indent}{self.format_node_ref(dep, marker='+')}")
        return lines

    def render_dep_rm_lines(self, report: DepRmView) -> list[str]:
        width = _label_width("source", "remove")
        indent = " " * (width + 1)
        lines = [
            f"{self._label('source', width)} {self.format_node_ref(report.source, marker='')}",
            self._metric(
                "remove",
                len(report.removed),
                "direct dependency removed",
                width=width,
                tone=STYLE["org"],
            ),
        ]
        for dep in report.removed:
            lines.append(f"{indent}{self.format_node_ref(dep, marker='-')}")
        return lines

    def render_cycle_lines(self, cycle: CycleView) -> list[str]:
        if not cycle.nodes:
            return ["dependency cycle detected"]

        lines = [f"{self._status_tag('Error', STYLE['red'])} dependency cycle detected"]
        total = len(cycle.nodes)
        for idx, item in enumerate(cycle.nodes):
            rendered = self.format_node_ref(item, marker="+")
            if total == 1 or idx == 0:
                prefix = "┌─➤"
            elif idx == total - 1:
                prefix = "└──"
            else:
                prefix = "│  "
            lines.append(f"{colorize(prefix, STYLE['blu'], STYLE['bld'])} {rendered}")
        return lines

    def render_graph_check_lines(self, report: GraphCheckView) -> list[str]:
        width = _label_width("nodes", "edges", "missing", "invalid", "cycles")
        lines = [
            self._metric(
                "nodes",
                report.nodes,
                "scanned mdoc file(s)",
                width=width,
                tone=STYLE["blu"],
            ),
            self._metric(
                "edges",
                report.edges,
                "dependency edge(s), broken refs included",
                width=width,
                tone=STYLE["blu"],
            ),
            self._metric(
                "missing",
                len(report.missing),
                "unresolved target(s)",
                width=width,
                tone=STYLE["org"] if report.missing else STYLE["grn"],
            ),
            self._metric(
                "invalid",
                len(report.invalid),
                "invalid mdoc file(s)",
                width=width,
                tone=STYLE["org"] if report.invalid else STYLE["grn"],
            ),
            self._metric(
                "cycles",
                len(report.cycles),
                "cyclic component(s)",
                width=width,
                tone=STYLE["red"] if report.cycles else STYLE["grn"],
            ),
        ]

        if report.missing:
            lines.append(colorize("missing:", STYLE["bld"], STYLE["org"]))
            for issue in report.missing:
                lines.append(f"  {self.format_issue(issue)}")
                lines.append(
                    f"    {colorize('!', STYLE['bld'], STYLE['org'])} {issue.error}"
                )

        if report.invalid:
            lines.append(colorize("invalid:", STYLE["bld"], STYLE["org"]))
            for issue in report.invalid:
                lines.append(f"  {self.format_issue(issue)}")
                lines.append(
                    f"    {colorize('!', STYLE['bld'], STYLE['org'])} {issue.error}"
                )

        if report.cycles:
            lines.append(colorize("cycles:", STYLE["bld"], STYLE["red"]))
            for index, cycle in enumerate(report.cycles, start=1):
                cycle_lines = self.render_cycle_lines(cycle)
                if not cycle_lines:
                    continue
                lines.append(
                    f"{colorize(f'[{index}]', STYLE['bld'], STYLE['red'])} {cycle_lines[0]}"
                )
                for cycle_line in cycle_lines[1:]:
                    lines.append(f"    {cycle_line}")

        return lines

    def render_eval_results_lines(self, report: EvalReportView) -> list[str]:
        lines: list[str] = []
        for index, block in enumerate(report.blocks, start=1):
            lines.extend(self.render_eval_block_lines(block))
            if index < len(report.blocks):
                lines.append("")
        return lines

    def render_eval_block_start_lines(
        self,
        *,
        index: int,
        total: int,
        srctype: str,
    ) -> list[str]:
        return [
            colorize(
                f"[{index}/{total}] {srctype}:",
                STYLE["bld"],
                STYLE["cyn"],
            )
        ]

    def render_eval_block_lines(self, block: EvalBlockView) -> list[str]:
        lines = self.render_eval_block_start_lines(
            index=block.index,
            total=block.total,
            srctype=block.srctype,
        )
        lines.extend(self.render_eval_block_finish_lines(block))
        return lines

    def render_eval_block_finish_lines(self, block: EvalBlockView) -> list[str]:
        lines: list[str] = []
        if block.stdout:
            for line in block.stdout.rstrip("\n").splitlines():
                lines.append(f"    {line}")
        if block.stderr:
            for line in block.stderr.rstrip("\n").splitlines():
                lines.append(f"    ! {line}")

        if block.ok:
            lines.append(
                colorize(
                    f"[{block.index}/{block.total}] OK",
                    STYLE["bld"],
                    STYLE["grn"],
                )
            )
        else:
            lines.append(
                colorize(
                    f"[{block.index}/{block.total}] FAIL ({block.rtcode})",
                    STYLE["bld"],
                    STYLE["red"],
                )
            )
        return lines

    def render_synced_lines(self, total: int) -> list[str]:
        return [f"synced: {total}"]

    def render_edited_lines(self, rel_path: str) -> list[str]:
        width = _label_width("edited")
        return [
            f"{self._label('edited', width)} {colorize(rel_path, STYLE['blu'], STYLE['bld'])}"
        ]
