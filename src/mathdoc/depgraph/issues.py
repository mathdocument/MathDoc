from pathlib import Path

from .models import DependencyItem
from .models import GraphIssue
from .state import GraphState
from ..utils import to_rel_path


def is_broken_fnode(state: GraphState, fnode: str) -> bool:
    return fnode in state.missing_fnodes or fnode in state.invalid_fnodes


def issue_for_fnode(state: GraphState, fnode: str) -> GraphIssue | None:
    return state.broken_issues.get(fnode)


def dependency_item_for_fnode(
    *,
    mdcroot: Path,
    state: GraphState,
    fnode: str,
    depth: int,
) -> DependencyItem:
    node = state.nodes_by_fnode.get(fnode)
    if node is not None:
        return DependencyItem(
            depth=depth,
            fnode=node.fnode,
            title=node.title,
            rel_path=to_rel_path(mdcroot, node.path),
        )

    issue = state.broken_issues.get(fnode)
    if issue is not None:
        return DependencyItem(
            depth=depth,
            fnode=issue.fnode,
            title=issue.title,
            rel_path=issue.rel_path,
        )

    return DependencyItem(
        depth=depth,
        fnode=fnode,
        title="<missing>",
        rel_path="<unknown>",
    )


def mark_missing(state: GraphState, fnode: str) -> None:
    state.missing_fnodes.add(fnode)
    state.invalid_fnodes.discard(fnode)
    state.nodes_by_fnode.pop(fnode, None)
    state.dep_graph.setdefault(fnode, [])
    state.broken_issues[fnode] = GraphIssue(
        kind="missing",
        fnode=fnode,
        title="<missing>",
        rel_path="<unknown>",
        error=f"no mdoc matched reference: {fnode}",
    )


def make_invalid_issue(
    *,
    mdcroot: Path,
    path: Path,
    error: str,
    fnode: str,
) -> GraphIssue:
    return GraphIssue(
        kind="invalid",
        fnode=fnode,
        title="<invalid>",
        rel_path=to_rel_path(mdcroot, path),
        error=error,
    )


def record_invalid_issue(state: GraphState, issue: GraphIssue) -> None:
    upsert_issue(state.invalid_file_issues, issue)
    if issue.fnode.startswith("<") and issue.fnode.endswith(">"):
        return
    state.invalid_fnodes.add(issue.fnode)
    state.missing_fnodes.discard(issue.fnode)
    state.nodes_by_fnode.pop(issue.fnode, None)
    state.dep_graph.setdefault(issue.fnode, [])
    state.broken_issues[issue.fnode] = issue


def clear_broken_issue(state: GraphState, fnode: str) -> None:
    state.missing_fnodes.discard(fnode)
    state.invalid_fnodes.discard(fnode)
    state.broken_issues.pop(fnode, None)


def upsert_issue(issues: list[GraphIssue], issue: GraphIssue) -> None:
    key = (issue.kind, issue.fnode, issue.rel_path)
    for index, existing in enumerate(issues):
        existing_key = (existing.kind, existing.fnode, existing.rel_path)
        if existing_key == key:
            issues[index] = issue
            return
    issues.append(issue)


def dedupe_issues(issues: list[GraphIssue]) -> list[GraphIssue]:
    deduped: list[GraphIssue] = []
    for issue in issues:
        upsert_issue(deduped, issue)
    return deduped


def sorted_issues(issues: list[GraphIssue]) -> list[GraphIssue]:
    return sorted(
        issues,
        key=lambda issue: (issue.rel_path, issue.fnode, issue.error),
    )
