import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .config import load_config


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

        try:
            config = load_config(mdoc_root)
        except (OSError, ValueError) as exc:
            return self._fail(f"failed to load config.toml: {exc}", 1)

        src_cfg = config.get("src", {})
        if not isinstance(src_cfg, dict):
            return self._fail("config key 'src' must be a table", 1)

        code_kind = self.codetype.strip().casefold()
        compiler_cfg = src_cfg.get(code_kind, {})
        if compiler_cfg is None:
            compiler_cfg = {}
        if not isinstance(compiler_cfg, dict):
            return self._fail(f"config key 'src.{code_kind}' must be a table", 1)

        if code_kind == "natl":
            return self._compile_natl()
        if code_kind == "py":
            return self._compile_py(compiler_cfg)
        if code_kind == "latex":
            return self._compile_latex(
                mdoc_root=mdoc_root,
                fnode=fnode,
                config=compiler_cfg,
            )
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

    def _compile_py(self, config: dict[str, Any]) -> CompileResult:
        timeout_sec, failed = self._read_positive_int(
            config,
            key="timeout_sec",
            full_key="src.py.timeout_sec",
        )
        if failed is not None:
            return failed

        proc, failed = self._run_process(
            [sys.executable, "-c", self.content],
            tool_name="python",
            timeout_sec=timeout_sec,
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
        config: dict[str, Any],
    ) -> CompileResult:
        timeout_sec, failed = self._read_positive_int(
            config,
            key="timeout_sec",
            full_key="src.latex.timeout_sec",
        )
        if failed is not None:
            return failed

        preamble, failed = self._read_str(
            config,
            key="preamble",
            full_key="src.latex.preamble",
        )
        if failed is not None:
            return failed

        postamble, failed = self._read_str(
            config,
            key="postamble",
            full_key="src.latex.postamble",
        )
        if failed is not None:
            return failed

        latexmk = self._require_tool("latexmk")
        if isinstance(latexmk, CodeBlock.CompileResult):
            return latexmk
        xelatex = self._require_tool("xelatex")
        if isinstance(xelatex, CodeBlock.CompileResult):
            return xelatex

        artifacts, failed = self._prepare_latex_artifacts(mdoc_root, fnode)
        if failed is not None:
            return failed
        assert artifacts is not None

        payload = self._latex_payload(
            preamble=preamble,
            postamble=postamble,
        )
        try:
            artifacts.tex_path.write_text(payload, encoding="utf-8")
        except OSError as exc:
            return self._fail(f"failed to write latex source: {exc}", 1)

        tex_proc, failed = self._run_process(
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
        if failed is not None:
            return failed
        assert tex_proc is not None
        if tex_proc.returncode != 0:
            return self._fail(
                self._summarize_latex_error(tex_proc.stdout, tex_proc.stderr),
                tex_proc.returncode,
            )

        if not artifacts.pdf_path.is_file():
            return self._fail(
                f"latexmk succeeded but pdf not found: {artifacts.pdf_path}",
                1,
            )

        output_lines = [
            f"artifact dir: {artifacts.tex_dir}",
            f"artifact tex: {artifacts.tex_path}",
            f"artifact pdf: {artifacts.pdf_path}",
        ]
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

    def _read_positive_int(
        self,
        config: dict[str, Any],
        *,
        key: str,
        full_key: str,
    ) -> tuple[int, CompileResult | None]:
        if key not in config:
            return 0, self._fail(f"config key '{full_key}' is required", 1)
        value = config[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            return 0, self._fail(f"config key '{full_key}' must be a positive integer", 1)
        return value, None

    def _read_str(
        self,
        config: dict[str, Any],
        *,
        key: str,
        full_key: str,
    ) -> tuple[str, CompileResult | None]:
        if key not in config:
            return "", self._fail(f"config key '{full_key}' is required", 1)
        value = config[key]
        if not isinstance(value, str):
            return "", self._fail(f"config key '{full_key}' must be a string", 1)
        return value, None
