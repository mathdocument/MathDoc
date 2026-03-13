from __future__ import annotations

import sqlite3
from collections import deque
from pathlib import Path
from typing import Any

from .compiler import CompilerRes
from .config import load_config
from .indcache import IndCache
from .mdocnode import DependencyItem
from .mdocnode import MdocNode
from .srcblock import SrcBlock
from .utils import format_mdoc_item
from .utils import to_rel_path


class DepGraph:
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
            raise ValueError(
                f"root fnode mismatch: {self.root_fnode} != {node.fnode}"
            )
        self.root_fnode = node.fnode

    def set_root_fnode(self, fnode: str) -> None:
        value = fnode.strip()
        if not value:
            raise ValueError("root fnode cannot be empty")
        if self.root_fnode and self.root_fnode != value:
            raise ValueError(f"root fnode mismatch: {self.root_fnode} != {value}")
        self.root_fnode = value

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
        return [self.nodes_by_fnode[fnode] for fnode in topo_fnodes]

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

        for file_path in self._iter_mdoc_files():
            node = self._load_node_from_path(file_path)
            self.nodes_by_fnode[node.fnode] = node

        for node in list(self.nodes_by_fnode.values()):
            self.dep_graph[node.fnode] = []
            for dep_fnode in self._dedupe_keep_order(node.depens):
                if dep_fnode not in self.nodes_by_fnode:
                    dep_node = self._load_node(dep_fnode, tolerate_missing=True)
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
                    dep_node = self._load_node(dep_fnode, tolerate_missing=True)
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
        node = self._load_node(fnode, tolerate_missing=False)
        if node is None:
            raise ValueError(f"no mdoc matched reference: {fnode}")
        self.nodes_by_fnode[fnode] = node
        self.dep_graph.setdefault(fnode, [])
        return node

    def _load_node(self, fnode: str, *, tolerate_missing: bool) -> MdocNode | None:
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

            self.missing_fnodes.discard(fnode)
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
        self.nodes_by_fnode.pop(fnode, None)
        self.dep_graph.setdefault(fnode, [])

    def _load_node_from_path(self, path: Path) -> MdocNode:
        node = MdocNode(mdcroot=self.mdcroot, path=path, title="")
        node.load()
        return node

    def _find_cycle(self, *, root_fnode: str | None = None) -> list[str] | None:
        state: dict[str, int] = {}
        stack: list[str] = []
        stack_idx: dict[str, int] = {}

        def dfs(fnode: str) -> list[str] | None:
            state[fnode] = 1
            stack_idx[fnode] = len(stack)
            stack.append(fnode)

            for dep_fnode in self.dep_graph.get(fnode, []):
                dep_state = state.get(dep_fnode, 0)
                if dep_state == 0:
                    cycle = dfs(dep_fnode)
                    if cycle is not None:
                        return cycle
                elif dep_state == 1:
                    start = stack_idx[dep_fnode]
                    return stack[start:] + [dep_fnode]

            stack.pop()
            stack_idx.pop(fnode, None)
            state[fnode] = 2
            return None

        roots = [root_fnode] if root_fnode is not None else list(self.dep_graph)
        for fnode in roots:
            if fnode not in self.dep_graph:
                continue
            if state.get(fnode, 0) != 0:
                continue
            cycle = dfs(fnode)
            if cycle is not None:
                return cycle
        return None

    def _format_cycle(self, cycle: list[str]) -> str:
        if not cycle:
            return "dependency cycle detected"
        lines = ["dependency cycle detected:"]

        cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
        total = len(cycle_nodes)
        for idx, fnode in enumerate(cycle_nodes):
            node = self.nodes_by_fnode.get(fnode)
            if node is None:
                item = format_mdoc_item(fnode, "<missing>", "<unknown>", marker="+")
            else:
                item = format_mdoc_item(
                    node.fnode,
                    node.title,
                    to_rel_path(self.mdcroot, node.path),
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
        visited: set[str] = set()
        order: list[str] = []

        def dfs(fnode: str) -> None:
            if fnode in visited:
                return
            visited.add(fnode)
            for dep_fnode in self.dep_graph.get(fnode, []):
                dfs(dep_fnode)
            order.append(fnode)

        dfs(root_fnode)
        return order

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

            node = self.nodes_by_fnode.get(fnode)
            if node is None:
                items.append(
                    DependencyItem(
                        depth=node_depth,
                        fnode=fnode,
                        title="<missing>",
                        rel_path="<unknown>",
                    )
                )
            else:
                items.append(
                    DependencyItem(
                        depth=node_depth,
                        fnode=node.fnode,
                        title=node.title,
                        rel_path=to_rel_path(self.mdcroot, node.path),
                    )
                )

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
