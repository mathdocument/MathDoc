import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

from .config import load_latex_config


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

    @dataclass(slots=True)
    class LatexArtifacts:
        tex_dir: Path
        tex_path: Path
        pdf_path: Path
        png_prefix: Path
        png_path: Path

    def compile(
        self,
        *,
        mdoc_root: Path,
        fnode: str,
    ) -> CompileResult:
        if mdoc_root is None:
            return self._fail("mdoc_root is required for compile", 1)
        if not fnode.strip():
            return self._fail("fnode is required for compile", 1)

        code_kind = self.codetype.strip().casefold()
        if code_kind == "natl":
            return self._compile_natl()
        if code_kind == "py":
            return self._compile_py()
        if code_kind == "latex":
            return self._compile_latex(mdoc_root=mdoc_root, fnode=fnode)
        return self._fail(f"unsupported codetype: {self.codetype}", 127)

    def _fail(self, message: str, returncode: int) -> CompileResult:
        return CodeBlock.CompileResult(
            ok=False,
            codetype=self.codetype,
            stderr=message,
            returncode=returncode,
        )

    def _compile_natl(self) -> CompileResult:
        return CodeBlock.CompileResult(
            ok=True,
            codetype=self.codetype,
            stdout=self.content.rstrip("\n"),
            returncode=0,
        )

    def _compile_py(self) -> CompileResult:
        proc, failed = self._run_process(
            [sys.executable, "-c", self.content],
            tool_name="python",
            timeout_sec=30,
        )
        if failed is not None:
            return failed
        assert proc is not None
        return CodeBlock.CompileResult(
            ok=proc.returncode == 0,
            codetype=self.codetype,
            stdout=proc.stdout,
            stderr=proc.stderr,
            returncode=proc.returncode,
        )

    def _compile_latex(
        self,
        *,
        mdoc_root: Path,
        fnode: str,
    ) -> CompileResult:
        cfg = {
            "latex_timeout_sec": 30,
            "pdftoppm_timeout_sec": 15,
            "imgcat_timeout_sec": 10,
            "pdftoppm_dpi": "1200",
            "imgcat_width": "60%",
        }

        xelatex = self._require_tool("xelatex")
        if isinstance(xelatex, CodeBlock.CompileResult):
            return xelatex
        pdftoppm = self._require_tool("pdftoppm")
        if isinstance(pdftoppm, CodeBlock.CompileResult):
            return pdftoppm
        imgcat = self._require_tool("imgcat")
        if isinstance(imgcat, CodeBlock.CompileResult):
            return imgcat

        artifacts, failed = self._prepare_latex_artifacts(mdoc_root, fnode)
        if failed is not None:
            return failed
        assert artifacts is not None

        try:
            latex_cfg = load_latex_config(mdoc_root)
        except (OSError, ValueError) as exc:
            return self._fail(f"failed to load config.toml: {exc}", 1)

        payload = self._latex_payload(
            preamble=latex_cfg.preamble,
            postamble=latex_cfg.postamble,
        )
        try:
            artifacts.tex_path.write_text(payload, encoding="utf-8")
        except OSError as exc:
            return self._fail(f"failed to write latex source: {exc}", 1)

        tex_proc, failed = self._run_process(
            [
                xelatex,
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-output-directory",
                str(artifacts.tex_dir),
                str(artifacts.tex_path),
            ],
            tool_name="xelatex",
            timeout_sec=cfg["latex_timeout_sec"],
            cwd=artifacts.tex_dir,
        )
        if failed is not None:
            return failed
        assert tex_proc is not None
        if tex_proc.returncode != 0:
            return self._fail(
                self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                tex_proc.returncode,
            )

        png_proc, failed = self._run_process(
            [
                pdftoppm,
                "-png",
                "-r",
                cfg["pdftoppm_dpi"],
                "-singlefile",
                str(artifacts.pdf_path),
                str(artifacts.png_prefix),
            ],
            tool_name="pdftoppm",
            timeout_sec=cfg["pdftoppm_timeout_sec"],
        )
        if failed is not None:
            return failed
        assert png_proc is not None
        if png_proc.returncode != 0 or not artifacts.png_path.is_file():
            detail = (png_proc.stderr or png_proc.stdout or "").strip()
            if not detail:
                detail = "failed to convert pdf to png"
            return self._fail(detail, png_proc.returncode or 1)

        preview, failed = self._render_imgcat_preview(
            imgcat,
            artifacts.png_path,
            width=cfg["imgcat_width"],
            timeout_sec=cfg["imgcat_timeout_sec"],
        )
        if failed is not None:
            return failed

        output_lines = [
            f"artifact tex: {artifacts.tex_path}",
            f"artifact pdf: {artifacts.pdf_path}",
        ]
        if preview:
            output_lines.append(preview)
        return CodeBlock.CompileResult(
            ok=True,
            codetype=self.codetype,
            stdout="\n".join(output_lines),
            returncode=0,
        )

    def _require_tool(self, tool_name: str) -> str | CompileResult:
        path = shutil.which(tool_name)
        if path is None:
            return self._fail(f"{tool_name} not found in PATH", 127)
        return path

    def _prepare_latex_artifacts(
        self,
        mdoc_root: Path,
        fnode: str,
    ) -> tuple[LatexArtifacts | None, CompileResult | None]:
        tex_dir = mdoc_root.resolve() / ".mdc" / ".tex"
        try:
            tex_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            return None, self._fail(f"failed to create tex artifact dir: {exc}", 1)

        safe_fnode = "".join(
            ch if ch.isalnum() or ch in ("-", "_", ".") else "_"
            for ch in fnode.strip()
        )
        stem = f"snippet_{safe_fnode}"
        artifacts = CodeBlock.LatexArtifacts(
            tex_dir=tex_dir,
            tex_path=tex_dir / f"{stem}.tex",
            pdf_path=tex_dir / f"{stem}.pdf",
            png_prefix=tex_dir / stem,
            png_path=tex_dir / f"{stem}.png",
        )
        return artifacts, None

    def _latex_payload(
        self,
        *,
        preamble: str,
        postamble: str,
    ) -> str:
        if "\\documentclass" in self.content:
            return self.content

        preamble_text = preamble.rstrip("\n")
        body = self.content.rstrip("\n")
        postamble_text = postamble.strip("\n")

        parts = [preamble_text, body]
        if postamble_text:
            parts.append(postamble_text)
        return "\n".join(parts) + "\n"

    def _summarize_latex_error(self, stdout: str, stderr: str) -> str:
        combined = "\n".join([stdout or "", stderr or ""]).strip()
        lines = combined.splitlines()
        error_lines = [line for line in lines if line.startswith("! ")]
        summary_lines = error_lines[-8:] if error_lines else lines[-24:]
        return "\n".join(summary_lines).strip()

    def _run_process(
        self,
        command: list[str],
        *,
        tool_name: str,
        timeout_sec: int,
        cwd: Path | None = None,
    ) -> tuple[subprocess.CompletedProcess[str] | None, CompileResult | None]:
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
            return None, self._fail(
                f"{tool_name} timed out after {timeout_sec} seconds",
                124,
            )
        except OSError as exc:
            return None, self._fail(f"failed to run {tool_name}: {exc}", 127)
        return proc, None

    def _render_imgcat_preview(
        self,
        imgcat_path: str,
        png_path: Path,
        *,
        width: str,
        timeout_sec: int,
    ) -> tuple[str, CompileResult | None]:
        commands: list[list[str]] = []
        imgcat_width = width.strip()
        if imgcat_width:
            commands.append([imgcat_path, "--width", imgcat_width, str(png_path)])
        commands.append([imgcat_path, str(png_path)])

        last_detail = ""
        for command in commands:
            proc, failed = self._run_process(
                command,
                tool_name="imgcat",
                timeout_sec=timeout_sec,
            )
            if failed is not None:
                return "", failed
            assert proc is not None
            if proc.returncode == 0:
                return proc.stdout.rstrip("\n"), None
            last_detail = (proc.stderr or proc.stdout or "").strip()

        if not last_detail:
            last_detail = "imgcat failed to render png"
        return "", self._fail(last_detail, 1)
