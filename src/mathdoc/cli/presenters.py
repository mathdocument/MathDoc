from ..depgraph import DependencyItem, GraphCheckReport, GraphIssue, GraphRootItem
from ..depgraph.graph import DepGraph
from ..ui import (
    BrokenDependencySummary,
    ChainView,
    CycleView,
    EvalBlockView,
    GraphCheckView,
    GraphRootEntryView,
    GraphRootsView,
    IssueView,
    MissingReferrerView,
    NodeRef,
)


def node_ref(
    *,
    fnode: str,
    title: str,
    rel_path: str,
    depth: int | None = None,
    broken: bool = False,
) -> NodeRef:
    return NodeRef(
        fnode=fnode,
        title=title,
        rel_path=rel_path,
        depth=depth,
        broken=broken,
    )


def node_ref_from_item(
    item: DependencyItem,
    *,
    rel_path: str | None = None,
    broken: bool = False,
) -> NodeRef:
    return node_ref(
        fnode=item.fnode,
        title=item.title,
        rel_path=rel_path or item.rel_path,
        depth=item.depth,
        broken=broken,
    )


def node_ref_from_row(
    row: tuple[str, str, str],
    *,
    depth: int | None = None,
    broken: bool = False,
) -> NodeRef:
    return node_ref(
        fnode=row[0],
        title=row[1],
        rel_path=row[2],
        depth=depth,
        broken=broken,
    )


def issue_view(issue: GraphIssue) -> IssueView:
    return IssueView(
        ref=node_ref(
            fnode=issue.fnode,
            title=issue.title,
            rel_path=issue.rel_path,
            broken=True,
        ),
        error=issue.error,
    )


def cycle_view(
    cycle: list[str],
    *,
    graph: DepGraph | None = None,
    ref_rows_by_fnode: dict[str, tuple[str, str]] | None = None,
) -> CycleView:
    cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
    nodes: list[NodeRef] = []
    for fnode in cycle_nodes:
        if graph is not None:
            nodes.append(
                node_ref_from_item(
                    graph.ref_item_for_fnode(fnode),
                    broken=graph.is_broken_fnode(fnode),
                )
            )
            continue

        title, rel_path = ("<missing>", "<unknown>")
        if ref_rows_by_fnode is not None:
            title, rel_path = ref_rows_by_fnode.get(fnode, (title, rel_path))
        nodes.append(
            node_ref(
                fnode=fnode,
                title=title,
                rel_path=rel_path,
            )
        )

    return CycleView(nodes=tuple(nodes))


def graph_check_view(
    report: GraphCheckReport,
    *,
    graph: DepGraph | None = None,
    cycle_rows_by_fnode: dict[str, tuple[str, str]] | None = None,
) -> GraphCheckView:
    return GraphCheckView(
        nodes=report.nodes,
        edges=report.edges,
        missing=tuple(issue_view(issue) for issue in report.missing),
        invalid=tuple(issue_view(issue) for issue in report.invalid),
        cycles=tuple(
            cycle_view(
                cycle,
                graph=graph,
                ref_rows_by_fnode=cycle_rows_by_fnode,
            )
            for cycle in report.cycles
        ),
    )


def graph_roots_view(items: list[GraphRootItem]) -> GraphRootsView:
    return GraphRootsView(
        roots=tuple(
            GraphRootEntryView(
                ref=node_ref(
                    fnode=item.fnode,
                    title=item.title,
                    rel_path=item.rel_path,
                    broken=item.broken,
                ),
                component_size=item.component_size,
            )
            for item in items
        )
    )


def chain_view(
    *,
    anchor_label: str,
    anchor: NodeRef,
    count_label: str,
    items: list[DependencyItem],
    graph: DepGraph | None = None,
) -> ChainView:
    return ChainView(
        anchor_label=anchor_label,
        anchor=anchor,
        count_label=count_label,
        items=tuple(
            node_ref_from_item(
                item,
                broken=graph.is_broken_fnode(item.fnode) if graph is not None else False,
            )
            for item in items
        ),
    )


def broken_dependency_summary(
    dep_items: list[DependencyItem],
    graph: DepGraph,
) -> BrokenDependencySummary:
    missing = 0
    invalid = 0
    for item in dep_items:
        issue = graph.issue_for_fnode(item.fnode)
        if issue is None:
            continue
        if issue.kind == "missing":
            missing += 1
        elif issue.kind == "invalid":
            invalid += 1
    return BrokenDependencySummary(missing=missing, invalid=invalid)


def missing_referrer_views(
    dep_items: list[DependencyItem],
    graph: DepGraph,
) -> tuple[MissingReferrerView, ...]:
    reverse_graph: dict[str, list[str]] = {}
    for src_fnode, dep_fnodes in graph.dep_graph.items():
        for dep_fnode in dep_fnodes:
            refs = reverse_graph.setdefault(dep_fnode, [])
            if src_fnode not in refs:
                refs.append(src_fnode)

    views: list[MissingReferrerView] = []
    seen_missing: set[str] = set()
    for item in dep_items:
        issue = graph.issue_for_fnode(item.fnode)
        if issue is None or issue.kind != "missing" or item.fnode in seen_missing:
            continue
        seen_missing.add(item.fnode)
        referrers = tuple(
            node_ref_from_item(
                graph.ref_item_for_fnode(ref_fnode),
                broken=graph.is_broken_fnode(ref_fnode),
            )
            for ref_fnode in reverse_graph.get(item.fnode, [])
        )
        if not referrers:
            continue
        views.append(
            MissingReferrerView(
                target=node_ref_from_item(item, broken=True),
                referrers=referrers,
            )
        )
    return tuple(views)


def eval_block_view(
    *,
    index: int,
    total: int,
    srctype: str,
    result: object,
) -> EvalBlockView:
    return EvalBlockView(
        index=index,
        total=total,
        srctype=srctype,
        ok=bool(getattr(result, "result")),
        rtcode=int(getattr(result, "rtcode")),
        stdout=str(getattr(result, "stdout")),
        stderr=str(getattr(result, "stderr")),
    )
