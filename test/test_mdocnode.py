import tempfile
import unittest
from pathlib import Path
import sys

PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.mdocnode import MdocNode
from mathdoc.codeblock import CodeBlock


class TestMdocNode(unittest.TestCase):
    def test_create_save_load_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_roundtrip.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(folder=str(root), title="Roundtrip")
            node.add_dependency("dep-a")
            node.blocks.append(
                CodeBlock(codetype="text", content="hello\nworld", metadata={"lang": "en"})
            )
            node.save()

            loaded = MdocNode(path=node.path, title="")
            loaded.load()

            self.assertEqual(loaded.title, "Roundtrip")
            self.assertEqual(loaded.fnode, node.fnode)
            self.assertEqual(loaded.depens, ["dep-a"])
            self.assertEqual(len(loaded.blocks), 1)
            self.assertEqual(loaded.blocks[0].codetype, "text")
            self.assertEqual(loaded.blocks[0].content, "hello\nworld\n")
            self.assertEqual(loaded.blocks[0].metadata, {"lang": "en"})

    def test_add_dependency_is_unique(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_dep.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(folder=str(root), title="Deps")
            node.add_dependency("x")
            node.add_dependency("x")
            node.save()

            loaded = MdocNode(path=node.path, title="")
            loaded.load()
            self.assertEqual(loaded.depens, ["x"])

    def test_load_rejects_missing_required_headers(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_invalid.") as tmp:
            file_path = Path(tmp) / "bad.mdoc"
            file_path.write_text("@title: no fnode\n", encoding="utf-8")
            node = MdocNode(path=file_path, title="")
            with self.assertRaises(ValueError):
                node.load()

    def test_eval_blocks_runs_all_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(folder=str(root), title="Eval")
            node.blocks.append(CodeBlock(codetype="natl", content="hello", metadata={}))
            node.blocks.append(CodeBlock(codetype="py", content="print('hi')", metadata={}))
            node.save()

            loaded = MdocNode(path=node.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(mdoc_root=root)

            self.assertEqual(len(block_results), 2)
            self.assertEqual(block_results[0].block.codetype, "natl")
            self.assertTrue(block_results[0].result.ok)
            self.assertEqual(block_results[0].result.stdout, "hello")
            self.assertEqual(block_results[1].block.codetype, "py")
            self.assertTrue(block_results[1].result.ok)
            self.assertEqual(block_results[1].result.stdout.strip(), "hi")


if __name__ == "__main__":
    unittest.main()
