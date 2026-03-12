import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.config import DEFAULT_CONFIG, load_config


class TestConfig(unittest.TestCase):
    def test_load_config_returns_defaults_when_missing(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_config_default.") as tmp:
            cfg = load_config(Path(tmp))
        self.assertEqual(cfg["src"]["latex"]["preamble"], DEFAULT_CONFIG["src"]["latex"]["preamble"])
        self.assertEqual(cfg["src"]["latex"]["postamble"], DEFAULT_CONFIG["src"]["latex"]["postamble"])
        self.assertEqual(cfg["src"]["py"]["timeout_sec"], DEFAULT_CONFIG["src"]["py"]["timeout_sec"])

    def test_load_config_reads_empty_file(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_config_empty.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / ".mdc" / "config.toml").write_text("", encoding="utf-8")
            cfg = load_config(root)
        self.assertEqual(cfg, DEFAULT_CONFIG)

    def test_load_config_merges_partial_overrides(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_config_merge.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / ".mdc" / "config.toml").write_text(
                "[src.latex]\n"
                "preamble = \"\\\\documentclass{standalone}\\n\\\\begin{document}\\n\"\n"
                "[src.py]\n"
                "timeout_sec = 5\n",
                encoding="utf-8",
            )
            cfg = load_config(root)

        self.assertEqual(cfg["src"]["latex"]["preamble"], "\\documentclass{standalone}\n\\begin{document}\n")
        self.assertEqual(cfg["src"]["latex"]["postamble"], DEFAULT_CONFIG["src"]["latex"]["postamble"])
        self.assertEqual(cfg["src"]["py"]["timeout_sec"], 5)
        self.assertEqual(cfg["src"]["natl"]["depens"], DEFAULT_CONFIG["src"]["natl"]["depens"])

    def test_load_config_invalid_toml_raises(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_config_bad_toml.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / ".mdc" / "config.toml").write_text("[src\nbad = true\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                load_config(root)


if __name__ == "__main__":
    unittest.main()
