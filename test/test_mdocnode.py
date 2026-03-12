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
    @staticmethod
    def _new_node(root: Path, title: str, codetype: str, content: str) -> MdocNode:
        (root / ".mdc").mkdir(parents=True, exist_ok=True)
        node = MdocNode.create(folder=str(root), title=title)
        node.blocks.append(CodeBlock(codetype=codetype, content=content, metadata={}))
        return node

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
            node = self._new_node(root, "Eval", "natl", "hello")
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

    def test_eval_blocks_requires_initialized_mdoc_root(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_need_root.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(folder=str(root), title="Eval")
            node.blocks.append(CodeBlock(codetype="natl", content="hello", metadata={}))
            node.save()

            loaded = MdocNode(path=node.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks(mdoc_root=root)
            self.assertIn("invalid mdoc root (missing .mdc)", str(ctx.exception))

    def test_eval_blocks_merges_dependencies_with_default_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_depth1.") as tmp:
            root = Path(tmp)
            dep2 = self._new_node(root, "Dep2", "natl", "dep2")
            dep2.save()

            dep1 = self._new_node(root, "Dep1", "natl", "dep1")
            dep1.add_dependency(dep2.fnode)
            dep1.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep1.fnode)
            src.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(mdoc_root=root)

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0].result.ok)
            self.assertEqual(block_results[0].result.stdout, "dep1\n\nsrc")

    def test_eval_blocks_merges_dependencies_with_unbounded_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_depth_inf.") as tmp:
            root = Path(tmp)
            dep2 = self._new_node(root, "Dep2", "natl", "dep2")
            dep2.save()

            dep1 = self._new_node(root, "Dep1", "natl", "dep1")
            dep1.add_dependency(dep2.fnode)
            dep1.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep1.fnode)
            src.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(mdoc_root=root, depth=-1)

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0].result.ok)
            self.assertEqual(block_results[0].result.stdout, "dep2\n\ndep1\n\nsrc")

    def test_eval_blocks_merges_dependencies_in_reverse_order(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_reverse.") as tmp:
            root = Path(tmp)
            dep2 = self._new_node(root, "Dep2", "natl", "dep2")
            dep2.save()

            dep1 = self._new_node(root, "Dep1", "natl", "dep1")
            dep1.add_dependency(dep2.fnode)
            dep1.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep1.fnode)
            src.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(
                mdoc_root=root,
                depth=-1,
                reverse_depens=True,
            )

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0].result.ok)
            self.assertEqual(block_results[0].result.stdout, "src\n\ndep1\n\ndep2")

    def test_eval_blocks_does_not_merge_when_depens_disabled(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_no_merge.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "py", "print('dep')")
            dep.save()

            src = self._new_node(root, "Src", "py", "print('src')")
            src.add_dependency(dep.fnode)
            src.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(mdoc_root=root, depth=-1)

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0].result.ok)
            self.assertEqual(block_results[0].result.stdout.strip(), "src")

    def test_eval_blocks_raises_on_dependency_cycle(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_cycle.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "natl", "dep")
            dep.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            dep.add_dependency(src.fnode)
            dep.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks(mdoc_root=root, depth=-1)
            message = str(ctx.exception)
            self.assertIn("dependency cycle detected", message)
            self.assertIn(src.fnode, message)
            self.assertIn(dep.fnode, message)

    def test_eval_blocks_raises_on_cycle_with_depth_boundary(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_cycle_depth1.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "natl", "dep")
            dep.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            dep.add_dependency(src.fnode)
            dep.save()

            loaded = MdocNode(path=src.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks(mdoc_root=root, depth=1)
            message = str(ctx.exception)
            self.assertIn("dependency cycle detected", message)
            self.assertIn(src.fnode, message)
            self.assertIn(dep.fnode, message)


if __name__ == "__main__":
    unittest.main()
