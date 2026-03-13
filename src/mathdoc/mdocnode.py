import sqlite3
import shlex
from collections import deque
from pathlib import Path
from uuid import uuid4
from dataclasses import dataclass, field
from typing import Any

from .srcblock import SrcBlock
from .indcache import IndCache
from .config import load_config
from .utils import format_mdoc_item, to_rel_path


@dataclass(slots=True)
class MdocNode:
    """
    Core unit for a knowledge card file.
    - `fnode`: globally unique id for this card.
    - `title`: human readable title.
    - `path`: file path of the card.
    - `blocks`: one or more code/text blocks (natural language, LaTeX, C++, etc).
    - `depens`: other MdocNode ids this node depends on.
    """

    mdoc_root: Path
    path: Path
    title: str
    fnode: str = field(default_factory=lambda: str(uuid4()), init=False)
    depens: list[str] = field(default_factory=list, init=False)
    blocks: list[SrcBlock] = field(default_factory=list, init=False)

    @classmethod
    def create(
        cls,
        *,
        mdoc_root: Path,
        folder: str = ".",
        title: str = "Untitled",
    ) -> "MdocNode":
        """Create a new node with an auto-generated unique id."""
        root_path = Path(mdoc_root).resolve()
        folder_path = Path(folder).resolve()
        node = cls(mdoc_root=root_path, path=folder_path, title=title)
        node.path = folder_path / f"{node.fnode}.mdoc"
        return node

    def add_dependency(self, dep_fnode: str) -> None:
        """Register a dependency by MdocNode id."""
        if dep_fnode not in self.depens:
            self.depens.append(dep_fnode)

    def rmv_dependency(self, dep_fnode: str) -> None:
        """Unregister a dependency by MdocNode id."""
        if dep_fnode in self.depens:
            self.depens.remove(dep_fnode)

    def eval_blocks(self, *, depth: int = 1, reverse_depens: bool = False) -> list[SrcBlock]:
        if depth < -1:
            raise ValueError("depth must be -1 (infinite) or >= 0")
        if not self.blocks:
            return []

        try:
            dep_graph, nodes_by_fnode = self._build_dependency_graph(
                depth=depth,
            )
        except (OSError, ValueError, sqlite3.Error) as exc:
            raise ValueError(
                f"failed to build dependency graph: {exc}") from exc
        cycle = self._find_cycle(dep_graph)
        if cycle is not None:
            raise ValueError(
                self._format_cycle(
                    cycle=cycle,
                    nodes_by_fnode=nodes_by_fnode,
                )
            )

        topo_fnodes = self._topo_dependencies_first(
            root_fnode=self.fnode,
            dep_graph=dep_graph,
        )
        if reverse_depens:
            topo_fnodes = list(reversed(topo_fnodes))

        try:
            config = load_config(self.mdoc_root)
        except (OSError, ValueError) as exc:
            raise ValueError(f"failed to load config.toml: {exc}") from exc

        src_cfg = config.get("src", {})
        if not isinstance(src_cfg, dict):
            raise ValueError("config key 'src' must be a table")

        merged_blocks = self._merged_blocks_for_eval(
            topo_fnodes=topo_fnodes,
            nodes_by_fnode=nodes_by_fnode,
            src_cfg=src_cfg,
        )

        for block in merged_blocks:
            block.compile(
                mdoc_root=self.mdoc_root,
                fnode=self.fnode,
                src_cfg=src_cfg,
            )
        return merged_blocks

    def _build_dependency_graph(self, *, depth: int) -> tuple[dict[str, list[str]], dict[str, "MdocNode"]]:
        mdc_dir = self.mdoc_root / ".mdc"
        if not mdc_dir.is_dir():
            raise ValueError(f"invalid mdoc root (missing .mdc): {mdc_dir}")
        cache = IndCache(self.mdoc_root)
        cache.bootstrap_if_needed()

        dep_graph: dict[str, list[str]] = {self.fnode: []}
        nodes_by_fnode: dict[str, MdocNode] = {self.fnode: self}
        seen: set[str] = {self.fnode}
        queue: deque[tuple[MdocNode, int]] = deque([(self, 0)])

        while queue:
            node, node_depth = queue.popleft()
            dep_graph[node.fnode] = []
            for dep_fnode in self._dedupe_keep_order(node.depens):
                dep_node = nodes_by_fnode.get(dep_fnode)
                if dep_node is None:
                    if depth != -1 and node_depth >= depth:
                        continue
                    _, _, dep_path = cache.resolve_ref(
                        dep_fnode, cwd=cache.root)
                    dep_node = MdocNode(mdoc_root=self.mdoc_root,
                                        path=dep_path, title="")
                    dep_node.load()

                    nodes_by_fnode[dep_fnode] = dep_node
                dep_graph[node.fnode].append(dep_fnode)
                if dep_fnode in seen:
                    continue
                seen.add(dep_fnode)
                queue.append((dep_node, node_depth + 1))

        for fnode in nodes_by_fnode:
            dep_graph.setdefault(fnode, [])

        return dep_graph, nodes_by_fnode

    @staticmethod
    def _find_cycle(dep_graph: dict[str, list[str]]) -> list[str] | None:
        state: dict[str, int] = {}
        stack: list[str] = []
        stack_idx: dict[str, int] = {}

        def dfs(fnode: str) -> list[str] | None:
            state[fnode] = 1
            stack_idx[fnode] = len(stack)
            stack.append(fnode)

            for dep_fnode in dep_graph.get(fnode, []):
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

        for fnode in dep_graph:
            if state.get(fnode, 0) != 0:
                continue
            cycle = dfs(fnode)
            if cycle is not None:
                return cycle
        return None

    def _format_cycle(self, *, cycle: list[str], nodes_by_fnode: dict[str, "MdocNode"]) -> str:
        if not cycle:
            return "dependency cycle detected"
        lines = ["dependency cycle detected:"]

        cycle_nodes = cycle[:-1] if len(cycle) > 1 else cycle
        total = len(cycle_nodes)
        for idx, fnode in enumerate(cycle_nodes):
            node = nodes_by_fnode.get(fnode)
            if node is None:
                item = format_mdoc_item(
                    fnode, "<missing>", "<unknown>", marker="+")
            else:
                item = format_mdoc_item(
                    node.fnode,
                    node.title,
                    to_rel_path(self.mdoc_root, node.path),
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

    @staticmethod
    def _topo_dependencies_first(*, root_fnode: str, dep_graph: dict[str, list[str]]) -> list[str]:
        visited: set[str] = set()
        order: list[str] = []

        def dfs(fnode: str) -> None:
            if fnode in visited:
                return
            visited.add(fnode)
            for dep_fnode in dep_graph.get(fnode, []):
                dfs(dep_fnode)
            order.append(fnode)

        dfs(root_fnode)
        return order

    def _merged_blocks_for_eval(
        self, *, topo_fnodes: list[str], nodes_by_fnode: dict[str, "MdocNode"], src_cfg: dict[str, Any]
    ) -> list[SrcBlock]:
        merged: list[SrcBlock] = []

        blocks_by_node: dict[str, dict[str, SrcBlock]] = {}
        for fnode in topo_fnodes:
            node = nodes_by_fnode[fnode]
            by_srctype: dict[str, SrcBlock] = {}
            for block in node.blocks:
                by_srctype[block.srctype.casefold()] = block
            blocks_by_node[fnode] = by_srctype

        for root_block in self.blocks:
            srctype_key = root_block.srctype.casefold()
            depens_enabled = self._depens_enabled_for_srctype(
                src_cfg=src_cfg,
                srctype_key=srctype_key,
            )
            if not depens_enabled:
                merged.append(root_block)
                continue

            ordered_blocks: list[SrcBlock] = []
            for fnode in topo_fnodes:
                candidate = blocks_by_node[fnode].get(srctype_key)
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
            raise ValueError(
                f"config key 'src.{srctype_key}' must be a table")
        depens = compiler_cfg.get("depens", False)
        if not isinstance(depens, bool):
            raise ValueError(
                f"config key 'src.{srctype_key}.depens' must be a boolean")
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

    def load(self) -> None:
        """
        Load card content from file.
        """
        if not self.path.exists():
            raise FileNotFoundError(f"mdoc file not found: {self.path}")

        lines = self.path.read_text(encoding="utf-8").splitlines()
        fnode: str = ""
        title: str = ""
        depens: list[str] = []
        blocks: list[SrcBlock] = []

        status = ""
        for index, raw_line in enumerate(lines, start=1):
            line = raw_line.strip()

            if not line:
                continue
            # fnode and title must exist and be unique
            if line.startswith("@fnode:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@fnode' after {status} block in {self.path}")
                if fnode:
                    raise ValueError(
                        f"line {index}: Duplicate '@fnode' in {self.path}")
                fnode = line.split(":", 1)[1].strip()
                if not fnode:
                    raise ValueError(
                        f"line {index}: '@fnode' must be non-empty in {self.path}")
                continue
            if line.startswith("@title:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@title' after {status} block in {self.path}")
                if title:
                    raise ValueError(
                        f"line {index}: Duplicate '@title' in {self.path}")
                title = line.split(":", 1)[1].strip()
                if not title:
                    raise ValueError(
                        f"line {index}: '@title' must be non-empty in {self.path}")
                continue

            # depens is optional but must be unique and non-empty if exists
            if line.startswith("@dep:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@dep' after {status} block in {self.path}")
                if depens:
                    raise ValueError(
                        f"line {index}: Duplicate '@dep' in {self.path}")
                status = "@dep"
                continue
            # src is optional and can have multiple blocks
            if line.startswith("@src:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@src' after {status} block in {self.path}")
                srctype, metadata = self._parse_src_header(line)
                for block in blocks:
                    if srctype == block.srctype:
                        raise ValueError(
                            f"line {index}: Duplicate '@src' srctype '{srctype}' in {self.path}")
                blocks.append(SrcBlock(srctype=srctype,
                              content="", metadata=metadata))
                status = "@src"
                continue

            # get dep content
            if status == "@dep":
                if line == "@end":
                    if not depens:
                        raise ValueError(
                            f"line {index}: '@dep' block must be non-empty in {self.path}")
                    status = ""
                    continue
                dep = line.split(":", 1)[0].strip()
                if not dep:
                    raise ValueError(
                        f"line {index}: Invalid dependency format in {self.path}: '{line}'")
                if dep in depens:
                    raise ValueError(
                        f"line {index}: Duplicate dependency '{dep}' in {self.path}")
                depens.append(dep)
                continue
            # get src content
            if status == "@src":
                if line == "@end":
                    status = ""
                    continue
                blocks[-1].content += raw_line.rstrip() + "\n"
                continue
            raise ValueError(
                f"line {index}: Unrecognized line in {self.path}: '{line}'")

        if status:
            raise ValueError(
                f"Unclosed block '{status}' in {self.path}")
        if not fnode:
            raise ValueError(
                f"'@fnode' must exist and be non-empty in {self.path}")
        if not title:
            raise ValueError(
                f"'@title' must exist and be non-empty in {self.path}")

        self.fnode = fnode
        self.title = title
        self.depens = depens
        self.blocks = blocks

    def save(self) -> None:
        """
        Save card content to file.
        """
        if not self.fnode:
            self.fnode = str(uuid4())

        self.path.parent.mkdir(parents=True, exist_ok=True)

        output_lines: list[str] = [
            f"@fnode: {self.fnode}",
            f"@title: {self.title}",
            "",
        ]

        if self.depens:
            output_lines.append("@dep:")
            output_lines.extend(self.depens)
            output_lines.append("@end")
            output_lines.append("")

        for block in self.blocks:
            output_lines.append(self._format_src_header(
                block.srctype, block.metadata))
            if block.content:
                output_lines.extend(block.content.splitlines())
            output_lines.append("@end")
            output_lines.append("")

        payload = "\n".join(output_lines).rstrip() + "\n"
        self.path.write_text(payload, encoding="utf-8")

    @staticmethod
    def _parse_src_header(line: str) -> tuple[str, dict[str, str]]:
        """
        Parse a src header line.

        Example:
        - @src: latex preamble="path"
        - @src: lean version=4.2
        """
        payload = line.split(":", 1)[1].strip()
        if not payload:
            raise ValueError("Missing srctype after '@src:'.")

        tokens = shlex.split(payload)
        if not tokens:
            raise ValueError("Invalid '@src' header.")

        srctype = tokens[0]
        metadata: dict[str, str] = {}
        for token in tokens[1:]:
            if "=" not in token:
                raise ValueError(f"Invalid src metadata token: '{token}'")
            key, value = token.split("=", 1)
            key = key.strip()
            if not key:
                raise ValueError(f"Invalid src metadata token: '{token}'")
            metadata[key] = value

        return srctype, metadata

    @staticmethod
    def _format_src_header(srctype: str, metadata: dict[str, str]) -> str:
        """Format a src header line for saving."""

        if not metadata:
            return f"@src: {srctype}"

        def _quote(value: str) -> str:
            escaped = value.replace("\\", "\\\\").replace('"', '\\"')
            return f"\"{escaped}\""

        meta_tokens = [
            f"{key}={_quote(value)}" for key, value in metadata.items()]
        return f"@src: {srctype} " + " ".join(meta_tokens)
