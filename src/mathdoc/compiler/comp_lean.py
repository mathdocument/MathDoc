import re
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .base import CompilerReq
from .base import CompilerRes
from .base import SrcCompiler


class CompilerLean(SrcCompiler):
    MANAGED_MARKER = "# Managed by MathDoc"
    MODULE_NAME = "MathDocCheck"
    WORKSPACE_REL = Path(".mdc") / "lean"
    CACHE_STAMP = ".mathdoc-cache.stamp"

    @dataclass(slots=True, frozen=True)
    class LeanRelease:
        version: str
        toolchain: str
        mathlib_rev: str

    @dataclass(slots=True, frozen=True)
    class LeanWorkspace:
        root: Path
        lakefile_path: Path
        toolchain_path: Path
        module_path: Path
        manifest_path: Path
        mathlib_sentinel: Path
        cache_stamp_path: Path

    @property
    def srctype(self) -> str:
        return "lean"

    def compile(self, req: CompilerReq) -> CompilerRes:
        try:
            timeout_sec = self._read_positive_int(
                compcfg=req.compcfg,
                key="timeout_sec",
                full_key="src.lean.timeout_sec",
            )
            setup_timeout_sec = self._read_positive_int(
                compcfg=req.compcfg,
                key="setup_timeout_sec",
                full_key="src.lean.setup_timeout_sec",
            )
            imports = self._read_imports(req.compcfg)
            preamble = self._read_optional_str(
                compcfg=req.compcfg,
                key="preamble",
                full_key="src.lean.preamble",
            )
        except (KeyError, ValueError) as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=exc.args[0] if exc.args else str(exc),
                rtcode=1,
            )

        try:
            lake = self._require_tool("lake")
            lean = self._require_tool("lean")
            release = self._detect_lean_release(
                lean_path=lean,
                timeout_sec=min(timeout_sec, 10),
            )
            workspace = self._workspace(req.mdcroot)
            self._write_workspace_scaffold(workspace, release=release)
            self._ensure_cached_dependencies(
                workspace=workspace,
                release=release,
                lake_path=lake,
                timeout_sec=setup_timeout_sec,
                progress=req.progress,
            )
            payload = self._lean_payload(
                content=req.content,
                imports=imports,
                preamble=preamble,
            )
            workspace.module_path.write_text(payload, encoding="utf-8")
            self._emit_progress(
                req.progress,
                f"building Lean module `{self.MODULE_NAME}` with `lake build +{self.MODULE_NAME}`",
            )
            build_proc = self._run_process(
                [
                    lake,
                    "--quiet",
                    "--no-ansi",
                    "build",
                    f"+{self.MODULE_NAME}",
                ],
                tool_name=f"lake build +{self.MODULE_NAME}",
                timeout_sec=timeout_sec,
                cwd=workspace.root,
            )
        except FileNotFoundError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=str(exc),
                rtcode=127,
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
                rtcode=1,
            )
        except OSError as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=f"failed to prepare Lean workspace: {exc}",
                rtcode=1,
            )

        stdout, stderr = self._classify_build_output(
            stdout=build_proc.stdout,
            stderr=build_proc.stderr,
            ok=build_proc.returncode == 0,
        )
        return CompilerRes(
            result=build_proc.returncode == 0,
            stdout=stdout,
            stderr=stderr,
            rtcode=build_proc.returncode,
        )

    def _detect_lean_release(
        self,
        *,
        lean_path: str,
        timeout_sec: int,
    ) -> LeanRelease:
        proc = self._run_process(
            [lean_path, "--version"],
            tool_name="lean",
            timeout_sec=timeout_sec,
        )
        version = self._parse_lean_version(proc.stdout or proc.stderr)
        if version is None:
            raise RuntimeError("failed to detect Lean version from `lean --version`")
        return CompilerLean.LeanRelease(
            version=version,
            toolchain=f"leanprover/lean4:v{version}",
            mathlib_rev=f"v{version}",
        )

    @staticmethod
    def _parse_lean_version(output: str) -> str | None:
        match = re.search(r"version\s+([0-9]+\.[0-9]+\.[0-9]+)", output)
        if match is None:
            return None
        return match.group(1)

    def _workspace(self, mdcroot: Path) -> LeanWorkspace:
        root = mdcroot.resolve() / self.WORKSPACE_REL
        return CompilerLean.LeanWorkspace(
            root=root,
            lakefile_path=root / "lakefile.toml",
            toolchain_path=root / "lean-toolchain",
            module_path=root / f"{self.MODULE_NAME}.lean",
            manifest_path=root / "lake-manifest.json",
            mathlib_sentinel=root / ".lake" / "packages" / "mathlib" / "Mathlib.lean",
            cache_stamp_path=root / self.CACHE_STAMP,
        )

    def _write_workspace_scaffold(
        self,
        workspace: LeanWorkspace,
        *,
        release: LeanRelease,
    ) -> None:
        workspace.root.mkdir(parents=True, exist_ok=True)
        workspace.lakefile_path.write_text(
            self._lakefile_text(release=release),
            encoding="utf-8",
        )
        workspace.toolchain_path.write_text(f"{release.toolchain}\n", encoding="utf-8")

    def _ensure_cached_dependencies(
        self,
        *,
        workspace: LeanWorkspace,
        release: LeanRelease,
        lake_path: str,
        timeout_sec: int,
        progress: Callable[[str], None] | None = None,
    ) -> None:
        if self._cache_is_ready(workspace, release=release):
            return

        self._emit_progress(
            progress,
            f"preparing Lean workspace in `{self.WORKSPACE_REL.as_posix()}` and resolving Mathlib dependencies (first run may take a while)",
        )
        self._emit_progress(
            progress,
            "resolving Mathlib dependencies with `lake update`",
        )
        update_proc = self._run_process(
            [lake_path, "--quiet", "--no-ansi", "update"],
            tool_name="lake update",
            timeout_sec=timeout_sec,
            cwd=workspace.root,
        )
        if update_proc.returncode != 0:
            raise RuntimeError(
                "failed to initialize Lean dependencies:\n"
                + self._combine_output(update_proc.stdout, update_proc.stderr)
            )

        self._emit_progress(
            progress,
            "downloading Mathlib cache with `lake exe cache get`",
        )
        cache_proc = self._run_process(
            [lake_path, "--quiet", "--no-ansi", "exe", "cache", "get"],
            tool_name="lake exe cache get",
            timeout_sec=timeout_sec,
            cwd=workspace.root,
        )
        if cache_proc.returncode != 0:
            raise RuntimeError(
                "failed to download Mathlib cache:\n"
                + self._combine_output(cache_proc.stdout, cache_proc.stderr)
            )

        workspace.cache_stamp_path.write_text(
            self._cache_signature(release),
            encoding="utf-8",
        )

    def _cache_is_ready(
        self,
        workspace: LeanWorkspace,
        *,
        release: LeanRelease,
    ) -> bool:
        if not workspace.manifest_path.is_file():
            return False
        if not workspace.mathlib_sentinel.is_file():
            return False
        if not workspace.cache_stamp_path.is_file():
            return False
        try:
            signature = workspace.cache_stamp_path.read_text(encoding="utf-8")
        except OSError:
            return False
        return signature == self._cache_signature(release)

    @staticmethod
    def _cache_signature(release: LeanRelease) -> str:
        return "\n".join(
            [
                f"toolchain={release.toolchain}",
                f"mathlib_rev={release.mathlib_rev}",
            ]
        )

    def _lakefile_text(self, *, release: LeanRelease) -> str:
        return "\n".join(
            [
                self.MANAGED_MARKER,
                'name = "mathdoclean"',
                'version = "0.1.0"',
                f'defaultTargets = ["{self.MODULE_NAME}"]',
                "",
                "[leanOptions]",
                "pp.unicode.fun = true",
                "relaxedAutoImplicit = false",
                "weak.linter.mathlibStandardSet = true",
                "maxSynthPendingDepth = 3",
                "",
                "[[require]]",
                'name = "mathlib"',
                'scope = "leanprover-community"',
                f'rev = "{release.mathlib_rev}"',
                "",
                "[[lean_lib]]",
                f'name = "{self.MODULE_NAME}"',
                "",
            ]
        )

    def _read_imports(self, compcfg: dict[str, object]) -> list[str]:
        raw = compcfg.get("imports", ["Mathlib"])
        if not isinstance(raw, list):
            raise ValueError(
                "config key 'src.lean.imports' must be an array of strings"
            )
        imports: list[str] = []
        seen: set[str] = set()
        for item in raw:
            if not isinstance(item, str):
                raise ValueError(
                    "config key 'src.lean.imports' must be an array of strings"
                )
            value = item.strip()
            if not value or value in seen:
                continue
            seen.add(value)
            imports.append(value)
        return imports

    @staticmethod
    def _read_optional_str(
        *,
        compcfg: dict[str, object],
        key: str,
        full_key: str,
    ) -> str:
        value = compcfg.get(key, "")
        if not isinstance(value, str):
            raise ValueError(f"config key '{full_key}' must be a string")
        return value

    def _lean_payload(
        self,
        *,
        content: str,
        imports: list[str],
        preamble: str,
    ) -> str:
        head_imports, body = self._extract_leading_imports(content)
        merged_imports: list[str] = []
        seen: set[str] = set()
        for item in [*imports, *head_imports]:
            if item in seen:
                continue
            seen.add(item)
            merged_imports.append(item)

        parts: list[str] = []
        for imp in merged_imports:
            parts.append(f"import {imp}")

        if parts:
            parts.append("")
        parts.append("set_option warn.sorry true")

        preamble_text = preamble.strip("\n")
        if preamble_text:
            parts.append(preamble_text)

        body_text = body.strip("\n")
        if body_text:
            parts.extend(["", body_text])

        return "\n".join(parts).rstrip() + "\n"

    @staticmethod
    def _extract_leading_imports(content: str) -> tuple[list[str], str]:
        lines = content.splitlines()
        index = 0
        while index < len(lines) and not lines[index].strip():
            index += 1

        imports: list[str] = []
        while index < len(lines):
            line = lines[index].strip()
            if not line.startswith("import "):
                break
            target = line[len("import ") :].strip()
            if target:
                imports.append(target)
            index += 1

        return imports, "\n".join(lines[index:])

    def _classify_build_output(
        self,
        *,
        stdout: str,
        stderr: str,
        ok: bool,
    ) -> tuple[str, str]:
        lines = self._clean_output_lines(stdout, stderr)
        if not lines:
            return "", ""

        if not ok:
            return "", "\n".join(lines)

        out_lines: list[str] = []
        err_lines: list[str] = []
        for line in lines:
            if line.startswith("warning:") or line.startswith("error:"):
                err_lines.append(line)
            else:
                out_lines.append(line)
        return "\n".join(out_lines).strip(), "\n".join(err_lines).strip()

    def _combine_output(self, stdout: str, stderr: str) -> str:
        lines = self._clean_output_lines(stdout, stderr)
        return "\n".join(lines).strip() or "no diagnostic output"

    @staticmethod
    def _emit_progress(
        progress: Callable[[str], None] | None,
        message: str,
    ) -> None:
        if progress is not None:
            progress(message)

    @staticmethod
    def _clean_output_lines(stdout: str, stderr: str) -> list[str]:
        lines: list[str] = []
        for raw in (stdout, stderr):
            for line in raw.replace("\r\n", "\n").replace("\r", "\n").splitlines():
                text = line.strip()
                if not text:
                    continue
                if re.match(r"^[⚠✔✖ℹ]\s+\[\d+/\d+\]\s+Built\b", text):
                    continue
                if text == "Build completed successfully (0 jobs).":
                    continue
                if text.startswith(
                    "warning: failed to query latest release, using existing version "
                ):
                    continue
                lines.append(text)
        return lines
