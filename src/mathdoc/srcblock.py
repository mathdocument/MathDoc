import shutil
import subprocess
import sys
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass(slots=True)
class CompileResult:
    ok: bool
    stdout: str = ""
    stderr: str = ""
    returncode: int = 0


@dataclass(slots=True)
class CompileContext:
    mdoc_root: Path
    fnode: str
    compiler_cfg: dict[str, Any]


@dataclass(slots=True)
class SrcBlock:
    """A typed content block in a knowledge card."""

    srctype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)
    result: CompileResult | None = field(
        default=None, init=False, repr=False)
    context: CompileContext | None = field(
        default=None, init=False, repr=False)

    def require_result(self) -> CompileResult:
        if self.result is None:
            raise AssertionError("missing compile result")
        return self.result

    def require_context(self) -> CompileContext:
        if self.context is None:
            raise AssertionError("missing compile context")
        return self.context

    def _set_result(self, result: CompileResult) -> None:
        self.result = result

    def _set_fail(self, message: str, returncode: int) -> None:
        self._set_result(CompileResult(
            ok=False,
            stderr=message,
            returncode=returncode,
        ))

    def compile(self, *, mdoc_root: Path, fnode: str, src_cfg: dict[str, Any]) -> None:
        self.result = None
        self.context = None

        if mdoc_root is None:
            self._set_fail("mdoc_root is required for compile", 1)
            return
        if not fnode.strip():
            self._set_fail("fnode is required for compile", 1)
            return

        srctype_key = self.srctype.strip().casefold()
        compiler_cfg = src_cfg.get(srctype_key, {})
        if not isinstance(compiler_cfg, dict):
            self._set_fail(
                f"config key 'src.{srctype_key}' must be a table", 1)
            return

        if self.metadata is not None:
            # TODO: possible overwrite of compiler_cfg
            pass

        self.context = CompileContext(
            mdoc_root=mdoc_root,
            fnode=fnode,
            compiler_cfg=compiler_cfg,
        )

        compiler = DEFAULT_COMPILER_REGISTRY.resolve(srctype_key)
        if compiler is None:
            self._set_fail(f"unsupported srctype: {self.srctype}", 127)
            return
        compiler.compile(self)
        if self.result is None:
            self._set_fail("compiler did not set compile result", 1)


class BlockCompiler(ABC):
    @property
    @abstractmethod
    def srctype(self) -> str:
        raise NotImplementedError

    @abstractmethod
    def compile(self, block: SrcBlock) -> None:
        raise NotImplementedError

    def _read_positive_int(
        self,
        *,
        block: SrcBlock,
        key: str,
        full_key: str,
    ) -> int | None:
        config = block.require_context().compiler_cfg
        if key not in config:
            block._set_fail(f"config key '{full_key}' is required", 1)
            return None
        value = config[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            block._set_fail(
                f"config key '{full_key}' must be a positive integer", 1)
            return None
        return value

    def _read_str(
        self,
        *,
        block: SrcBlock,
        key: str,
        full_key: str,
    ) -> str | None:
        config = block.require_context().compiler_cfg
        if key not in config:
            block._set_fail(f"config key '{full_key}' is required", 1)
            return None
        value = config[key]
        if not isinstance(value, str):
            block._set_fail(f"config key '{full_key}' must be a string", 1)
            return None
        return value

    def _require_tool(self, block: SrcBlock, tool_name: str) -> str | None:
        path = shutil.which(tool_name)
        if path is None:
            block._set_fail(f"{tool_name} not found in PATH", 127)
            return None
        return path

    def _run_process(
        self,
        block: SrcBlock,
        command: list[str],
        *,
        tool_name: str,
        timeout_sec: int,
        cwd: Path | None = None,
    ) -> subprocess.CompletedProcess[str] | None:
        try:
            proc = subprocess.run(
                command,
                check=False,
                text=True,
                capture_output=True,
                timeout=timeout_sec,
                cwd=str(cwd) if cwd is not None else None,
            )
        except subprocess.TimeoutExpired:
            block._set_fail(
                f"{tool_name} timed out after {timeout_sec} seconds", 124)
            return None
        except OSError as exc:
            block._set_fail(f"failed to run {tool_name}: {exc}", 127)
            return None
        return proc


class NatlCompiler(BlockCompiler):
    @property
    def srctype(self) -> str:
        return "natl"

    def compile(self, block: SrcBlock) -> None:
        block._set_result(
            CompileResult(
                ok=True,
                stdout=block.content.rstrip("\n"),
                returncode=0,
            ),
        )


class PyCompiler(BlockCompiler):
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
            CompileResult(
                ok=proc.returncode == 0,
                stdout=proc.stdout,
                stderr=proc.stderr,
                returncode=proc.returncode,
            ),
        )


