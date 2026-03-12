import hashlib
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(slots=True)
class CodeBlock:
    """A typed content block in a knowledge card."""

    codetype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)

    @dataclass(slots=True)
    class CompileResult:
        ok: bool
        codetype: str
        stdout: str = ""
        stderr: str = ""
        returncode: int = 0

    def compile(self, *, mdoc_root: Path | None = None) -> CompileResult:
        code_kind = self.codetype.strip().casefold()
        if code_kind == "natl":
            return self._compile_natl()
        if code_kind == "py":
            return self._compile_py()
        if code_kind == "latex":
            return self._compile_latex(mdoc_root=mdoc_root)
        return CodeBlock.CompileResult(
            ok=False,
            codetype=self.codetype,
            stderr=f"unsupported codetype: {self.codetype}",
            returncode=127,
        )

    def _compile_natl(self) -> CompileResult:
        return CodeBlock.CompileResult(
            ok=True,
            codetype=self.codetype,
            stdout=self.content.rstrip("\n"),
            returncode=0,
        )

    def _compile_py(self) -> CompileResult:
        try:
            proc = subprocess.run(
                [sys.executable, "-c", self.content],
                check=False,
                text=True,
                capture_output=True,
            )
        except OSError as exc:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=str(exc),
                returncode=127,
            )
        return CodeBlock.CompileResult(
            ok=proc.returncode == 0,
            codetype=self.codetype,
            stdout=proc.stdout,
            stderr=proc.stderr,
            returncode=proc.returncode,
        )

    def _compile_latex(self, *, mdoc_root: Path | None = None) -> CompileResult:
        xelatex = shutil.which("xelatex")
        if xelatex is None:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="xelatex not found in PATH",
                returncode=127,
            )
        pdftoppm = shutil.which("pdftoppm")
        if pdftoppm is None:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="pdftoppm not found in PATH",
                returncode=127,
            )
        imgcat = shutil.which("imgcat")
        if imgcat is None:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="imgcat not found in PATH",
                returncode=127,
            )

        base_dir = mdoc_root.resolve() if mdoc_root is not None else Path.cwd().resolve()
        tex_dir = base_dir / ".mdc" / ".tex"
        try:
            tex_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=f"failed to create tex artifact dir: {exc}",
                returncode=1,
            )

        payload = self.content
        if "\\documentclass" not in payload:
            payload = (
                "\\documentclass[varwidth=true, border=5pt, crop]{standalone}\n"
                "\\begin{document}\n"
                f"{self.content.rstrip()}\n"
                "\\end{document}\n"
            )
        digest = hashlib.sha1(payload.encode("utf-8")).hexdigest()[:12]
        stem = f"snippet_{digest}"
        tex_path = tex_dir / f"{stem}.tex"
        pdf_path = tex_dir / f"{stem}.pdf"
        png_prefix = tex_dir / stem
        png_path = tex_dir / f"{stem}.png"
        try:
            tex_path.write_text(payload, encoding="utf-8")
        except OSError as exc:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=f"failed to write latex source: {exc}",
                returncode=1,
            )

        try:
            proc = subprocess.run(
                [
                    xelatex,
                    "-interaction=nonstopmode",
                    "-halt-on-error",
                    "-output-directory",
                    str(tex_dir),
                    str(tex_path),
                ],
                check=False,
                text=True,
                capture_output=True,
                cwd=str(tex_dir),
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="xelatex timed out after 30 seconds",
                returncode=124,
            )
        except OSError as exc:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=f"failed to run xelatex: {exc}",
                returncode=127,
            )

        if proc.returncode != 0:
            combined = "\n".join(
                [proc.stdout or "", proc.stderr or ""]).strip()
            lines = combined.splitlines()
            error_lines = [line for line in lines if line.startswith("! ")]
            summary_lines = error_lines[-8:] if error_lines else lines[-24:]
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="\n".join(summary_lines).strip(),
                returncode=proc.returncode,
            )

        try:
            png_proc = subprocess.run(
                [
                    pdftoppm,
                    "-png",
                    "-r",
                    "1200",
                    "-singlefile",
                    str(pdf_path),
                    str(png_prefix),
                ],
                check=False,
                text=True,
                capture_output=True,
                timeout=15,
            )
        except subprocess.TimeoutExpired:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr="pdftoppm timed out after 15 seconds",
                returncode=124,
            )
        except OSError as exc:
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=f"failed to run pdftoppm: {exc}",
                returncode=127,
            )
        if png_proc.returncode != 0 or not png_path.is_file():
            detail = (png_proc.stderr or png_proc.stdout or "").strip()
            if not detail:
                detail = "failed to convert pdf to png"
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=detail,
                returncode=png_proc.returncode or 1,
            )

        imgcat_width = os.environ.get("MDC_IMGCAT_WIDTH", "60%").strip()
        imgcat_attempts: list[list[str]] = []
        if imgcat_width:
            imgcat_attempts.append(
                [imgcat, "--width", imgcat_width, str(png_path)])
        imgcat_attempts.append([imgcat, str(png_path)])

        imgcat_proc: subprocess.CompletedProcess[str] | None = None
        last_detail = ""
        for command in imgcat_attempts:
            try:
                attempt = subprocess.run(
                    command,
                    check=False,
                    text=True,
                    capture_output=True,
                    timeout=10,
                )
            except subprocess.TimeoutExpired:
                return CodeBlock.CompileResult(
                    ok=False,
                    codetype=self.codetype,
                    stderr="imgcat timed out after 10 seconds",
                    returncode=124,
                )
            except OSError as exc:
                return CodeBlock.CompileResult(
                    ok=False,
                    codetype=self.codetype,
                    stderr=f"failed to run imgcat: {exc}",
                    returncode=127,
                )

            if attempt.returncode == 0:
                imgcat_proc = attempt
                break
            last_detail = (attempt.stderr or attempt.stdout or "").strip()

        if imgcat_proc is None:
            if not last_detail:
                last_detail = "imgcat failed to render png"
            return CodeBlock.CompileResult(
                ok=False,
                codetype=self.codetype,
                stderr=last_detail,
                returncode=1,
            )

        output_lines: list[str] = [
            f"artifact tex: {tex_path}",
            f"artifact pdf: {pdf_path}",
        ]
        preview = imgcat_proc.stdout.rstrip("\n")
        if preview:
            output_lines.append(preview)

        return CodeBlock.CompileResult(
            ok=True,
            codetype=self.codetype,
            stdout="\n".join(output_lines),
            returncode=0,
        )
