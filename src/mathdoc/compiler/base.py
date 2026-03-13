from __future__ import annotations

import shutil
import subprocess
from abc import ABC, abstractmethod
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ..srcblock import SrcBlock


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
            block._set_result_failed(f"config key '{full_key}' is required", 1)
            return None
        value = config[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            block._set_result_failed(
                f"config key '{full_key}' must be a positive integer", 1
            )
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
            block._set_result_failed(f"config key '{full_key}' is required", 1)
            return None
        value = config[key]
        if not isinstance(value, str):
            block._set_result_failed(f"config key '{full_key}' must be a string", 1)
            return None
        return value

    def _require_tool(self, block: SrcBlock, tool_name: str) -> str | None:
        path = shutil.which(tool_name)
        if path is None:
            block._set_result_failed(f"{tool_name} not found in PATH", 127)
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
            block._set_result_failed(
                f"{tool_name} timed out after {timeout_sec} seconds", 124
            )
            return None
        except OSError as exc:
            block._set_result_failed(f"failed to run {tool_name}: {exc}", 127)
            return None
        return proc
