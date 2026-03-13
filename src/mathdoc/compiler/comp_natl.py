from __future__ import annotations

from .base import SrcCompiler
from .base import CompilerReq
from .base import CompilerRes


class CompilerNatl(SrcCompiler):
    @property
    def srctype(self) -> str:
        return "natl"

    def compile(self, req: CompilerReq) -> CompilerRes:
        return CompilerRes(
            result=True,
            stdout=req.content.rstrip("\n"),
            stderr="",
            rtcode=0,
        )
