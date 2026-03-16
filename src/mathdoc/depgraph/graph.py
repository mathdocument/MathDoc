import sqlite3
from collections.abc import Callable
from pathlib import Path

from .algorithms import find_cycle
from .evaluate import GraphEvaluator
from .exceptions import DependencyCycleError
from .issues import is_broken_fnode, issue_for_fnode, dependency_item_for_fnode
from .loading import GraphLoader, create_root_node, load_root_node_from_ref
from .models import DependencyItem, GraphCheckReport, GraphIssue, GraphRootItem
from .query import (
    dependency_items_from_graph,
    leaf_items_from_graph,
)
from .state import GraphState
from ..compiler import CompilerRes
from ..indcache import IndCache
from ..mdocnode import MdocNode
from ..utils import to_rel_path


class DepGraph:
    @classmethod
    def create_root(
        cls,
        *,
        mdcroot: Path,
        file_path: str = ".",
        title: str = "Untitled",
        fnode: str | None = None,
        cache: IndCache | None = None,
    ) -> tuple["DepGraph", str]:
        node, rel_path = create_root_node(
            mdcroot=mdcroot,
            file_path=file_path,
            title=title,
            fnode=fnode,
        )
        graph = cls(mdcroot=mdcroot, root_node=node, cache=cache)
        return graph, rel_path

    @classmethod
    def from_ref(
        cls,
        *,
        cache: IndCache,
        ref: str,
        cwd: Path | None = None,
    ) -> tuple["DepGraph", str]:
        node, rel_path = load_root_node_from_ref(
            cache=cache,
            ref=ref,
            cwd=cwd,
        )
        graph = cls(mdcroot=cache.root, root_node=node, cache=cache)
        return graph, rel_path

    def __init__(
        self,
        *,
        mdcroot: Path,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
        cache: IndCache | None = None,
    ) -> None:
        self.mdcroot = Path(mdcroot).resolve()
        self.cache = cache or IndCache(self.mdcroot)
        self.state = GraphState()
        self._loader = GraphLoader(
            mdcroot=self.mdcroot,
            cache=self.cache,
            state=self.state,
        )
        self._evaluator = GraphEvaluator(
            mdcroot=self.mdcroot,
            state=self.state,
        )

        if root_node is not None:
            self.set_root_node(root_node)
        if root_fnode is not None:
            self.set_root_fnode(root_fnode)

    @property
    def root_fnode(self) -> str:
        return self.state.root_fnode

    @root_fnode.setter
    def root_fnode(self, value: str) -> None:
        self.state.root_fnode = value

    @property
    def dep_graph(self) -> dict[str, list[str]]:
        return self.state.dep_graph

    @property
    def nodes_by_fnode(self) -> dict[str, MdocNode]:
        return self.state.nodes_by_fnode

    @property
    def missing_fnodes(self) -> set[str]:
        return self.state.missing_fnodes

    @property
    def invalid_fnodes(self) -> set[str]:
        return self.state.invalid_fnodes

    @property
    def broken_issues(self) -> dict[str, GraphIssue]:
        return self.state.broken_issues

    @property
    def invalid_file_issues(self) -> list[GraphIssue]:
        return self.state.invalid_file_issues

    @property
    def scanned_file_count(self) -> int:
        return self.state.scanned_file_count

    def set_root_node(self, node: MdocNode) -> None:
        if node.mdcroot.resolve() != self.mdcroot:
            raise ValueError(
                f"mdoc node root mismatch: {node.mdcroot.resolve()} != {self.mdcroot}"
            )
        self.state.nodes_by_fnode[node.fnode] = node
        self.state.dep_graph.setdefault(node.fnode, [])
        if self.root_fnode and self.root_fnode != node.fnode:
            raise ValueError(f"root fnode mismatch: {self.root_fnode} != {node.fnode}")
        self.root_fnode = node.fnode

    def set_root_fnode(self, fnode: str) -> None:
        value = fnode.strip()
        if not value:
            raise ValueError("root fnode cannot be empty")
        if self.root_fnode and self.root_fnode != value:
            raise ValueError(f"root fnode mismatch: {self.root_fnode} != {value}")
        self.root_fnode = value

    def get_root_node(self) -> MdocNode:
        return self._loader.ensure_node_loaded(self._bind_root())

    def root_path(self) -> Path:
        return self.get_root_node().path

    def root_has_blocks(self) -> bool:
        return bool(self.get_root_node().blocks)

    def root_item(self) -> DependencyItem:
        issue = self.state.broken_issues.get(self.root_fnode)
        if issue is not None:
            return DependencyItem(
                depth=0,
                fnode=issue.fnode,
                title=issue.title,
                rel_path=issue.rel_path,
            )
        node = self.get_root_node()
        return DependencyItem(
            depth=0,
            fnode=node.fnode,
            title=node.title,
            rel_path=to_rel_path(self.mdcroot, node.path),
        )

    def is_broken_fnode(self, fnode: str) -> bool:
        if self._has_local_graph_state(fnode):
            return is_broken_fnode(self.state, fnode)
        return self.cache.is_broken_fnode(fnode)

    def issue_for_fnode(self, fnode: str) -> GraphIssue | None:
        if self._has_local_graph_state(fnode):
            return issue_for_fnode(self.state, fnode)
        return self.cache.issue_for_fnode(fnode)

    def ref_item_for_fnode(self, fnode: str, *, depth: int = 0) -> DependencyItem:
        if fnode in self.state.nodes_by_fnode or fnode in self.state.broken_issues:
            return dependency_item_for_fnode(
                mdcroot=self.mdcroot,
                state=self.state,
                fnode=fnode,
                depth=depth,
            )
        return self.cache.ref_item_for_fnode(fnode, depth=depth)

    def direct_dependency_fnodes(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[str]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._loader.ensure_node_loaded(active_root)
        return self._dedupe_keep_order(node.depens)

    def direct_dependency_items(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[DependencyItem]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._loader.ensure_node_loaded(active_root)
        dep_items: list[DependencyItem] = []

        for dep_fnode in self._dedupe_keep_order(node.depens):
            if dep_fnode not in self.state.nodes_by_fnode:
                dep_node = self._loader.load_node(
                    dep_fnode,
                    tolerate_missing=True,
                    tolerate_invalid=True,
                )
                if dep_node is not None:
                    self.state.nodes_by_fnode[dep_fnode] = dep_node
            self.state.dep_graph.setdefault(active_root, [])
            self.state.dep_graph.setdefault(dep_fnode, [])
            dep_items.append(self.ref_item_for_fnode(dep_fnode, depth=1))

        self.state.dep_graph[active_root] = self._dedupe_keep_order(node.depens)
        return dep_items

    def add_direct_dependencies(
        self,
        dep_fnodes: list[str],
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> tuple[list[str], list[str], list[str]]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._loader.ensure_node_loaded(active_root)

        added: list[str] = []
        skipped_existing: list[str] = []
        skipped_self: list[str] = []
        existing = set(self.direct_dependency_fnodes(root_fnode=active_root))

        for dep_fnode in self._dedupe_keep_order(dep_fnodes):
            if dep_fnode == node.fnode:
                skipped_self.append(dep_fnode)
                continue
            if dep_fnode in existing:
                skipped_existing.append(dep_fnode)
                continue
            added.append(dep_fnode)
            existing.add(dep_fnode)

        if added:
            use_cached_precheck = True
            try:
                self.cache.bootstrap_if_needed()
                self.cache.refresh_rows(self.cache.dep_rows(added))
                referrer_fnodes = self.cache.referrer_fnodes(target_fnode=node.fnode)
            except (OSError, ValueError, sqlite3.Error):
                use_cached_precheck = False
                referrer_fnodes = set()

            if use_cached_precheck:
                suspicious_existing = [
                    dep_fnode for dep_fnode in node.depens if dep_fnode in referrer_fnodes
                ]
                suspicious_added = [
                    dep_fnode for dep_fnode in added if dep_fnode in referrer_fnodes
                ]
            else:
                suspicious_existing = list(node.depens)
                suspicious_added = list(added)

            if suspicious_existing or suspicious_added:
                self._loader.expand_from_root(root_fnode=node.fnode, depth=-1)
                for dep_fnode in suspicious_added:
                    dep_node = self._loader.load_node(
                        dep_fnode,
                        tolerate_missing=True,
                        tolerate_invalid=True,
                    )
                    if dep_node is None:
                        self.state.dep_graph.setdefault(dep_fnode, [])
                        continue
                    self.state.nodes_by_fnode[dep_fnode] = dep_node
                    self._loader.expand_from_root(root_fnode=dep_fnode, depth=-1)

                candidate_deps = self._dedupe_keep_order([*node.depens, *added])
                candidate_graph = {
                    fnode: list(dep_list) for fnode, dep_list in self.state.dep_graph.items()
                }
                candidate_graph[node.fnode] = candidate_deps
                for dep_fnode in added:
                    candidate_graph.setdefault(dep_fnode, [])

                cycle = find_cycle(candidate_graph, root_fnode=node.fnode)
                if cycle is not None:
                    raise DependencyCycleError(cycle)

            for dep_fnode in added:
                node.add_dependency(dep_fnode)

        if added:
            node.save()
            self.state.dep_graph[node.fnode] = self._dedupe_keep_order(node.depens)
            for dep_fnode in added:
                self.state.dep_graph.setdefault(dep_fnode, [])

        return added, skipped_existing, skipped_self

    def remove_direct_dependencies(
        self,
        dep_fnodes: list[str],
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[str]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._loader.ensure_node_loaded(active_root)

        removed: list[str] = []
        for dep_fnode in self._dedupe_keep_order(dep_fnodes):
            if dep_fnode not in node.depens:
                continue
            node.rmv_dependency(dep_fnode)
            removed.append(dep_fnode)

        if removed:
            node.save()
            self.state.dep_graph[node.fnode] = self._dedupe_keep_order(node.depens)

        return removed

    def referrer_items(
        self,
        *,
        depth: int = 1,
        target_fnode: str | None = None,
    ) -> list[DependencyItem]:
        if depth < -1:
            raise ValueError("depth must be -1 (infinite) or >= 0")

        active_target = target_fnode or self._bind_root()
        self.cache.bootstrap_if_needed()
        if not self.cache.has_fnode(active_target):
            raise ValueError(f"no mdoc matched reference: {active_target}")
        return self.cache.referrer_items(
            target_fnode=active_target,
            depth=depth,
        )

    def graph_check_report(self) -> GraphCheckReport:
        self.cache.bootstrap_if_needed()
        return self.cache.graph_check_report()

    def global_root_items(self) -> list[GraphRootItem]:
        self.cache.bootstrap_if_needed()
        return self.cache.global_root_items()

    def dependency_items(
        self,
        *,
        depth: int = 1,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[DependencyItem]:
        active_root = self._dependency_context(
            depth=depth,
            root_node=root_node,
            root_fnode=root_fnode,
        )
        return dependency_items_from_graph(
            mdcroot=self.mdcroot,
            state=self.state,
            root_fnode=active_root,
        )

    def leaf_dependency_items(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[DependencyItem]:
        active_root = self._dependency_context(
            depth=-1,
            root_node=root_node,
            root_fnode=root_fnode,
        )
        return leaf_items_from_graph(
            mdcroot=self.mdcroot,
            state=self.state,
            root_fnode=active_root,
        )

    def ordered_nodes(
        self,
        *,
        depth: int = 1,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[MdocNode]:
        active_root = self._dependency_context(
            depth=depth,
            root_node=root_node,
            root_fnode=root_fnode,
        )
        return self._evaluator.ordered_nodes(
            root_fnode=active_root,
        )

    def eval_blocks(
        self,
        *,
        depth: int = 1,
        dep_items: list[DependencyItem] | None = None,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
        progress: Callable[[str], None] | None = None,
        on_start: Callable[[int, int, str], None] | None = None,
        on_result: Callable[[int, int, str, CompilerRes], None] | None = None,
    ) -> list[tuple[str, CompilerRes]]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        root = self._loader.ensure_node_loaded(active_root)
        active_dep_items = dep_items
        if active_dep_items is None:
            active_dep_items = self.dependency_items(
                depth=depth,
                root_node=root_node,
                root_fnode=root_fnode,
            )
        return self._evaluator.eval_blocks(
            root_node=root,
            root_fnode=active_root,
            dep_items=active_dep_items,
            progress=progress,
            on_start=on_start,
            on_result=on_result,
        )

    def scan_all(self) -> None:
        self._loader.scan_all()

    def _dependency_context(
        self,
        *,
        depth: int,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> str:
        if depth < -1:
            raise ValueError("depth must be -1 (infinite) or >= 0")

        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)

        try:
            self._loader.expand_from_root(root_fnode=active_root, depth=depth)
        except (OSError, ValueError, sqlite3.Error) as exc:
            raise ValueError(f"failed to build dependency graph: {exc}") from exc

        cycle = find_cycle(self.state.dep_graph, root_fnode=active_root)
        if cycle is not None:
            raise DependencyCycleError(cycle)

        return active_root

    def _bind_root(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> str:
        if root_node is not None:
            self.set_root_node(root_node)
        if root_fnode is not None:
            self.set_root_fnode(root_fnode)
        if not self.root_fnode:
            raise ValueError("root fnode is required")
        return self.root_fnode

    @staticmethod
    def _dedupe_keep_order(items: list[str]) -> list[str]:
        out: list[str] = []
        seen: set[str] = set()
        for item in items:
            if item in seen:
                continue
            seen.add(item)
            out.append(item)
        return out

    def _has_local_graph_state(self, fnode: str) -> bool:
        return (
            fnode in self.state.nodes_by_fnode
            or fnode in self.state.broken_issues
            or fnode in self.state.dep_graph
        )
