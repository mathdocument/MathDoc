from mathdoc.compiler import CompilerRes
from mathdoc.srcblock import SrcBlock
from mathdoc.mdocnode import MdocNode
import mathdoc.mdocnode as mdocnode_module
import tempfile
import unittest
from pathlib import Path
import sys

PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))


class TestMdocNode(unittest.TestCase):
    @staticmethod
    def _new_node(root: Path, title: str, srctype: str, content: str) -> MdocNode:
        (root / ".mdc").mkdir(parents=True, exist_ok=True)
        node = MdocNode.create(mdcroot=root, folder=str(root), title=title)
        node.blocks.append(SrcBlock(srctype=srctype, content=content, metadata={}))
        return node

    @staticmethod
    def _result(entry: tuple[str, CompilerRes]) -> CompilerRes:
        return entry[1]

    def test_create_save_load_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_roundtrip.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(mdcroot=root, folder=str(root), title="Roundtrip")
            node.add_dependency("dep-a")
            node.blocks.append(
                SrcBlock(
                    srctype="text", content="hello\nworld", metadata={"lang": "en"}
                )
            )
            node.save()

            loaded = MdocNode(mdcroot=root, path=node.path, title="")
            loaded.load()

            self.assertEqual(loaded.title, "Roundtrip")
            self.assertEqual(loaded.fnode, node.fnode)
            self.assertEqual(loaded.depens, ["dep-a"])
            self.assertEqual(len(loaded.blocks), 1)
            self.assertEqual(loaded.blocks[0].srctype, "text")
            self.assertEqual(loaded.blocks[0].content, "hello\nworld\n")
            self.assertEqual(loaded.blocks[0].metadata, {"lang": "en"})

    def test_add_dependency_is_unique(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_dep.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(mdcroot=root, folder=str(root), title="Deps")
            node.add_dependency("x")
            node.add_dependency("x")
            node.save()

            loaded = MdocNode(mdcroot=root, path=node.path, title="")
            loaded.load()
            self.assertEqual(loaded.depens, ["x"])

    def test_load_rejects_missing_required_headers(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_invalid.") as tmp:
            file_path = Path(tmp) / "bad.mdoc"
            file_path.write_text("@title: no fnode\n", encoding="utf-8")
            node = MdocNode(mdcroot=Path(tmp), path=file_path, title="")
            with self.assertRaises(ValueError):
                node.load()

    def test_load_preserves_blank_lines_in_src_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_blank_src.") as tmp:
            root = Path(tmp)
            file_path = root / "blank.mdoc"
            file_path.write_text(
                "@fnode: blank-node\n"
                "@title: Blank Lines\n"
                "\n"
                "@src: py\n"
                "print('line1')\n"
                "\n"
                "print('line3')\n"
                "@end\n",
                encoding="utf-8",
            )
            node = MdocNode(mdcroot=root, path=file_path, title="")
            node.load()

            self.assertEqual(len(node.blocks), 1)
            self.assertEqual(
                node.blocks[0].content,
                "print('line1')\n\nprint('line3')\n",
            )

    def test_load_dependency_keeps_full_token(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_dep_token.") as tmp:
            root = Path(tmp)
            file_path = root / "dep-token.mdoc"
            file_path.write_text(
                "@fnode: dep-node\n@title: Dep Token\n\n@dep:\nabc:def\n@end\n",
                encoding="utf-8",
            )
            node = MdocNode(mdcroot=root, path=file_path, title="")
            node.load()

            self.assertEqual(node.depens, ["abc:def"])

    def test_eval_blocks_runs_all_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval.") as tmp:
            root = Path(tmp)
            node = self._new_node(root, "Eval", "natl", "hello")
            node.blocks.append(
                SrcBlock(srctype="py", content="print('hi')", metadata={})
            )
            node.save()

            loaded = MdocNode(mdcroot=root, path=node.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks()

            self.assertEqual(len(block_results), 2)
            result0 = self._result(block_results[0])
            result1 = self._result(block_results[1])
            self.assertEqual(block_results[0][0], "natl")
            self.assertTrue(result0.result)
            self.assertEqual(result0.stdout, "hello")
            self.assertEqual(block_results[1][0], "py")
            self.assertTrue(result1.result)
            self.assertEqual(result1.stdout.strip(), "hi")

    def test_eval_blocks_loads_config_once(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_cfg_once.") as tmp:
            root = Path(tmp)
            node = self._new_node(root, "Eval", "natl", "hello")
            node.blocks.append(
                SrcBlock(srctype="py", content="print('hi')", metadata={})
            )
            node.save()

            loaded = MdocNode(mdcroot=root, path=node.path, title="")
            loaded.load()

            mdoc_calls = 0
            original_mdoc_load = mdocnode_module.load_config

            try:

                def counted_mdoc_load(mdcroot: Path) -> dict[str, object]:
                    nonlocal mdoc_calls
                    mdoc_calls += 1
                    return original_mdoc_load(mdcroot)

                # type: ignore[assignment]
                mdocnode_module.load_config = counted_mdoc_load
                loaded.eval_blocks(depth=-1)
            finally:
                # type: ignore[assignment]
                mdocnode_module.load_config = original_mdoc_load

            self.assertEqual(mdoc_calls, 1)

    def test_eval_blocks_requires_initialized_mdcroot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_need_root.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(mdcroot=root, folder=str(root), title="Eval")
            node.blocks.append(SrcBlock(srctype="natl", content="hello", metadata={}))
            node.save()

            loaded = MdocNode(mdcroot=root, path=node.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks()
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

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks()

            self.assertEqual(len(block_results), 1)
            result0 = self._result(block_results[0])
            self.assertTrue(result0.result)
            self.assertEqual(result0.stdout, "dep1\n\nsrc")

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

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(depth=-1)

            self.assertEqual(len(block_results), 1)
            result0 = self._result(block_results[0])
            self.assertTrue(result0.result)
            self.assertEqual(result0.stdout, "dep2\n\ndep1\n\nsrc")

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

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(
                depth=-1,
                reverse_depens=True,
            )

            self.assertEqual(len(block_results), 1)
            result0 = self._result(block_results[0])
            self.assertTrue(result0.result)
            self.assertEqual(result0.stdout, "src\n\ndep1\n\ndep2")

    def test_eval_blocks_does_not_merge_when_depens_disabled(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_eval_no_merge.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "py", "print('dep')")
            dep.save()

            src = self._new_node(root, "Src", "py", "print('src')")
            src.add_dependency(dep.fnode)
            src.save()

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            block_results = loaded.eval_blocks(depth=-1)

            self.assertEqual(len(block_results), 1)
            result0 = self._result(block_results[0])
            self.assertTrue(result0.result)
            self.assertEqual(result0.stdout.strip(), "src")

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

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks(depth=-1)
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

            loaded = MdocNode(mdcroot=root, path=src.path, title="")
            loaded.load()
            with self.assertRaises(ValueError) as ctx:
                loaded.eval_blocks(depth=1)
            message = str(ctx.exception)
            self.assertIn("dependency cycle detected", message)
            self.assertIn(src.fnode, message)
            self.assertIn(dep.fnode, message)


if __name__ == "__main__":
    unittest.main()
