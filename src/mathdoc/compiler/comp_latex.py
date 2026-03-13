from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from .base import BlockCompiler

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
            block._set_result_failed(f"failed to write latex source: {exc}", 1)
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
            block._set_result_failed(
                self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                tex_proc.returncode,
            )
            return

        if not artifacts.pdf_path.is_file():
            block._set_result_failed(
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
            ok=True,
            stdout="\n".join(output_lines),
            returncode=0,
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
