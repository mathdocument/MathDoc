from __future__ import annotations

from typing import TYPE_CHECKING

from .base import BlockCompiler

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


class CompilerNatl(BlockCompiler):
    @property
    def srctype(self) -> str:
        return "natl"

    def compile(self, block: SrcBlock) -> None:
        block._set_result(
            ok=True,
            stdout=block.content.rstrip("\n"),
            returncode=0,
        )