class LatexCompiler(BlockCompiler):
    @dataclass(slots=True)
    class LatexArtifacts:
        tex_dir: Path
        tex_path: Path
        pdf_path: Path

    @property
    def srctype(self) -> str:
        return "latex"

    def compile(self, block: SrcBlock) -> None:
        timeout_sec = self._read_positive_int(
            block=block,
            key="timeout_sec",
            full_key="src.latex.timeout_sec",
        )
        if timeout_sec is None:
            return

        preamble = self._read_str(
            block=block,
            key="preamble",
            full_key="src.latex.preamble",
        )
        if preamble is None:
            return

        postamble = self._read_str(
            block=block,
            key="postamble",
            full_key="src.latex.postamble",
        )
        if postamble is None:
            return

        latexmk = self._require_tool(block, "latexmk")
        if latexmk is None:
            return
        xelatex = self._require_tool(block, "xelatex")
        if xelatex is None:
            return

        context = block.require_context()
        artifacts = self._prepare_latex_artifacts(
            block=block,
            mdoc_root=context.mdoc_root,
            fnode=context.fnode,
        )
        if artifacts is None:
            return

        payload = self._latex_payload(
            content=block.content,
            preamble=preamble,
            postamble=postamble,
        )
        try:
            artifacts.tex_path.write_text(payload, encoding="utf-8")
        except OSError as exc:
            block._set_fail(f"failed to write latex source: {exc}", 1)
            return

        tex_proc = self._run_process(
            block,
            [
                latexmk,
                "-pdf",
                "-xelatex",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-outdir=.",
                artifacts.tex_path.name,
            ],
            tool_name="latexmk",
            timeout_sec=timeout_sec,
            cwd=artifacts.tex_dir,
        )
        if tex_proc is None:
            return
        if tex_proc.returncode != 0:
            block._set_fail(
                self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                tex_proc.returncode,
            )
            return

        if not artifacts.pdf_path.is_file():
            block._set_fail(
                f"latexmk succeeded but pdf not found: {artifacts.pdf_path}",
                1,
            )
            return

        output_lines = [
            f"artifact dir: {artifacts.tex_dir}",
            f"artifact tex: {artifacts.tex_path}",
            f"artifact pdf: {artifacts.pdf_path}",
        ]
        block._set_result(
            CompileResult(
                ok=True,
                stdout="\n".join(output_lines),
                returncode=0,
            ),
        )

    def _prepare_latex_artifacts(
        self,
        *,
        block: SrcBlock,
        mdoc_root: Path,
        fnode: str,
    ) -> LatexArtifacts | None:
        tex_dir = mdoc_root.resolve() / ".mdc" / ".tex"
        try:
            tex_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            block._set_fail(f"failed to create tex artifact dir: {exc}", 1)
            return None

        safe_fnode = "".join(
            ch if ch.isalnum() or ch in ("-", "_", ".") else "_"
            for ch in fnode.strip()
        )
        stem = f"snippet_{safe_fnode}"
        artifacts = LatexCompiler.LatexArtifacts(
            tex_dir=tex_dir,
            tex_path=tex_dir / f"{stem}.tex",
            pdf_path=tex_dir / f"{stem}.pdf",
        )
        return artifacts

    @staticmethod
    def _latex_payload(
        *,
        content: str,
        preamble: str,
        postamble: str,
    ) -> str:
        if "\\documentclass" in content:
            return content

        preamble_text = preamble.rstrip("\n")
        body = content.rstrip("\n")
        postamble_text = postamble.strip("\n")

        parts = [preamble_text, body]
        if postamble_text:
            parts.append(postamble_text)
        return "\n".join(parts) + "\n"

    @staticmethod
    def _summarize_latex_error(stdout: str, stderr: str) -> str:
        combined = "\n".join([stdout or "", stderr or ""]).strip()
        lines = combined.splitlines()
        error_lines = [line for line in lines if line.startswith("! ")]
        summary_lines = error_lines[-8:] if error_lines else lines[-24:]
        return "\n".join(summary_lines).strip()


class CompilerRegistry:
    def __init__(self, compilers: list[BlockCompiler]) -> None:
        self._compilers = {compiler.srctype.casefold(
        ): compiler for compiler in compilers}

    def resolve(self, srctype: str) -> BlockCompiler | None:
        return self._compilers.get(srctype.casefold())


DEFAULT_COMPILER_REGISTRY = CompilerRegistry(
    [
        NatlCompiler(),
        PyCompiler(),
        LatexCompiler(),
    ]
)
