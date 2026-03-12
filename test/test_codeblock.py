import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.codeblock import CodeBlock


def _write_fake_imgcat(bin_dir: Path) -> Path:
    script = bin_dir / "imgcat"
    script.write_text(
        "#!/bin/sh\n"
        "file=''\n"
        "while [ \"$#\" -gt 0 ]; do\n"
        "  case \"$1\" in\n"
        "    --width|--height)\n"
        "      shift 2\n"
        "      ;;\n"
        "    --*)\n"
        "      shift\n"
        "      ;;\n"
        "    *)\n"
        "      file=\"$1\"\n"
        "      break\n"
        "      ;;\n"
        "  esac\n"
        "done\n"
        "if [ -z \"$file\" ]; then\n"
        "  exit 2\n"
        "fi\n"
        "cat \"$file\" >/dev/null || exit 1\n"
        "printf '\\033]1337;File=name=fake.png;inline=1:RkFLRQ==\\a\\n'\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return script


class TestCodeBlock(unittest.TestCase):
    def test_compile_natl(self) -> None:
        block = CodeBlock(codetype="natl", content="hello natl\n")
        result = block.compile()
        self.assertTrue(result.ok)
        self.assertEqual(result.codetype, "natl")
        self.assertEqual(result.stdout, "hello natl")
        self.assertEqual(result.returncode, 0)

    def test_compile_py_success(self) -> None:
        block = CodeBlock(codetype="py", content="print('hello py')")
        result = block.compile()
        self.assertTrue(result.ok)
        self.assertEqual(result.codetype, "py")
        self.assertEqual(result.stdout.strip(), "hello py")
        self.assertEqual(result.returncode, 0)

    def test_compile_py_failure(self) -> None:
        block = CodeBlock(codetype="py", content="1/0")
        result = block.compile()
        self.assertFalse(result.ok)
        self.assertEqual(result.codetype, "py")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ZeroDivisionError", result.stderr)

    def test_compile_unsupported_codetype(self) -> None:
        block = CodeBlock(codetype="cpp", content="int main() { return 0; }")
        result = block.compile()
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("unsupported codetype", result.stderr)

    def test_compile_latex_success_when_xelatex_exists(self) -> None:
        required = ("xelatex", "pdftoppm")
        missing = [name for name in required if shutil.which(name) is None]
        if missing:
            self.skipTest(f"missing tools: {', '.join(missing)}")
        block = CodeBlock(codetype="latex", content=r"$a^2+b^2=c^2$")
        with tempfile.TemporaryDirectory(prefix="mdc_codeblock_latex.") as tmp:
            tmp_path = Path(tmp)
            bin_dir = tmp_path / "bin"
            bin_dir.mkdir(parents=True, exist_ok=True)
            _write_fake_imgcat(bin_dir)
            path_env = f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"
            with patch.dict(os.environ, {"PATH": path_env}, clear=False):
                result = block.compile(mdoc_root=tmp_path)
        self.assertTrue(result.ok, result.stderr)
        self.assertEqual(result.returncode, 0)
        self.assertIn("artifact tex:", result.stdout)
        self.assertIn("artifact pdf:", result.stdout)
        self.assertIn("\x1b]1337;File=name=fake.png;inline=1:", result.stdout)

    def test_compile_latex_without_xelatex(self) -> None:
        # Simulate missing xelatex by using an unknown codetype path via monkeypatch.
        # Keep this test lightweight and deterministic by patching shutil.which.
        original_which = shutil.which
        try:
            shutil.which = lambda name: None if name == "xelatex" else original_which(name)  # type: ignore[assignment]
            block = CodeBlock(codetype="latex", content=r"$x$")
            result = block.compile()
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("xelatex not found", result.stderr)

    def test_compile_latex_without_imgcat(self) -> None:
        if shutil.which("xelatex") is None or shutil.which("pdftoppm") is None:
            self.skipTest("xelatex/pdftoppm is not available in PATH")
        original_which = shutil.which
        try:
            shutil.which = lambda name: None if name == "imgcat" else original_which(name)  # type: ignore[assignment]
            block = CodeBlock(codetype="latex", content=r"$x$")
            result = block.compile()
        finally:
            shutil.which = original_which  # type: ignore[assignment]
        self.assertFalse(result.ok)
        self.assertEqual(result.returncode, 127)
        self.assertIn("imgcat not found", result.stderr)


if __name__ == "__main__":
    unittest.main()
