import os
from mathdoc.mdocnode import MdocNode
from mathdoc.srcblock import SrcBlock
import tempfile
import unittest
from pathlib import Path
import sys

PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))


class TestMdocNode(unittest.TestCase):
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

    def test_create_relative_folder_is_anchored_to_mdcroot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_rel_folder_root.") as tmp_root:
            with tempfile.TemporaryDirectory(prefix="mdoc_node_rel_folder_cwd.") as tmp_cwd:
                root = Path(tmp_root)
                cwd = Path(tmp_cwd)
                old_cwd = Path.cwd()
                try:
                    os.chdir(cwd)
                    node = MdocNode.create(mdcroot=root, folder="notes", title="Anchored")
                finally:
                    os.chdir(old_cwd)

                self.assertEqual(node.path.parent, (root / "notes").resolve())
                self.assertTrue(node.path.name.endswith(".mdoc"))

    def test_create_rejects_folder_outside_mdcroot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdoc_node_outside_root.") as tmp:
            root = Path(tmp)
            with self.assertRaises(ValueError) as ctx:
                MdocNode.create(mdcroot=root, folder="../outside", title="Nope")
            self.assertIn("mdoc folder must be under mdoc root", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
