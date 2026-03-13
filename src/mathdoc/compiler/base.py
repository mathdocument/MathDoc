from dataclasses import dataclass, field

import shutil
import subprocess
from abc import ABC, abstractmethod
from pathlib import Path
from typing import Any


@dataclass(slots=True, kw_only=True)
class CompilerReq:
    mdcroot: Path
    srctype: str
    content: str
    compcfg: dict[str, Any] = field(default_factory=dict)


@dataclass(slots=True, kw_only=True)
class CompilerRes:
    result: bool = True
    stdout: str = ""
    stderr: str = ""
    rtcode: int = 0


class SrcCompiler(ABC):
    @property
    @abstractmethod
    def srctype(self) -> str:
        raise NotImplementedError

    @abstractmethod
    def compile(self, req: CompilerReq) -> CompilerRes:
        raise NotImplementedError

    def _read_positive_int(
        self,
        *,
        compcfg: dict[str, Any],
        key: str,
        full_key: str,
    ) -> int:
        config = compcfg
        if key not in config:
            raise KeyError(f"config key '{full_key}' is required")
        value = config[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"config key '{full_key}' must be a positive integer")
        return value

    def _read_str(
        self,
        *,
        compcfg: dict[str, Any],
        key: str,
        full_key: str,
    ) -> str:
        config = compcfg
        if key not in config:
            raise KeyError(f"config key '{full_key}' is required")
        value = config[key]
        if not isinstance(value, str):
            raise ValueError(f"config key '{full_key}' must be a string")
        return value

    def _require_tool(self, tool_name: str) -> str:
        path = shutil.which(tool_name)
        if path is None:
            raise FileNotFoundError(f"{tool_name} not found in PATH")
        return path

    def _run_process(
        self,
        command: list[str],
        *,
        tool_name: str,
        timeout_sec: int,
        cwd: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
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
            raise TimeoutError(f"{tool_name} timed out after {timeout_sec} seconds")
        except OSError as exc:
            raise RuntimeError(f"failed to run {tool_name}: {exc}")
        return proc
