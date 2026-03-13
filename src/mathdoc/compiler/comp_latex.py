from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

from .base import SrcCompiler
from .base import CompilerReq
from .base import CompilerRes


class CompilerLatex(SrcCompiler):
    @dataclass(slots=True)
    class LatexArtifacts:
        tex_dir: Path
        tex_path: Path
        pdf_path: Path

    @property
    def srctype(self) -> str:
        return "latex"

    def compile(self, req: CompilerReq) -> CompilerRes:
        try:
            timeout_sec = self._read_positive_int(
                compcfg=req.compcfg,
                key="timeout_sec",
                full_key="src.latex.timeout_sec",
            )
            preamble = self._read_str(
                compcfg=req.compcfg,
                key="preamble",
                full_key="src.latex.preamble",
            )
            postamble = self._read_str(
                compcfg=req.compcfg,
                key="postamble",
                full_key="src.latex.postamble",
            )
        except (KeyError, ValueError) as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=exc.args[0] if exc.args else str(exc),
                rtcode=1,
            )

        try:
            latexmk = self._require_tool("latexmk")
            xelatex = self._require_tool("xelatex")
            _ = xelatex
        except FileNotFoundError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=str(exc),
                rtcode=127,
            )

        try:
            artifacts = self._prepare_latex_artifacts(
                mdcroot=req.mdcroot,
            )
        except RuntimeError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=str(exc),
                rtcode=1,
            )

        payload = self._latex_payload(
            content=req.content,
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

        try:
            tex_proc = self._run_process(
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
        except TimeoutError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=str(exc),
                rtcode=124,
            )
        except RuntimeError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=str(exc),
                rtcode=127,
            )
        if tex_proc.returncode != 0:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                rtcode=tex_proc.returncode,
            )

        if not artifacts.pdf_path.is_file():
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
        mdcroot: Path,
    ) -> LatexArtifacts:
        tex_dir = mdcroot.resolve() / ".mdc" / ".tex"
        try:
            tex_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            raise RuntimeError(f"failed to create tex artifact dir: {exc}") from exc

        stem = f"temp-latex-{uuid4().hex[:8]}"
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
