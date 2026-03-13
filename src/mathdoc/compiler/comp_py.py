from __future__ import annotations

import sys
from typing import TYPE_CHECKING

from .base import BlockCompiler
from .base import CompilerRes

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


class CompilerPy(BlockCompiler):
    @property
    def srctype(self) -> str:
        return "py"

    def compile(self, block: SrcBlock) -> CompilerRes:
        timeout_sec = self._read_positive_int(
            block=block,
            key="timeout_sec",
            full_key="src.py.timeout_sec",
        )
        if timeout_sec is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="invalid timeout_sec config",
                rtcode=1,
            )

        proc = self._run_process(
            block,
            [sys.executable, "-c", block.content],
            tool_name="python",
            timeout_sec=timeout_sec,
        )
        if proc is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="timed out",
                rtcode=124,
            )

        return CompilerRes(
            result=proc.returncode == 0,
            stdout=proc.stdout,
            stderr=proc.stderr,
            rtcode=proc.returncode,
        )
