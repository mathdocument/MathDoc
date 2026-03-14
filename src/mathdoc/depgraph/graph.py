import sqlite3
from collections import deque
from pathlib import Path
from typing import Any

from .algorithms import component_has_cycle
from .algorithms import find_cycle
from .algorithms import representative_cycle
from .algorithms import strongly_connected_components
from .algorithms import topo_dependencies_first
from .models import DependencyItem
from .models import GraphCheckReport
from .models import GraphIssue
from ..compiler import CompilerRes
from ..config import load_config
from ..indcache import IndCache
from ..mdocnode import MdocNode
from ..srcblock import SrcBlock
from ..utils import format_mdoc_item
from ..utils import to_rel_path


class DepGraph:
    @classmethod
    def create_root(
        cls,
        *,
        mdcroot: Path,
        folder: str = ".",
        title: str = "Untitled",
        cache: IndCache | None = None,
    ) -> tuple["DepGraph", str]:
        root_path = Path(mdcroot).resolve()
        node = MdocNode.create(
            mdcroot=root_path,
            folder=folder,
            title=title,
        )
        node.save()
        graph = cls(mdcroot=root_path, root_node=node, cache=cache)
        return graph, to_rel_path(root_path, node.path)

    @classmethod
    def from_ref(
        cls,
        *,
        cache: IndCache,
        ref: str,
        cwd: Path | None = None,
    ) -> tuple["DepGraph", str]:
        base_cwd = (cwd or Path.cwd()).resolve()

        for attempt in range(2):
            try:
                _, _, src_path = cache.resolve_ref(ref, cwd=base_cwd)
            except ValueError:
                if attempt == 0:
                    cache.refresh_all()
                    continue
                raise

            node = MdocNode(mdcroot=cache.root, path=src_path, title="")
            try:
                node.load()
            except FileNotFoundError:
                if attempt == 0:
                    cache.refresh_all()
                    continue
                raise

            return (
                cls(mdcroot=cache.root, root_node=node, cache=cache),
                to_rel_path(cache.root, src_path),
            )

        raise ValueError(f"failed to resolve mdoc reference: {ref}")

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
        self.root_fnode = ""
        self.dep_graph: dict[str, list[str]] = {}
        self.nodes_by_fnode: dict[str, MdocNode] = {}
        self.missing_fnodes: set[str] = set()
        self.invalid_fnodes: set[str] = set()
        self.broken_issues: dict[str, GraphIssue] = {}
        self.invalid_file_issues: list[GraphIssue] = []
        self.scanned_file_count = 0

        if root_node is not None:
            self.set_root_node(root_node)
        if root_fnode is not None:
            self.set_root_fnode(root_fnode)

    def set_root_node(self, node: MdocNode) -> None:
        if node.mdcroot.resolve() != self.mdcroot:
            raise ValueError(
                f"mdoc node root mismatch: {node.mdcroot.resolve()} != {self.mdcroot}"
            )
        self.nodes_by_fnode[node.fnode] = node
        self.dep_graph.setdefault(node.fnode, [])
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
        active_root = self._bind_root()
        return self._ensure_node_loaded(active_root)

    def root_path(self) -> Path:
        return self.get_root_node().path

    def root_has_blocks(self) -> bool:
        return bool(self.get_root_node().blocks)

    def root_item(self) -> DependencyItem:
        issue = self.issue_for_fnode(self.root_fnode)
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
        return fnode in self.missing_fnodes or fnode in self.invalid_fnodes

    def issue_for_fnode(self, fnode: str) -> GraphIssue | None:
        return self.broken_issues.get(fnode)

    def format_cycle(self, cycle: list[str]) -> str:
        return self._format_cycle(cycle)

    def direct_dependency_fnodes(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[str]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._ensure_node_loaded(active_root)
        return self._dedupe_keep_order(node.depens)

    def direct_dependency_items(
        self,
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[DependencyItem]:
        return self.dependency_items(
            depth=1,
            root_node=root_node,
            root_fnode=root_fnode,
        )

    def add_direct_dependencies(
        self,
        dep_fnodes: list[str],
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> tuple[list[str], list[str], list[str]]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._ensure_node_loaded(active_root)

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
            node.add_dependency(dep_fnode)
            existing.add(dep_fnode)
            added.append(dep_fnode)

        if added:
            node.save()
            self.dep_graph[node.fnode] = self._dedupe_keep_order(node.depens)
            for dep_fnode in added:
                self.dep_graph.setdefault(dep_fnode, [])

        return added, skipped_existing, skipped_self

    def remove_direct_dependencies(
        self,
        dep_fnodes: list[str],
        *,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[str]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        node = self._ensure_node_loaded(active_root)

        removed: list[str] = []
        for dep_fnode in self._dedupe_keep_order(dep_fnodes):
            if dep_fnode not in node.depens:
                continue
            node.rmv_dependency(dep_fnode)
            removed.append(dep_fnode)

        if removed:
            node.save()
            self.dep_graph[node.fnode] = self._dedupe_keep_order(node.depens)

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
        self.scan_all()
        if active_target not in self.dep_graph and not self.is_broken_fnode(
            active_target
        ):
            raise ValueError(f"no mdoc matched reference: {active_target}")

        reverse_graph: dict[str, list[str]] = {}
        for src_fnode, dep_fnodes in self.dep_graph.items():
            for dep_fnode in dep_fnodes:
                reverse_graph.setdefault(dep_fnode, []).append(src_fnode)

        items: list[DependencyItem] = []
        seen: set[str] = {active_target}
        queue: deque[tuple[str, int]] = deque(
            (ref_fnode, 1) for ref_fnode in reverse_graph.get(active_target, [])
        )

        while queue:
            fnode, item_depth = queue.popleft()
            if fnode in seen:
                continue
            seen.add(fnode)
            items.append(self._dependency_item_for_fnode(fnode=fnode, depth=item_depth))

            if depth != -1 and item_depth >= depth:
                continue
            for ref_fnode in reverse_graph.get(fnode, []):
                if ref_fnode == active_target:
                    continue
                queue.append((ref_fnode, item_depth + 1))

        return items

    def graph_check_report(self) -> GraphCheckReport:
        self.scan_all()
        edges = sum(len(dep_fnodes) for dep_fnodes in self.dep_graph.values())
        missing = self._sorted_issues(
            [issue for issue in self.broken_issues.values() if issue.kind == "missing"]
        )
        invalid = self._sorted_issues(
            self._dedupe_issues(
                [
                    *[
                        issue
                        for issue in self.broken_issues.values()
                        if issue.kind == "invalid"
                    ],
                    *self.invalid_file_issues,
                ]
            )
        )
        cycles = self._cycles_by_component()
        return GraphCheckReport(
            nodes=self.scanned_file_count,
            edges=edges,
            missing=missing,
            invalid=invalid,
            cycles=cycles,
        )

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
        return self._dependency_items_from_graph(root_fnode=active_root)

    def ordered_nodes(
        self,
        *,
        depth: int = 1,
        reverse_depens: bool = False,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[MdocNode]:
        active_root = self._dependency_context(
            depth=depth,
            root_node=root_node,
            root_fnode=root_fnode,
        )
        topo_fnodes = self._topo_dependencies_first(root_fnode=active_root)
        if reverse_depens:
            topo_fnodes = list(reversed(topo_fnodes))
        return [
            self.nodes_by_fnode[fnode]
            for fnode in topo_fnodes
            if fnode in self.nodes_by_fnode
        ]

    def eval_blocks(
        self,
        *,
        depth: int = 1,
        reverse_depens: bool = False,
        root_node: MdocNode | None = None,
        root_fnode: str | None = None,
    ) -> list[tuple[str, CompilerRes]]:
        active_root = self._bind_root(root_node=root_node, root_fnode=root_fnode)
        root = self._ensure_node_loaded(active_root)
        dep_items = self.dependency_items(
            depth=depth,
            root_node=root_node,
            root_fnode=root_fnode,
        )
        if any(self.is_broken_fnode(item.fnode) for item in dep_items):
            raise ValueError(
                "broken dependency targets detected; "
                "remove the broken references with `mdc dep rm` before eval"
            )
        if not root.blocks:
            return []

        ordered_nodes = self.ordered_nodes(
            depth=depth,
            reverse_depens=reverse_depens,
            root_node=root_node,
            root_fnode=root_fnode,
        )

        try:
            config = load_config(self.mdcroot)
        except (OSError, ValueError) as exc:
            raise ValueError(f"failed to load config.toml: {exc}") from exc

        src_cfg = config.get("src", {})
        if not isinstance(src_cfg, dict):
            raise ValueError("config key 'src' must be a table")

        merged_blocks = self._merged_blocks_for_eval(
            root_node=root,
            ordered_nodes=ordered_nodes,
            src_cfg=src_cfg,
        )

        results: list[tuple[str, CompilerRes]] = []
        for block in merged_blocks:
            results.append(
                (
                    block.srctype,
                    block.compile(
                        mdcroot=self.mdcroot,
                        src_cfg=src_cfg,
                    ),
                )
            )
        return results

    def scan_all(self) -> None:
        self._ensure_ready()
        self.dep_graph.clear()
        self.nodes_by_fnode.clear()
        self.missing_fnodes.clear()
        self.invalid_fnodes.clear()
        self.broken_issues.clear()
        self.invalid_file_issues.clear()
        self.scanned_file_count = 0

        for file_path in self._iter_mdoc_files():
            self.scanned_file_count += 1
            try:
                node = self._load_node_from_path(file_path)
            except ValueError as exc:
                self._record_invalid_issue(
                    self._invalid_issue_from_path(
                        file_path,
                        error=str(exc),
                    )
                )
                continue
            self.nodes_by_fnode[node.fnode] = node

        for node in list(self.nodes_by_fnode.values()):
            self.dep_graph[node.fnode] = []
            for dep_fnode in self._dedupe_keep_order(node.depens):
                if (
                    dep_fnode not in self.nodes_by_fnode
                    and dep_fnode not in self.invalid_fnodes
                ):
                    dep_node = self._load_node(
                        dep_fnode,
                        tolerate_missing=True,
                        tolerate_invalid=True,
                    )
                    if dep_node is not None:
                        self.nodes_by_fnode[dep_fnode] = dep_node
                self.dep_graph[node.fnode].append(dep_fnode)
                self.dep_graph.setdefault(dep_fnode, [])

        for fnode in self.nodes_by_fnode:
            self.dep_graph.setdefault(fnode, [])

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
            self._expand_from_root(root_fnode=active_root, depth=depth)
        except (OSError, ValueError, sqlite3.Error) as exc:
            raise ValueError(f"failed to build dependency graph: {exc}") from exc

        cycle = self._find_cycle(root_fnode=active_root)
        if cycle is not None:
            raise ValueError(self._format_cycle(cycle))

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

    def _expand_from_root(self, *, root_fnode: str, depth: int) -> None:
        self._ensure_ready()
        root = self._ensure_node_loaded(root_fnode)

        seen: set[str] = {root.fnode}
        queue: deque[tuple[MdocNode, int]] = deque([(root, 0)])

        while queue:
            node, node_depth = queue.popleft()
            self.dep_graph[node.fnode] = []

            for dep_fnode in self._dedupe_keep_order(node.depens):
                dep_node = self.nodes_by_fnode.get(dep_fnode)
                if dep_node is None:
                    if depth != -1 and node_depth >= depth:
                        continue
                    dep_node = self._load_node(
                        dep_fnode,
                        tolerate_missing=True,
                        tolerate_invalid=True,
                    )
                    if dep_node is not None:
                        self.nodes_by_fnode[dep_fnode] = dep_node

                self.dep_graph[node.fnode].append(dep_fnode)
                self.dep_graph.setdefault(dep_fnode, [])
                if dep_node is None:
                    continue

                if dep_fnode in seen:
                    continue
                seen.add(dep_fnode)
                queue.append((dep_node, node_depth + 1))

        for fnode in self.nodes_by_fnode:
            self.dep_graph.setdefault(fnode, [])

    def _ensure_ready(self) -> None:
        mdc_dir = self.mdcroot / ".mdc"
        if not mdc_dir.is_dir():
            raise ValueError(f"invalid mdoc root (missing .mdc): {mdc_dir}")
        self.cache.bootstrap_if_needed()

    def _ensure_node_loaded(self, fnode: str) -> MdocNode:
        node = self.nodes_by_fnode.get(fnode)
        if node is not None:
            return node
        self._ensure_ready()
        node = self._load_node(
            fnode,
            tolerate_missing=False,
            tolerate_invalid=False,
        )
        if node is None:
            raise ValueError(f"no mdoc matched reference: {fnode}")
        self.nodes_by_fnode[fnode] = node
        self.dep_graph.setdefault(fnode, [])
        return node

    def _load_node(
        self,
        fnode: str,
        *,
        tolerate_missing: bool,
        tolerate_invalid: bool,
    ) -> MdocNode | None:
        for attempt in range(2):
            path = self._resolve_fnode_path(
                fnode,
                tolerate_missing=tolerate_missing,
            )
            if path is None:
                if attempt == 0:
                    self.cache.refresh_all()
                    continue
                self._mark_missing(fnode)
                return None

            try:
                node = self._load_node_from_path(path)
            except FileNotFoundError:
                if attempt == 0:
                    self.cache.refresh_all()
                    continue
                if tolerate_missing:
                    self._mark_missing(fnode)
                    return None
                raise
            except ValueError as exc:
                if tolerate_invalid:
                    self._mark_invalid(
                        fnode,
                        path=path,
                        error=str(exc),
                    )
                    return None
                raise

            self._clear_broken_issue(fnode)
            return node

        if tolerate_missing:
            self._mark_missing(fnode)
            return None
        raise ValueError(f"no mdoc matched reference: {fnode}")

    def _resolve_fnode_path(
        self,
        fnode: str,
        *,
        tolerate_missing: bool,
    ) -> Path | None:
        try:
            _, _, path = self.cache.resolve_ref(fnode, cwd=self.cache.root)
            return path
        except ValueError as exc:
            if tolerate_missing and str(exc).startswith("no mdoc matched reference:"):
                return None
            raise

    def _mark_missing(self, fnode: str) -> None:
        self.missing_fnodes.add(fnode)
        self.invalid_fnodes.discard(fnode)
        self.nodes_by_fnode.pop(fnode, None)
        self.dep_graph.setdefault(fnode, [])
        self.broken_issues[fnode] = GraphIssue(
            kind="missing",
            fnode=fnode,
            title="<missing>",
            rel_path="<unknown>",
            error=f"no mdoc matched reference: {fnode}",
        )

    def _mark_invalid(
        self,
        fnode: str,
        *,
        path: Path,
        error: str,
    ) -> None:
        issue = self._invalid_issue_from_path(path, error=error, fnode=fnode)
        self._record_invalid_issue(issue)

    def _record_invalid_issue(self, issue: GraphIssue) -> None:
        self._upsert_issue(self.invalid_file_issues, issue)
        if issue.fnode.startswith("<") and issue.fnode.endswith(">"):
            return
        self.invalid_fnodes.add(issue.fnode)
        self.missing_fnodes.discard(issue.fnode)
        self.nodes_by_fnode.pop(issue.fnode, None)
        self.dep_graph.setdefault(issue.fnode, [])
        self.broken_issues[issue.fnode] = issue

    def _clear_broken_issue(self, fnode: str) -> None:
        self.missing_fnodes.discard(fnode)
        self.invalid_fnodes.discard(fnode)
        self.broken_issues.pop(fnode, None)

    def _load_node_from_path(self, path: Path) -> MdocNode:
        node = MdocNode(mdcroot=self.mdcroot, path=path, title="")
        node.load()
        return node

    def _find_cycle(self, *, root_fnode: str | None = None) -> list[str] | None:
        return find_cycle(self.dep_graph, root_fnode=root_fnode)

    def _format_cycle(self, cycle: list[str]) -> str:
        if not cycle:
            return "dependency cycle detected"
        lines = ["dependency cycle detected:"]

        cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
        total = len(cycle_nodes)
        for idx, fnode in enumerate(cycle_nodes):
            dep_item = self._dependency_item_for_fnode(fnode=fnode, depth=0)
            item = format_mdoc_item(
                dep_item.fnode,
                dep_item.title,
                dep_item.rel_path,
                marker="+",
            )
            if total == 1:
                prefix = "┌─➤"
            elif idx == 0:
                prefix = "┌─➤"
            elif idx == total - 1:
                prefix = "└──"
            else:
                prefix = "│  "
            lines.append(f"{prefix} {item}")
        return "\n".join(lines)

    def _topo_dependencies_first(self, *, root_fnode: str) -> list[str]:
        return topo_dependencies_first(self.dep_graph, root_fnode=root_fnode)

    def _dependency_items_from_graph(self, *, root_fnode: str) -> list[DependencyItem]:
        items: list[DependencyItem] = []
        seen: set[str] = set()
        queue: deque[tuple[str, int]] = deque(
            (dep_fnode, 1) for dep_fnode in self.dep_graph.get(root_fnode, [])
        )

        while queue:
            fnode, node_depth = queue.popleft()
            if fnode in seen:
                continue
            seen.add(fnode)

            items.append(self._dependency_item_for_fnode(fnode=fnode, depth=node_depth))

            for dep_fnode in self.dep_graph.get(fnode, []):
                queue.append((dep_fnode, node_depth + 1))

        return items

    def _merged_blocks_for_eval(
        self,
        *,
        root_node: MdocNode,
        ordered_nodes: list[MdocNode],
        src_cfg: dict[str, Any],
    ) -> list[SrcBlock]:
        merged: list[SrcBlock] = []

        blocks_by_node: dict[str, dict[str, SrcBlock]] = {}
        for node in ordered_nodes:
            by_srctype: dict[str, SrcBlock] = {}
            for block in node.blocks:
                by_srctype[block.srctype.casefold()] = block
            blocks_by_node[node.fnode] = by_srctype

        for root_block in root_node.blocks:
            srctype_key = root_block.srctype.casefold()
            depens_enabled = self._depens_enabled_for_srctype(
                src_cfg=src_cfg,
                srctype_key=srctype_key,
            )
            if not depens_enabled:
                merged.append(root_block)
                continue

            ordered_blocks: list[SrcBlock] = []
            for node in ordered_nodes:
                candidate = blocks_by_node[node.fnode].get(srctype_key)
                if candidate is not None:
                    ordered_blocks.append(candidate)

            merged.append(
                SrcBlock(
                    srctype=root_block.srctype,
                    content=self._merge_block_content(ordered_blocks),
                    metadata=dict(root_block.metadata),
                )
            )

        return merged

    @staticmethod
    def _depens_enabled_for_srctype(
        *,
        src_cfg: dict[str, Any],
        srctype_key: str,
    ) -> bool:
        compiler_cfg = src_cfg.get(srctype_key)
        if compiler_cfg is None:
            return False
        if not isinstance(compiler_cfg, dict):
            raise ValueError(f"config key 'src.{srctype_key}' must be a table")
        depens = compiler_cfg.get("depens", False)
        if not isinstance(depens, bool):
            raise ValueError(f"config key 'src.{srctype_key}.depens' must be a boolean")
        return depens

    @staticmethod
    def _merge_block_content(blocks: list[SrcBlock]) -> str:
        parts: list[str] = []
        for block in blocks:
            text = block.content.rstrip("\n")
            if text:
                parts.append(text)
        if not parts:
            return ""
        return "\n\n".join(parts) + "\n"

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

    def _iter_mdoc_files(self) -> list[Path]:
        files: list[Path] = []
        for file_path in self.mdcroot.rglob("*.mdoc"):
            if ".mdc" in file_path.parts:
                continue
            if file_path.is_file():
                files.append(file_path)
        return files

    def _dependency_item_for_fnode(self, *, fnode: str, depth: int) -> DependencyItem:
        node = self.nodes_by_fnode.get(fnode)
        if node is not None:
            return DependencyItem(
                depth=depth,
                fnode=node.fnode,
                title=node.title,
                rel_path=to_rel_path(self.mdcroot, node.path),
            )

        issue = self.broken_issues.get(fnode)
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

    def _invalid_issue_from_path(
        self,
        path: Path,
        *,
        error: str,
        fnode: str | None = None,
    ) -> GraphIssue:
        head_fnode, _ = self._read_mdoc_head(path)
        issue_fnode = fnode or head_fnode or "<unknown>"
        return GraphIssue(
            kind="invalid",
            fnode=issue_fnode,
            title="<invalid>",
            rel_path=to_rel_path(self.mdcroot, path),
            error=error,
        )

    @staticmethod
    def _read_mdoc_head(path: Path) -> tuple[str | None, str | None]:
        fnode = ""
        title = ""
        try:
            with path.open("r", encoding="utf-8") as handle:
                for raw_line in handle:
                    line = raw_line.strip()
                    lower = line.lower()
                    if lower.startswith("@fnode:"):
                        fnode = line.split(":", 1)[1].strip()
                    elif lower.startswith("@title:"):
                        title = line.split(":", 1)[1].strip()
                    if fnode and title:
                        break
        except OSError:
            return None, None
        return (fnode or None, title or None)

    @staticmethod
    def _upsert_issue(issues: list[GraphIssue], issue: GraphIssue) -> None:
        key = (issue.kind, issue.fnode, issue.rel_path)
        for index, existing in enumerate(issues):
            existing_key = (existing.kind, existing.fnode, existing.rel_path)
            if existing_key == key:
                issues[index] = issue
                return
        issues.append(issue)

    @staticmethod
    def _dedupe_issues(issues: list[GraphIssue]) -> list[GraphIssue]:
        deduped: list[GraphIssue] = []
        for issue in issues:
            DepGraph._upsert_issue(deduped, issue)
        return deduped

    @staticmethod
    def _sorted_issues(issues: list[GraphIssue]) -> list[GraphIssue]:
        return sorted(
            issues,
            key=lambda issue: (issue.rel_path, issue.fnode, issue.error),
        )

    def _cycles_by_component(self) -> list[list[str]]:
        components = strongly_connected_components(self.dep_graph)
        cycles: list[list[str]] = []
        for component in components:
            if not component_has_cycle(self.dep_graph, component):
                continue
            cycle = representative_cycle(self.dep_graph, component)
            if cycle is not None:
                cycles.append(cycle)
        cycles.sort(key=lambda cycle: tuple(cycle))
        return cycles
