from __future__ import annotations

from typing import TYPE_CHECKING

from .base import BlockCompiler
from .base import CompilerRes

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


class CompilerNatl(BlockCompiler):
    @property
    def srctype(self) -> str:
        return "natl"

    def compile(self, block: SrcBlock) -> CompilerRes:
        return CompilerRes(
            result=True,
            stdout=block.content.rstrip("\n"),
            stderr="",
            rtcode=0,
        )
