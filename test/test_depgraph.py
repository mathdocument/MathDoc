from mathdoc.depgraph import DepGraph
from mathdoc.mdocnode import DependencyItem
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


class TestDepGraph(unittest.TestCase):
    @staticmethod
    def _new_node(root: Path, title: str, srctype: str, content: str) -> MdocNode:
        (root / ".mdc").mkdir(parents=True, exist_ok=True)
        node = MdocNode.create(mdcroot=root, folder=str(root), title=title)
        node.blocks.append(SrcBlock(srctype=srctype, content=content, metadata={}))
        return node

    def test_dependency_items_expand_incrementally_from_root_node(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_incremental.") as tmp:
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

            graph = DepGraph(mdcroot=root, root_node=loaded)

            depth_1 = graph.dependency_items(depth=1)
            self.assertEqual(
                depth_1,
                [
                    DependencyItem(
                        depth=1,
                        fnode=dep1.fnode,
                        title="Dep1",
                        rel_path=dep1.path.resolve().relative_to(root.resolve()).as_posix(),
                    )
                ],
            )
            self.assertEqual(
                set(graph.nodes_by_fnode),
                {src.fnode, dep1.fnode},
            )

            depth_inf = graph.dependency_items(depth=-1)
            self.assertEqual(
                depth_inf,
                [
                    DependencyItem(
                        depth=1,
                        fnode=dep1.fnode,
                        title="Dep1",
                        rel_path=dep1.path.resolve().relative_to(root.resolve()).as_posix(),
                    ),
                    DependencyItem(
                        depth=2,
                        fnode=dep2.fnode,
                        title="Dep2",
                        rel_path=dep2.path.resolve().relative_to(root.resolve()).as_posix(),
                    ),
                ],
            )
            self.assertEqual(
                set(graph.nodes_by_fnode),
                {src.fnode, dep1.fnode, dep2.fnode},
            )

    def test_eval_blocks_supports_root_fnode_lazy_loading(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_root_fnode.") as tmp:
            root = Path(tmp)
            dep2 = self._new_node(root, "Dep2", "natl", "dep2")
            dep2.save()

            dep1 = self._new_node(root, "Dep1", "natl", "dep1")
            dep1.add_dependency(dep2.fnode)
            dep1.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep1.fnode)
            src.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            block_results = graph.eval_blocks(depth=-1)

            self.assertEqual(len(block_results), 1)
            self.assertEqual(block_results[0][0], "natl")
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout, "dep2\n\ndep1\n\nsrc")

    def test_scan_all_builds_global_graph(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_scan_all.") as tmp:
            root = Path(tmp)
            leaf = self._new_node(root, "Leaf", "natl", "leaf")
            leaf.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(leaf.fnode)
            src.save()

            other = self._new_node(root, "Other", "natl", "other")
            other.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            graph.scan_all()

            self.assertEqual(
                set(graph.nodes_by_fnode),
                {leaf.fnode, src.fnode, other.fnode},
            )
            self.assertEqual(graph.dep_graph[src.fnode], [leaf.fnode])
            self.assertEqual(graph.dep_graph[leaf.fnode], [])
            self.assertEqual(graph.dep_graph[other.fnode], [])

            items = graph.dependency_items(depth=-1)
            self.assertEqual(
                items,
                [
                    DependencyItem(
                        depth=1,
                        fnode=leaf.fnode,
                        title="Leaf",
                        rel_path=leaf.path.resolve().relative_to(root.resolve()).as_posix(),
                    )
                ],
            )


if __name__ == "__main__":
    unittest.main()
