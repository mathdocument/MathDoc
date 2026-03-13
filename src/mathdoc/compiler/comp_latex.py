from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from .base import BlockCompiler
from .base import CompilerRes

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


class CompilerLatex(BlockCompiler):
    @dataclass(slots=True)
    class LatexArtifacts:
        tex_dir: Path
        tex_path: Path
        pdf_path: Path

    @property
    def srctype(self) -> str:
        return "latex"

    def compile(self, block: SrcBlock) -> CompilerRes:
        timeout_sec = self._read_positive_int(
            block=block,
            key="timeout_sec",
            full_key="src.latex.timeout_sec",
        )
        if timeout_sec is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="invalid timeout_sec config",
                rtcode=1,
            )

        preamble = self._read_str(
            block=block,
            key="preamble",
            full_key="src.latex.preamble",
        )
        if preamble is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="invalid preamble config",
                rtcode=1,
            )

        postamble = self._read_str(
            block=block,
            key="postamble",
            full_key="src.latex.postamble",
        )
        if postamble is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="invalid postamble config",
                rtcode=1,
            )

        latexmk = self._require_tool(block, "latexmk")
        if latexmk is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="latexmk not found in PATH",
                rtcode=127,
            )
        xelatex = self._require_tool(block, "xelatex")
        if xelatex is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="xelatex not found in PATH",
                rtcode=127,
            )

        context = block.require_context()
        artifacts = self._prepare_latex_artifacts(
            block=block,
            mdcroot=context.mdcroot,
            fnode=context.fnode,
        )
        if artifacts is None:
            return CompilerRes(
                result=False,
                stdout="",
                stderr="failed to prepare latex artifacts",
                rtcode=1,
            )

        payload = self._latex_payload(
            content=block.content,
            preamble=preamble,
            postamble=postamble,
        )
        try:
            artifacts.tex_path.write_text(payload, encoding="utf-8")
        except OSError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=f"failed to write latex source: {exc}",
                rtcode=1,
            )

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
            return CompilerRes(
                result=False,
                stdout="",
                stderr="latexmk execution failed",
                rtcode=1,
            )
        if tex_proc.returncode != 0:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                rtcode=tex_proc.returncode,
            )

        if not artifacts.pdf_path.is_file():
            block._set_result_failed(
                f"latexmk succeeded but pdf not found: {artifacts.pdf_path}",
                1,
            )
            return CompilerRes(
                result=False,
                stdout="",
                stderr=f"latexmk succeeded but pdf not found: {artifacts.pdf_path}",
                rtcode=1,
            )

        output_lines = [
            f"artifact dir: {artifacts.tex_dir}",
            f"artifact tex: {artifacts.tex_path}",
            f"artifact pdf: {artifacts.pdf_path}",
        ]
        return CompilerRes(
            result=True,
            stdout="\n".join(output_lines),
            stderr="",
            rtcode=0,
        )

    def _prepare_latex_artifacts(
        self,
        *,
        block: SrcBlock,
        mdcroot: Path,
        fnode: str,
    ) -> LatexArtifacts | None:
        tex_dir = mdcroot.resolve() / ".mdc" / ".tex"
        try:
            tex_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            block._set_result_failed(f"failed to create tex artifact dir: {exc}", 1)
            return None

        safe_fnode = "".join(
            ch if ch.isalnum() or ch in ("-", "_", ".") else "_" for ch in fnode.strip()
        )
        stem = f"snippet_{safe_fnode}"
        artifacts = CompilerLatex.LatexArtifacts(
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
