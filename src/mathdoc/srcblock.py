from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .compiler import (
    COMPILER_REGISTRY,
    CompilerReq,
    CompilerRes,
)


@dataclass(slots=True)
class SrcBlock:
    """A typed content block in a knowledge card."""

    srctype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)

    def compile(
        self,
        *,
        mdcroot: Path,
        src_cfg: dict[str, Any],
        progress: Callable[[str], None] | None = None,
    ) -> CompilerRes:
        if mdcroot is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="mdcroot is required for compile",
                rtcode=1,
            )

        srctype = self.srctype.strip().casefold()
        compcfg = src_cfg.get(srctype, {})
        if not isinstance(compcfg, dict):
            return CompilerRes(
                result=False,
                stdout="",
                stderr=f"config key 'src.{srctype}' must be a table",
                rtcode=1,
            )

        if self.metadata is not None:
            # TODO: possible overwrite of compcfg
            pass

        compiler = COMPILER_REGISTRY.resolve(srctype)
        if compiler is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=f"unsupported srctype: {self.srctype}",
                rtcode=127,
            )

        req = CompilerReq(
            mdcroot=mdcroot,
            srctype=srctype,
            content=self.content,
            compcfg=compcfg,
            progress=progress,
        )
        return compiler.compile(req)
