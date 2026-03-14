import sys

from .base import SrcCompiler, CompilerReq, CompilerRes


class CompilerPy(SrcCompiler):
    @property
    def srctype(self) -> str:
        return "py"

    def compile(self, req: CompilerReq) -> CompilerRes:
        try:
            timeout_sec = self._read_positive_int(
                compcfg=req.compcfg,
                key="timeout_sec",
                full_key="src.py.timeout_sec",
            )
            proc = self._run_process(
                [sys.executable, "-c", req.content],
                tool_name="python",
                timeout_sec=timeout_sec,
            )
        except (KeyError, ValueError) as exc:
            return CompilerRes(
                result=False,
                stdout="",
                stderr=exc.args[0] if exc.args else str(exc),
                rtcode=1,
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

        return CompilerRes(
            result=proc.returncode == 0,
            stdout=proc.stdout,
            stderr=proc.stderr,
            rtcode=proc.returncode,
        )
