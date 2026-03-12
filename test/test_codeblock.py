import shutil
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.codeblock import CodeBlock


class TestCodeBlock(unittest.TestCase):
    def test_compile_natl(self) -> None:
        block = CodeBlock(codetype="natl", content="hello natl\n")
        result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
        self.assertTrue(result.ok)
        self.assertEqual(result.codetype, "natl")
        self.assertEqual(result.stdout, "hello natl")
        self.assertEqual(result.returncode, 0)

    def test_compile_natl_fails_when_config_toml_is_invalid(self) -> None:
        block = CodeBlock(codetype="natl", content="hello natl\n")
        with tempfile.TemporaryDirectory(prefix="mdc_codeblock_bad_cfg.") as tmp:
            tmp_path = Path(tmp)
            (tmp_path / ".mdc").mkdir(parents=True, exist_ok=True)
            (tmp_path / ".mdc" / "config.toml").write_text(
                "[src\ninvalid = true\n",
                encoding="utf-8",
            )
            result = block.compile(mdoc_root=tmp_path, fnode="test-fnode")
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("failed to load config.toml", result.stderr)

    def test_compile_py_success(self) -> None:
        block = CodeBlock(codetype="py", content="print('hello py')")
        result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
        self.assertTrue(result.ok)
        self.assertEqual(result.codetype, "py")
        self.assertEqual(result.stdout.strip(), "hello py")
        self.assertEqual(result.returncode, 0)

    def test_compile_py_failure(self) -> None:
        block = CodeBlock(codetype="py", content="1/0")
        result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
        self.assertFalse(result.ok)
        self.assertEqual(result.codetype, "py")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ZeroDivisionError", result.stderr)

    def test_compile_py_respects_timeout_from_config(self) -> None:
        block = CodeBlock(codetype="py", content="import time; time.sleep(2)")
        with tempfile.TemporaryDirectory(prefix="mdc_codeblock_py_timeout.") as tmp:
            tmp_path = Path(tmp)
            (tmp_path / ".mdc").mkdir(parents=True, exist_ok=True)
            (tmp_path / ".mdc" / "config.toml").write_text(
                "[src.py]\n"
                "timeout_sec = 1\n",
                encoding="utf-8",
            )
            result = block.compile(mdoc_root=tmp_path, fnode="test-fnode")
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 124)
        self.assertIn("timed out", result.stderr)

    def test_compile_unsupported_codetype(self) -> None:
        block = CodeBlock(codetype="cpp", content="int main() { return 0; }")
        result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("unsupported codetype", result.stderr)

    def test_compile_latex_success_when_xelatex_exists(self) -> None:
        required = ("latexmk", "xelatex")
        missing = [name for name in required if shutil.which(name) is None]
        if missing:
            self.skipTest(f"missing tools: {', '.join(missing)}")
        block = CodeBlock(codetype="latex", content=r"$a^2+b^2=c^2$")
        fnode = "abc12345-0000-1111-2222-fedcba987654"
        with tempfile.TemporaryDirectory(prefix="mdc_codeblock_latex.") as tmp:
            tmp_path = Path(tmp)
            result = block.compile(mdoc_root=tmp_path, fnode=fnode)
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
            result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
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
            result = block.compile(mdoc_root=Path.cwd(), fnode="test-fnode")
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("latexmk not found", result.stderr)

    def test_compile_latex_requires_non_empty_fnode(self) -> None:
        block = CodeBlock(codetype="latex", content=r"$x$")
        result = block.compile(mdoc_root=Path.cwd(), fnode="   ")
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("fnode is required", result.stderr)

    def test_compile_requires_mdoc_root(self) -> None:
        block = CodeBlock(codetype="natl", content="x")
        result = block.compile(mdoc_root=None, fnode="test-fnode")  # type: ignore[arg-type]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 1)
        self.assertIn("mdoc_root is required", result.stderr)


if __name__ == "__main__":
    unittest.main()
