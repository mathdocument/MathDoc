import shutil
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

import mathdoc.codeblock as codeblock_module
from mathdoc.codeblock import (
    BlockCompiler,
    CodeBlock,
    CompilerRegistry,
    CompileResult,
)


class TestCodeBlock(unittest.TestCase):
    @staticmethod
    def _result(block: CodeBlock) -> CompileResult:
        return block.require_result()

    @staticmethod
    def _config() -> dict[str, object]:
        return {
            "src": {
                "natl": {},
                "py": {"timeout_sec": 30},
                "latex": {
                    "timeout_sec": 30,
                    "preamble": "\\documentclass{article}\n\\begin{document}\n",
                    "postamble": "\\end{document}\n",
                },
            }
        }

    def test_compile_natl(self) -> None:
        block = CodeBlock(codetype="natl", content="hello natl\n")
        block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
        result = self._result(block)
        self.assertTrue(result.ok)
        self.assertEqual(result.stdout, "hello natl")
        self.assertEqual(result.returncode, 0)
        self.assertIsNotNone(block.context)

    def test_compile_natl_fails_when_src_config_is_not_table(self) -> None:
        block = CodeBlock(codetype="natl", content="hello natl\n")
        block.compile(
            mdoc_root=Path.cwd(),
            fnode="test-fnode",
            config={"src": "invalid"},
        )
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("config key 'src' must be a table", result.stderr)
        self.assertIsNone(block.context)

    def test_compile_py_success(self) -> None:
        block = CodeBlock(codetype="py", content="print('hello py')")
        block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
        result = self._result(block)
        self.assertTrue(result.ok)
        self.assertEqual(result.stdout.strip(), "hello py")
        self.assertEqual(result.returncode, 0)

    def test_compile_py_failure(self) -> None:
        block = CodeBlock(codetype="py", content="1/0")
        block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ZeroDivisionError", result.stderr)

    def test_compile_py_respects_timeout_from_config(self) -> None:
        block = CodeBlock(codetype="py", content="import time; time.sleep(2)")
        cfg = self._config()
        cfg["src"]["py"]["timeout_sec"] = 1  # type: ignore[index]
        block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=cfg)
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 124)
        self.assertIn("timed out", result.stderr)

    def test_compile_unsupported_codetype(self) -> None:
        block = CodeBlock(codetype="cpp", content="int main() { return 0; }")
        block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("unsupported codetype", result.stderr)

    def test_compile_fails_when_compiler_omits_result(self) -> None:
        class NoResultCompiler(BlockCompiler):
            @property
            def codetype(self) -> str:
                return "noop"

            def compile(self, block: CodeBlock) -> None:
                _ = block

        original_registry = codeblock_module.DEFAULT_COMPILER_REGISTRY
        try:
            codeblock_module.DEFAULT_COMPILER_REGISTRY = CompilerRegistry(
                [NoResultCompiler()]
            )
            block = CodeBlock(codetype="noop", content="hello")
            block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
            result = self._result(block)
        finally:
            codeblock_module.DEFAULT_COMPILER_REGISTRY = original_registry

        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("compiler did not set compile result", result.stderr)

    def test_compile_latex_success_when_xelatex_exists(self) -> None:
        required = ("latexmk", "xelatex")
        missing = [name for name in required if shutil.which(name) is None]
        if missing:
            self.skipTest(f"missing tools: {', '.join(missing)}")
        block = CodeBlock(codetype="latex", content=r"$a^2+b^2=c^2$")
        fnode = "abc12345-0000-1111-2222-fedcba987654"
        with tempfile.TemporaryDirectory(prefix="mdc_codeblock_latex.") as tmp:
            tmp_path = Path(tmp)
            block.compile(mdoc_root=tmp_path, fnode=fnode, config=self._config())
            result = self._result(block)
        self.assertTrue(result.ok, result.stderr)
        self.assertEqual(result.returncode, 0)
        self.assertIn(f"snippet_{fnode}.tex", result.stdout)
        self.assertIn("artifact dir:", result.stdout)
        self.assertIn("artifact tex:", result.stdout)
        self.assertIn("artifact pdf:", result.stdout)

    def test_compile_latex_without_xelatex(self) -> None:
        # Simulate missing xelatex by using an unknown codetype path via monkeypatch.
        # Keep this test lightweight and deterministic by patching shutil.which.
        original_which = shutil.which
        try:
            shutil.which = lambda name: None if name == "xelatex" else original_which(name)  # type: ignore[assignment]
            block = CodeBlock(codetype="latex", content=r"$x$")
            block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
            result = self._result(block)
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("xelatex not found", result.stderr)

    def test_compile_latex_without_latexmk(self) -> None:
        original_which = shutil.which
        try:
            shutil.which = lambda name: None if name == "latexmk" else original_which(name)  # type: ignore[assignment]
            block = CodeBlock(codetype="latex", content=r"$x$")
            block.compile(mdoc_root=Path.cwd(), fnode="test-fnode", config=self._config())
            result = self._result(block)
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("latexmk not found", result.stderr)

    def test_compile_latex_requires_non_empty_fnode(self) -> None:
        block = CodeBlock(codetype="latex", content=r"$x$")
        block.compile(mdoc_root=Path.cwd(), fnode="   ", config=self._config())
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("fnode is required", result.stderr)

    def test_compile_requires_mdoc_root(self) -> None:
        block = CodeBlock(codetype="natl", content="x")
        block.compile(
            mdoc_root=None,  # type: ignore[arg-type]
            fnode="test-fnode",
            config=self._config(),
        )
        result = self._result(block)
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("mdoc_root is required", result.stderr)


if __name__ == "__main__":
    unittest.main()
