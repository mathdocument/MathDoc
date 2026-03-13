from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .compiler import COMPILER_REGISTRY


@dataclass(slots=True)
class SrcBlock:
    """A typed content block in a knowledge card."""

    srctype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)

    @dataclass(slots=True)
    class CompileResult:
        ok: bool
        stdout: str = ""
        stderr: str = ""
        returncode: int = 0

    @dataclass(slots=True)
    class CompileContext:
        mdcroot: Path
        fnode: str
        compiler_cfg: dict[str, Any]

    result: CompileResult | None = field(default=None, init=False, repr=False)
    context: CompileContext | None = field(default=None, init=False, repr=False)

    def require_result(self) -> CompileResult:
        if self.result is None:
            raise AssertionError("missing compile result")
        return self.result

    def require_context(self) -> CompileContext:
        if self.context is None:
            raise AssertionError("missing compile context")
        return self.context

    def _set_result(
        self, *, ok: bool, stdout: str = "", stderr: str = "", returncode: int
    ) -> None:
        if self.result is not None:
            raise AssertionError("compile result already set")
        self.result = SrcBlock.CompileResult(
            ok=ok,
            stdout=stdout,
            stderr=stderr,
            returncode=returncode,
        )

    def _set_result_failed(self, message: str, returncode: int) -> None:
        self._set_result(
            ok=False,
            stderr=message,
            returncode=returncode,
        )

    def compile(self, *, mdcroot: Path, fnode: str, src_cfg: dict[str, Any]) -> None:
        self.result = None
        self.context = None

        if mdcroot is None:
            self._set_result_failed("mdcroot is required for compile", 1)
            return
        if not fnode.strip():
            self._set_result_failed("fnode is required for compile", 1)
            return

        srctype_key = self.srctype.strip().casefold()
        compiler_cfg = src_cfg.get(srctype_key, {})
        if not isinstance(compiler_cfg, dict):
            self._set_result_failed(
                f"config key 'src.{srctype_key}' must be a table", 1
            )
            return

        if self.metadata is not None:
            # TODO: possible overwrite of compiler_cfg
            pass

        self.context = SrcBlock.CompileContext(
            mdcroot=mdcroot,
            fnode=fnode,
            compiler_cfg=compiler_cfg,
        )

        compiler = COMPILER_REGISTRY.resolve(srctype_key)
        if compiler is None:
            self._set_result_failed(f"unsupported srctype: {self.srctype}", 127)
            return

        resp = compiler.compile(self)
        self.result = SrcBlock.CompileResult(
            ok=resp.result,
            stdout=resp.stdout,
            stderr=resp.stderr,
            returncode=resp.rtcode,
        )

        if self.result is None:
            self._set_result_failed("compiler did not set compile result", 1)
