from __future__ import annotations

import sys
from typing import TYPE_CHECKING

from .base import BlockCompiler

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


class CompilerPy(BlockCompiler):
    @property
    def srctype(self) -> str:
        return "py"

    def compile(self, block: SrcBlock) -> None:
        timeout_sec = self._read_positive_int(
            block=block,
            key="timeout_sec",
            full_key="src.py.timeout_sec",
        )
        if timeout_sec is None:
            return

        proc = self._run_process(
            block,
            [sys.executable, "-c", block.content],
            tool_name="python",
            timeout_sec=timeout_sec,
        )
        if proc is None:
            return

        block._set_result(
            ok=proc.returncode == 0,
            stdout=proc.stdout,
            stderr=proc.stderr,
            returncode=proc.returncode,
        )
