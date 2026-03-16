from collections.abc import Callable
from pathlib import Path
from typing import Any

from ..core import DependencyItem, topo_dependencies_first
from .issues import is_broken_fnode
from .state import GraphState
from ..compiler import CompilerRes
from ..config import load_config
from ..mdocnode import MdocNode
from ..srcblock import SrcBlock


class GraphEvaluator:
    def __init__(self, *, mdcroot: Path, state: GraphState) -> None:
        self.mdcroot = Path(mdcroot).resolve()
        self.state = state

    def ordered_nodes(
        self,
        *,
        root_fnode: str,
    ) -> list[MdocNode]:
        topo_fnodes = topo_dependencies_first(self.state.dep_graph, root_fnode=root_fnode)
        return [
            self.state.nodes_by_fnode[fnode]
            for fnode in topo_fnodes
            if fnode in self.state.nodes_by_fnode
        ]

    def eval_blocks(
        self,
        *,
        root_node: MdocNode,
        root_fnode: str,
        dep_items: list[DependencyItem],
        progress: Callable[[str], None] | None = None,
        on_start: Callable[[int, int, str], None] | None = None,
        on_result: Callable[[int, int, str, CompilerRes], None] | None = None,
    ) -> list[tuple[str, CompilerRes]]:
        if any(is_broken_fnode(self.state, item.fnode) for item in dep_items):
            raise ValueError(
                "broken dependency targets detected; "
                "remove the broken references with `mdc dep rm` before eval"
            )
        if not root_node.blocks:
            return []

        ordered_nodes = self.ordered_nodes(
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
            root_node=root_node,
            ordered_nodes=ordered_nodes,
            src_cfg=src_cfg,
        )
        total = len(merged_blocks)

        results: list[tuple[str, CompilerRes]] = []
        for index, block in enumerate(merged_blocks, start=1):
            if on_start is not None:
                on_start(index, total, block.srctype)
            block_progress = self._block_progress(
                progress=progress,
            )
            result = block.compile(
                mdcroot=self.mdcroot,
                src_cfg=src_cfg,
                progress=block_progress,
            )
            results.append(
                (
                    block.srctype,
                    result,
                )
            )
            if on_result is not None:
                on_result(index, total, block.srctype, result)
        return results

    @staticmethod
    def _block_progress(
        *,
        progress: Callable[[str], None] | None,
    ) -> Callable[[str], None] | None:
        if progress is None:
            return None

        def emit(message: str) -> None:
            progress(message)

        return emit

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

            reverse_depens = self._reverse_depens_for_srctype(
                src_cfg=src_cfg,
                srctype_key=srctype_key,
            )
            merge_nodes = (
                list(reversed(ordered_nodes)) if reverse_depens else ordered_nodes
            )
            ordered_blocks: list[SrcBlock] = []
            for node in merge_nodes:
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
    def _reverse_depens_for_srctype(
        *,
        src_cfg: dict[str, Any],
        srctype_key: str,
    ) -> bool:
        compiler_cfg = src_cfg.get(srctype_key)
        if compiler_cfg is None:
            return False
        if not isinstance(compiler_cfg, dict):
            raise ValueError(f"config key 'src.{srctype_key}' must be a table")
        reverse_depens = compiler_cfg.get("reverse_depens", False)
        if not isinstance(reverse_depens, bool):
            raise ValueError(
                f"config key 'src.{srctype_key}.reverse_depens' must be a boolean"
            )
        return reverse_depens

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
