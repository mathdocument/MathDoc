from mathdoc.compiler.base import CompilerReq, CompilerRes, SrcCompiler
from mathdoc.compiler.comp_latex import CompilerLatex
from mathdoc.compiler.registry import CompilerRegistry
from mathdoc.srcblock import SrcBlock
import mathdoc.srcblock as SrcBlock_module
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))


class TestSrcBlock(unittest.TestCase):
    @staticmethod
    def _result(result: CompilerRes) -> CompilerRes:
        return result

    @staticmethod
    def _config() -> dict[str, object]:
        return {
            "natl": {},
            "py": {"timeout_sec": 30},
            "latex": {
                "timeout_sec": 30,
                "preamble": "\\documentclass{article}\n\\begin{document}\n",
                "postamble": "\\end{document}\n",
            },
        }

    def test_compiler_res_defaults_to_not_compiled(self) -> None:
        result = CompilerRes()
        self.assertFalse(result.result)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr, "not compiled")
        self.assertEqual(result.rtcode, 1)

    def test_compile_natl(self) -> None:
        block = SrcBlock(srctype="natl", content="hello natl\n")
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg=self._config()))
        self.assertTrue(result.result)
        self.assertEqual(result.stdout, "hello natl")
        self.assertEqual(result.rtcode, 0)

    def test_compile_natl_fails_when_src_config_is_not_table(self) -> None:
        block = SrcBlock(srctype="natl", content="hello natl\n")
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg={"natl": "invalid"}))
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 1)
        self.assertIn("config key 'src.natl' must be a table", result.stderr)

    def test_compile_py_success(self) -> None:
        block = SrcBlock(srctype="py", content="print('hello py')")
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg=self._config()))
        self.assertTrue(result.result)
        self.assertEqual(result.stdout.strip(), "hello py")
        self.assertEqual(result.rtcode, 0)

    def test_compile_py_failure(self) -> None:
        block = SrcBlock(srctype="py", content="1/0")
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg=self._config()))
        self.assertFalse(result.result)
        self.assertNotEqual(result.rtcode, 0)
        self.assertIn("ZeroDivisionError", result.stderr)

    def test_compile_py_respects_timeout_from_config(self) -> None:
        block = SrcBlock(srctype="py", content="import time; time.sleep(2)")
        cfg = self._config()
        cfg["py"]["timeout_sec"] = 1  # type: ignore[index]
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg=cfg))
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 124)
        self.assertIn("timed out", result.stderr)

    def test_compile_unsupported_srctype(self) -> None:
        block = SrcBlock(srctype="cpp", content="int main() { return 0; }")
        result = self._result(block.compile(mdcroot=Path.cwd(), src_cfg=self._config()))
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 127)
        self.assertIn("unsupported srctype", result.stderr)

    def test_compile_uses_returned_compiler_res(self) -> None:
        class NoopCompiler(SrcCompiler):
            @property
            def srctype(self) -> str:
                return "noop"

            def compile(self, req: CompilerReq) -> CompilerRes:
                _ = req
                return CompilerRes(
                    result=False,
                    stdout="noop stdout",
                    stderr="noop stderr",
                    rtcode=9,
                )

        original_registry = SrcBlock_module.COMPILER_REGISTRY
        try:
            SrcBlock_module.COMPILER_REGISTRY = CompilerRegistry([NoopCompiler()])
            block = SrcBlock(srctype="noop", content="hello")
            result = self._result(
                block.compile(
                    mdcroot=Path.cwd(),
                    src_cfg=self._config(),
                )
            )
        finally:
            SrcBlock_module.COMPILER_REGISTRY = original_registry

        self.assertFalse(result.result)
        self.assertEqual(result.stdout, "noop stdout")
        self.assertEqual(result.stderr, "noop stderr")
        self.assertEqual(result.rtcode, 9)

    def test_compile_latex_success_when_xelatex_exists(self) -> None:
        required = ("latexmk", "xelatex")
        missing = [name for name in required if shutil.which(name) is None]
        if missing:
            self.skipTest(f"missing tools: {', '.join(missing)}")
        block = SrcBlock(srctype="latex", content=r"$a^2+b^2=c^2$")
        with tempfile.TemporaryDirectory(prefix="mdc_SrcBlock_latex.") as tmp:
            tmp_path = Path(tmp)
            result = self._result(block.compile(mdcroot=tmp_path, src_cfg=self._config()))
        self.assertTrue(result.result, result.stderr)
        self.assertEqual(result.rtcode, 0)
        self.assertIn("temp-latex-", result.stdout)
        self.assertIn("artifact dir:", result.stdout)
        self.assertIn("artifact tex:", result.stdout)
        self.assertIn("artifact pdf:", result.stdout)

    def test_latex_artifact_paths_are_unique(self) -> None:
        compiler = CompilerLatex()
        with tempfile.TemporaryDirectory(prefix="mdc_SrcBlock_latex.artifacts.") as tmp:
            tmp_path = Path(tmp)
            artifacts1 = compiler._prepare_latex_artifacts(mdcroot=tmp_path)
            artifacts2 = compiler._prepare_latex_artifacts(mdcroot=tmp_path)

        self.assertNotEqual(artifacts1.tex_path, artifacts2.tex_path)
        self.assertNotEqual(artifacts1.pdf_path, artifacts2.pdf_path)
        self.assertTrue(artifacts1.tex_path.name.startswith("temp-latex-"))
        self.assertTrue(artifacts2.tex_path.name.startswith("temp-latex-"))

    def test_compile_latex_without_xelatex(self) -> None:
        # Simulate missing xelatex by using an unknown srctype path via monkeypatch.
        # Keep this test lightweight and deterministic by patching shutil.which.
        original_which = shutil.which
        try:
            shutil.which = lambda name: (
                None if name == "xelatex" else original_which(name)
            )  # type: ignore[assignment]
            block = SrcBlock(srctype="latex", content=r"$x$")
            result = self._result(
                block.compile(
                    mdcroot=Path.cwd(),
                    src_cfg=self._config(),
                )
            )
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 127)
        self.assertIn("xelatex not found", result.stderr)

    def test_compile_latex_without_latexmk(self) -> None:
        original_which = shutil.which
        try:
            shutil.which = lambda name: (
                None if name == "latexmk" else original_which(name)
            )  # type: ignore[assignment]
            block = SrcBlock(srctype="latex", content=r"$x$")
            result = self._result(
                block.compile(
                    mdcroot=Path.cwd(),
                    src_cfg=self._config(),
                )
            )
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 127)
        self.assertIn("latexmk not found", result.stderr)

    def test_compile_requires_mdcroot(self) -> None:
        block = SrcBlock(srctype="natl", content="x")
        result = self._result(
            block.compile(
                mdcroot=None,  # type: ignore[arg-type]
                src_cfg=self._config(),
            )
        )
        self.assertFalse(result.result)
        self.assertEqual(result.rtcode, 1)
        self.assertIn("mdcroot is required", result.stderr)


if __name__ == "__main__":
    unittest.main()
