from mathdoc.depgraph import DepGraph
from mathdoc.depgraph import DependencyCycleError
from mathdoc.depgraph import DependencyItem
import mathdoc.depgraph.evaluate as depgraph_evaluate_module
from mathdoc.indcache import IndCache
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

    @staticmethod
    def _make_invalid(path: Path) -> None:
        text = path.read_text(encoding="utf-8")
        if not text.endswith("\n"):
            text += "\n"
        text += "@title: Duplicate Broken Title\n"
        path.write_text(text, encoding="utf-8")

    def test_from_ref_loads_root_graph(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_from_ref.") as tmp:
            root = Path(tmp)
            src = self._new_node(root, "Src", "natl", "src")
            src.save()

            cache = IndCache(root)
            cache.bootstrap_if_needed()

            graph, rel_path = DepGraph.from_ref(cache=cache, ref=src.fnode[:8], cwd=root)

            self.assertEqual(rel_path, src.path.resolve().relative_to(root.resolve()).as_posix())
            self.assertEqual(graph.get_root_node().fnode, src.fnode)

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
            self.assertEqual(set(graph.nodes_by_fnode), {src.fnode, dep1.fnode})

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

    def test_eval_blocks_runs_all_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_basic.") as tmp:
            root = Path(tmp)
            node = self._new_node(root, "Eval", "natl", "hello")
            node.blocks.append(SrcBlock(srctype="py", content="print('hi')", metadata={}))
            node.save()

            graph = DepGraph(mdcroot=root, root_fnode=node.fnode)
            block_results = graph.eval_blocks()

            self.assertEqual(len(block_results), 2)
            self.assertEqual(block_results[0][0], "natl")
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout, "hello")
            self.assertEqual(block_results[1][0], "py")
            self.assertTrue(block_results[1][1].result)
            self.assertEqual(block_results[1][1].stdout.strip(), "hi")

    def test_eval_blocks_loads_config_once(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_cfg_once.") as tmp:
            root = Path(tmp)
            node = self._new_node(root, "Eval", "natl", "hello")
            node.blocks.append(SrcBlock(srctype="py", content="print('hi')", metadata={}))
            node.save()

            graph = DepGraph(mdcroot=root, root_fnode=node.fnode)
            calls = 0
            original_load = depgraph_evaluate_module.load_config

            try:

                def counted_load(mdcroot: Path) -> dict[str, object]:
                    nonlocal calls
                    calls += 1
                    return original_load(mdcroot)

                depgraph_evaluate_module.load_config = counted_load
                graph.eval_blocks(depth=-1)
            finally:
                depgraph_evaluate_module.load_config = original_load

            self.assertEqual(calls, 1)

    def test_eval_blocks_requires_initialized_mdcroot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_need_root.") as tmp:
            root = Path(tmp)
            node = MdocNode.create(mdcroot=root, folder=str(root), title="Eval")
            node.blocks.append(SrcBlock(srctype="natl", content="hello", metadata={}))
            node.save()

            graph = DepGraph(mdcroot=root, root_fnode=node.fnode)
            with self.assertRaises(ValueError) as ctx:
                graph.eval_blocks()
            self.assertIn("invalid mdoc root (missing .mdc)", str(ctx.exception))

    def test_eval_blocks_merges_dependencies_with_default_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_depth1.") as tmp:
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
            block_results = graph.eval_blocks()

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout, "src\n\ndep1")

    def test_eval_blocks_merges_dependencies_with_unbounded_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_depth_inf.") as tmp:
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
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout, "src\n\ndep1\n\ndep2")

    def test_eval_blocks_respects_reverse_depens_override(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_reverse.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / ".mdc" / "config.toml").write_text(
                "[src.natl]\nreverse_depens = false\n",
                encoding="utf-8",
            )

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
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout, "dep2\n\ndep1\n\nsrc")

    def test_eval_blocks_does_not_merge_when_depens_disabled(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_no_merge.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "py", "print('dep')")
            dep.save()

            src = self._new_node(root, "Src", "py", "print('src')")
            src.add_dependency(dep.fnode)
            src.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            block_results = graph.eval_blocks(depth=-1)

            self.assertEqual(len(block_results), 1)
            self.assertTrue(block_results[0][1].result)
            self.assertEqual(block_results[0][1].stdout.strip(), "src")

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
            self.assertEqual(block_results[0][1].stdout, "src\n\ndep1\n\ndep2")

    def test_dependency_items_respect_default_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_dep_items_depth1.") as tmp:
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
            items = graph.dependency_items()

            self.assertEqual(
                items,
                [
                    DependencyItem(
                        depth=1,
                        fnode=dep1.fnode,
                        title="Dep1",
                        rel_path=dep1.path.resolve().relative_to(root.resolve()).as_posix(),
                    )
                ],
            )

    def test_dependency_items_expand_with_unbounded_depth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_dep_items_inf.") as tmp:
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
            items = graph.dependency_items(depth=-1)

            self.assertEqual(
                items,
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

    def test_leaf_dependency_items_only_return_reachable_leaves(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_leaf_items.") as tmp:
            root = Path(tmp)
            leaf_direct = self._new_node(root, "Leaf Direct", "natl", "leaf_direct")
            leaf_direct.save()

            leaf_shared = self._new_node(root, "Leaf Shared", "natl", "leaf_shared")
            leaf_shared.save()

            leaf_other = self._new_node(root, "Leaf Other", "natl", "leaf_other")
            leaf_other.save()

            mid1 = self._new_node(root, "Mid1", "natl", "mid1")
            mid1.add_dependency(leaf_shared.fnode)
            mid1.save()

            mid2 = self._new_node(root, "Mid2", "natl", "mid2")
            mid2.add_dependency(leaf_shared.fnode)
            mid2.add_dependency(leaf_other.fnode)
            mid2.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(mid1.fnode)
            src.add_dependency(leaf_direct.fnode)
            src.add_dependency(mid2.fnode)
            src.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            items = graph.leaf_dependency_items()

            self.assertEqual(
                items,
                [
                    DependencyItem(
                        depth=1,
                        fnode=leaf_direct.fnode,
                        title="Leaf Direct",
                        rel_path=leaf_direct.path.resolve().relative_to(root.resolve()).as_posix(),
                    ),
                    DependencyItem(
                        depth=2,
                        fnode=leaf_shared.fnode,
                        title="Leaf Shared",
                        rel_path=leaf_shared.path.resolve().relative_to(root.resolve()).as_posix(),
                    ),
                    DependencyItem(
                        depth=2,
                        fnode=leaf_other.fnode,
                        title="Leaf Other",
                        rel_path=leaf_other.path.resolve().relative_to(root.resolve()).as_posix(),
                    ),
                ],
            )

    def test_eval_blocks_raises_on_dependency_cycle(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_cycle.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "natl", "dep")
            dep.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            dep.add_dependency(src.fnode)
            dep.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            with self.assertRaises(DependencyCycleError) as ctx:
                graph.eval_blocks(depth=-1)
            self.assertIn("dependency cycle detected", str(ctx.exception))
            self.assertIn(src.fnode, ctx.exception.cycle)
            self.assertIn(dep.fnode, ctx.exception.cycle)

    def test_eval_blocks_raises_on_cycle_with_depth_boundary(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_eval_cycle_depth1.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "natl", "dep")
            dep.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            dep.add_dependency(src.fnode)
            dep.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            with self.assertRaises(DependencyCycleError) as ctx:
                graph.eval_blocks(depth=1)
            self.assertIn("dependency cycle detected", str(ctx.exception))
            self.assertIn(src.fnode, ctx.exception.cycle)
            self.assertIn(dep.fnode, ctx.exception.cycle)

    def test_direct_dependency_items_allow_cycle_repair(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_direct_cycle_repair.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Dep", "natl", "dep")
            dep.save()

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            dep.add_dependency(src.fnode)
            dep.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            items = graph.direct_dependency_items()

            self.assertEqual(
                items,
                [
                    DependencyItem(
                        depth=1,
                        fnode=dep.fnode,
                        title="Dep",
                        rel_path=dep.path.resolve().relative_to(root.resolve()).as_posix(),
                    )
                ],
            )

    def test_direct_dependency_mutation_uses_graph_api(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_mutation.") as tmp:
            root = Path(tmp)
            src = self._new_node(root, "Src", "natl", "src")
            src.save()

            dep1 = self._new_node(root, "Dep1", "natl", "dep1")
            dep1.save()

            dep2 = self._new_node(root, "Dep2", "natl", "dep2")
            dep2.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            added, skipped_existing, skipped_self = graph.add_direct_dependencies(
                [dep1.fnode, src.fnode, dep2.fnode]
            )

            self.assertEqual(added, [dep1.fnode, dep2.fnode])
            self.assertEqual(skipped_existing, [])
            self.assertEqual(skipped_self, [src.fnode])
            self.assertEqual(graph.direct_dependency_fnodes(), [dep1.fnode, dep2.fnode])

            added_again, skipped_existing_again, skipped_self_again = graph.add_direct_dependencies(
                [dep1.fnode]
            )
            self.assertEqual(added_again, [])
            self.assertEqual(skipped_existing_again, [dep1.fnode])
            self.assertEqual(skipped_self_again, [])

            removed = graph.remove_direct_dependencies([dep1.fnode, "missing", dep1.fnode])
            self.assertEqual(removed, [dep1.fnode])
            self.assertEqual(graph.direct_dependency_fnodes(), [dep2.fnode])

            reloaded = MdocNode(mdcroot=root, path=src.path, title="")
            reloaded.load()
            self.assertEqual(reloaded.depens, [dep2.fnode])

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

    def test_dependency_items_show_invalid_placeholder(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_invalid_placeholder.") as tmp:
            root = Path(tmp)
            dep = self._new_node(root, "Broken Dep", "natl", "dep")
            dep.save()
            self._make_invalid(dep.path)

            src = self._new_node(root, "Src", "natl", "src")
            src.add_dependency(dep.fnode)
            src.save()

            graph = DepGraph(mdcroot=root, root_fnode=src.fnode)
            items = graph.dependency_items()

            self.assertEqual(
                items,
                [
                    DependencyItem(
                        depth=1,
                        fnode=dep.fnode,
                        title="<invalid>",
                        rel_path=dep.path.resolve().relative_to(root.resolve()).as_posix(),
                    )
                ],
            )
            self.assertIn(dep.fnode, graph.invalid_fnodes)
            issue = graph.issue_for_fnode(dep.fnode)
            self.assertIsNotNone(issue)
            self.assertEqual(issue.kind, "invalid")

    def test_graph_check_report_collects_missing_invalid_and_cycles(self) -> None:
        with tempfile.TemporaryDirectory(prefix="depgraph_check_report.") as tmp:
            root = Path(tmp)
            bad = self._new_node(root, "Broken Node", "natl", "bad")
            bad.save()
            self._make_invalid(bad.path)

            a = self._new_node(root, "Cycle A", "natl", "a")
            a.save()

            b = self._new_node(root, "Cycle B", "natl", "b")
            b.save()

            a.add_dependency(b.fnode)
            a.save()
            b.add_dependency(a.fnode)
            b.save()

            src = self._new_node(root, "Source", "natl", "src")
            src.add_dependency("missing-target-001")
            src.add_dependency(bad.fnode)
            src.save()

            graph = DepGraph(mdcroot=root)
            report = graph.graph_check_report()

            self.assertEqual(report.nodes, 4)
            self.assertEqual(report.edges, 4)
            self.assertEqual(len(report.missing), 1)
            self.assertEqual(report.missing[0].fnode, "missing-target-001")
            self.assertEqual(len(report.invalid), 1)
            self.assertEqual(report.invalid[0].rel_path, bad.path.resolve().relative_to(root.resolve()).as_posix())
            self.assertEqual(len(report.cycles), 1)
            self.assertIn(a.fnode, report.cycles[0])
            self.assertIn(b.fnode, report.cycles[0])


if __name__ == "__main__":
    unittest.main()
