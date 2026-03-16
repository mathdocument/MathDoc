import os
import sqlite3
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest import mock


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.indcache import IndCache
import mathdoc.indcache.cache as indcache_cache_module
import mathdoc.indcache.queries as indcache_queries_module


class TestIndCache(unittest.TestCase):
    def test_refresh_all_skips_nested_workspace_files(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_nested.") as tmp:
            parent = Path(tmp) / "parent"
            child = parent / "child"
            (parent / ".mdc").mkdir(parents=True, exist_ok=True)
            (child / ".mdc").mkdir(parents=True, exist_ok=True)

            (parent / "parent-card.mdoc").write_text(
                "@fnode: parent-node\n"
                "@title: Parent Card\n",
                encoding="utf-8",
            )
            (child / "child-card.mdoc").write_text(
                "@fnode: child-node\n"
                "@title: Child Card\n",
                encoding="utf-8",
            )

            cache = IndCache(parent)
            cache.refresh_all()

            self.assertEqual(len(cache.search("Parent Card")), 1)
            self.assertEqual(len(cache.search("Child Card")), 0)

    def test_refresh_all_detects_same_second_changes_with_mtime_ns(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_ns.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            file_path = root / "card.mdoc"
            file_path.write_text(
                "@fnode: node-ns\n"
                "@title: OLD0\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()
            self.assertEqual(len(cache.search("OLD0")), 1)

            old_stat = file_path.stat()
            old_mtime_ns = int(old_stat.st_mtime_ns)
            sec_base_ns = (old_mtime_ns // 1_000_000_000) * 1_000_000_000
            sub_ns = old_mtime_ns - sec_base_ns
            new_sub_ns = (sub_ns + 1) % 1_000_000_000
            if new_sub_ns == sub_ns:
                new_sub_ns = (sub_ns + 2) % 1_000_000_000
            new_mtime_ns = sec_base_ns + new_sub_ns

            file_path.write_text(
                "@fnode: node-ns\n"
                "@title: NEW0\n",
                encoding="utf-8",
            )
            os.utime(file_path, ns=(old_stat.st_atime_ns, new_mtime_ns))

            updated_stat = file_path.stat()
            if int(updated_stat.st_mtime_ns) == old_mtime_ns:
                self.skipTest("filesystem mtime precision is too coarse")
            if int(updated_stat.st_mtime_ns) // 1_000_000_000 != old_mtime_ns // 1_000_000_000:
                self.skipTest("failed to keep mtime in the same second")

            cache.refresh_all()
            self.assertEqual(len(cache.search("NEW0")), 1)
            self.assertEqual(len(cache.search("OLD0")), 0)

    def test_legacy_schema_is_migrated_to_mtime_ns(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_migrate.") as tmp:
            root = Path(tmp)
            mdc_dir = root / ".mdc"
            mdc_dir.mkdir(parents=True, exist_ok=True)

            file_path = root / "legacy.mdoc"
            file_path.write_text(
                "@fnode: legacy-node\n"
                "@title: Legacy Title\n",
                encoding="utf-8",
            )

            db_path = mdc_dir / "index.db"
            with closing(sqlite3.connect(db_path)) as conn:
                conn.execute(
                    """
                    CREATE TABLE mdocs (
                        fnode TEXT PRIMARY KEY,
                        path TEXT NOT NULL UNIQUE,
                        title TEXT NOT NULL,
                        title_lc TEXT NOT NULL,
                        mtime_sec INTEGER NOT NULL,
                        size INTEGER NOT NULL
                    )
                    """
                )
                conn.execute(
                    "CREATE INDEX idx_mdocs_title_lc ON mdocs(title_lc)"
                )
                stat = file_path.stat()
                conn.execute(
                    """
                    INSERT INTO mdocs (fnode, path, title, title_lc, mtime_sec, size)
                    VALUES (?, ?, ?, ?, ?, ?)
                    """,
                    (
                        "legacy-node",
                        "legacy.mdoc",
                        "Legacy Title",
                        "legacy title",
                        int(stat.st_mtime),
                        int(stat.st_size),
                    ),
                )
                conn.commit()

            cache = IndCache(root)
            cache.refresh_all()

            rows = cache.search("Legacy Title")
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0][0], "legacy-node")

            with closing(sqlite3.connect(db_path)) as conn:
                columns = {
                    str(row[1]) for row in conn.execute("PRAGMA table_info(mdocs)")
                }
                self.assertIn("mtime_ns", columns)
                row = conn.execute(
                    "SELECT mtime_ns FROM mdocs WHERE fnode = ?",
                    ("legacy-node",),
                ).fetchone()
            self.assertIsNotNone(row)
            self.assertGreater(int(row[0]), 0)

    def test_schema_version_pragma_follows_constant(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_schema_version.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "card.mdoc").write_text(
                "@fnode: node-version\n"
                "@title: Version Check\n",
                encoding="utf-8",
            )

            original_version = IndCache.SCHEMA_VERSION
            try:
                IndCache.SCHEMA_VERSION = 7
                cache = IndCache(root)
                cache.refresh_all()
            finally:
                IndCache.SCHEMA_VERSION = original_version

            with closing(sqlite3.connect(root / ".mdc" / "index.db")) as conn:
                row = conn.execute("PRAGMA user_version").fetchone()
            self.assertIsNotNone(row)
            self.assertEqual(int(row[0]), 7)

    def test_bootstrap_stabilizes_empty_repo(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_bootstrap_empty.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            cache = IndCache(root)
            cache.refresh_all()

            with cache._open_conn() as conn:
                self.assertFalse(cache._bootstrap_required(conn))

            with mock.patch(
                "mathdoc.indcache.cache.refresh_search_index",
                side_effect=AssertionError("bootstrap should already be stable"),
            ):
                cache.bootstrap_if_needed()

    def test_bootstrap_stabilizes_invalid_only_repo(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_bootstrap_invalid.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "broken.mdoc").write_text(
                "@title: Broken Only\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            self.assertEqual(cache.indexed_file_count(), 1)
            self.assertEqual(cache.count(), 0)

            with cache._open_conn() as conn:
                self.assertFalse(cache._bootstrap_required(conn))

            with mock.patch(
                "mathdoc.indcache.cache.refresh_search_index",
                side_effect=AssertionError("bootstrap should already be stable"),
            ):
                cache.bootstrap_if_needed()

    def test_cache_operations_close_sqlite_connections(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_close.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "card.mdoc").write_text(
                "@fnode: node-close\n"
                "@title: Close Check\n",
                encoding="utf-8",
            )

            original_connect = indcache_cache_module.sqlite3.connect
            closed_count = 0

            class TrackingConnection(sqlite3.Connection):
                def close(self) -> None:
                    nonlocal closed_count
                    closed_count += 1
                    super().close()

            def tracking_connect(*args: object, **kwargs: object) -> sqlite3.Connection:
                kwargs.setdefault("factory", TrackingConnection)
                return original_connect(*args, **kwargs)

            indcache_cache_module.sqlite3.connect = tracking_connect
            try:
                cache = IndCache(root)
                cache.count()
                cache.refresh_all()
                cache.search("close")
            finally:
                indcache_cache_module.sqlite3.connect = original_connect

            self.assertEqual(closed_count, 3)

    def test_search_and_resolve_surface_duplicate_fnodes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_dupe.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "dup-a.mdoc").write_text(
                "@fnode: dup-node\n"
                "@title: Dup A\n",
                encoding="utf-8",
            )
            (root / "dup-b.mdoc").write_text(
                "@fnode: dup-node\n"
                "@title: Dup B\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            self.assertEqual(
                cache.search("dup-node"),
                [
                    ("dup-node", "Dup A", "dup-a.mdoc"),
                    ("dup-node", "Dup B", "dup-b.mdoc"),
                ],
            )
            self.assertEqual(
                cache.search("Dup"),
                [
                    ("dup-node", "Dup A", "dup-a.mdoc"),
                    ("dup-node", "Dup B", "dup-b.mdoc"),
                ],
            )
            with self.assertRaises(ValueError) as ctx:
                cache.resolve_ref("dup-node", cwd=root)
            self.assertIn("ambiguous mdoc reference 'dup-node'", str(ctx.exception))
            self.assertEqual(
                cache.duplicate_fnode_paths("dup-node"),
                [(root / "dup-a.mdoc").resolve(), (root / "dup-b.mdoc").resolve()],
            )

    def test_upsert_path_updates_cached_edges_and_missing_issues(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_upsert_graph.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            src_path = root / "src.mdoc"
            leaf_path = root / "leaf.mdoc"
            leaf_path.write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )
            src_path.write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()
            self.assertEqual(
                [item.fnode for item in cache.referrer_items(target_fnode="leaf-node", depth=1)],
                ["src-node"],
            )
            self.assertEqual(cache.graph_check_report().missing, [])

            src_path.write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "missing-target-001\n"
                "@end\n",
                encoding="utf-8",
            )
            cache.upsert_path(src_path)

            report = cache.graph_check_report()
            self.assertEqual(
                [issue.fnode for issue in report.missing],
                ["missing-target-001"],
            )
            self.assertEqual(
                cache.referrer_items(target_fnode="leaf-node", depth=1),
                [],
            )

    def test_cached_graph_queries_cover_roots_refs_and_invalid(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_cached_graph.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            (root / "leaf.mdoc").write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )
            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )
            bad_path = root / "bad.mdoc"
            bad_path.write_text(
                "@fnode: bad-node\n"
                "@title: Broken Card\n"
                "@title: Duplicate Broken Title\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            roots = cache.global_root_items()
            self.assertEqual(roots[0].fnode, "src-node")
            self.assertEqual(roots[0].component_size, 2)
            self.assertEqual(roots[1].fnode, "bad-node")
            self.assertEqual(roots[1].title, "<invalid>")

            refs = cache.referrer_items(target_fnode="leaf-node", depth=1)
            self.assertEqual([item.fnode for item in refs], ["src-node"])

            report = cache.graph_check_report()
            self.assertEqual(report.nodes, 3)
            self.assertEqual(report.edges, 1)
            self.assertEqual(len(report.invalid), 1)
            self.assertEqual(report.invalid[0].fnode, "bad-node")

    def test_dependency_reports_do_not_use_full_graph_snapshots(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_targeted_dep_report.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            (root / "leaf.mdoc").write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )
            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )
            (root / "other.mdoc").write_text(
                "@fnode: other-node\n"
                "@title: Other Card\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            with (
                mock.patch.object(
                    indcache_queries_module,
                    "_dep_graph_snapshot",
                    side_effect=AssertionError(
                        "dependency report should not snapshot the whole graph"
                    ),
                ),
                mock.patch.object(
                    indcache_queries_module,
                    "_node_lookup",
                    side_effect=AssertionError(
                        "dependency report should not load all nodes"
                    ),
                ),
                mock.patch.object(
                    indcache_queries_module,
                    "_issue_lookup",
                    side_effect=AssertionError(
                        "dependency report should not load all issues"
                    ),
                ),
            ):
                report = cache.dependency_report(root_fnode="src-node", depth=-1)
                leaf_report = cache.leaf_dependency_report(root_fnode="src-node")

            self.assertEqual([item.fnode for item in report.items], ["leaf-node"])
            self.assertEqual([item.fnode for item in leaf_report.items], ["leaf-node"])

    def test_discover_workspace_changes_finds_external_duplicate_paths(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_discovery_duplicate.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            dep_path = root / "dep.mdoc"
            dep_path.write_text(
                "@fnode: dep-node\n"
                "@title: Dup Discovery Dep\n",
                encoding="utf-8",
            )
            src_path = root / "src.mdoc"
            src_path.write_text(
                "@fnode: src-node\n"
                "@title: Dup Discovery Src\n"
                "\n"
                "@dep:\n"
                "dep-node\n"
                "@end\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            duplicate_path = root / "copies" / "dep-copy.mdoc"
            duplicate_path.parent.mkdir(parents=True, exist_ok=True)
            duplicate_path.write_text(dep_path.read_text(encoding="utf-8"), encoding="utf-8")

            cache.discover_workspace_changes()
            report = cache.dependency_report(root_fnode="src-node", depth=-1)

            self.assertEqual([item.fnode for item in report.items], ["dep-node"])
            self.assertIn("dep-node", report.issues_by_fnode)
            self.assertEqual(report.issues_by_fnode["dep-node"].kind, "invalid")

    def test_discover_workspace_changes_skips_scandir_for_unchanged_dirs(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_discovery_cheap.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "nested").mkdir(parents=True, exist_ok=True)
            (root / "root.mdoc").write_text(
                "@fnode: root-node\n"
                "@title: Root Node\n",
                encoding="utf-8",
            )
            (root / "nested" / "child.mdoc").write_text(
                "@fnode: child-node\n"
                "@title: Child Node\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            with mock.patch(
                "mathdoc.indcache.discovery.os.scandir",
                side_effect=AssertionError(
                    "unchanged discovery should not enumerate directory entries"
                ),
            ):
                cache.discover_workspace_changes()

    def test_discover_workspace_changes_recurses_into_known_child_dirs(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_discovery_child.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "deps").mkdir(parents=True, exist_ok=True)

            dep_path = root / "deps" / "dep.mdoc"
            dep_path.write_text(
                "@fnode: dep-node\n"
                "@title: Child Dir Dep\n",
                encoding="utf-8",
            )
            src_path = root / "src.mdoc"
            src_path.write_text(
                "@fnode: src-node\n"
                "@title: Source Node\n"
                "\n"
                "@dep:\n"
                "dep-node\n"
                "@end\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            duplicate_path = root / "deps" / "dep-copy.mdoc"
            duplicate_path.write_text(dep_path.read_text(encoding="utf-8"), encoding="utf-8")

            cache.discover_workspace_changes()
            report = cache.dependency_report(root_fnode="src-node", depth=-1)

            self.assertIn("dep-node", report.issues_by_fnode)
            self.assertEqual(report.issues_by_fnode["dep-node"].kind, "invalid")

    def test_indcache_exposes_workspace_refresh_name(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_refresh_api.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)

            cache = IndCache(root)

            self.assertTrue(hasattr(cache, "refresh_workspace_index"))

    def test_knows_fnode_tracks_valid_and_issue_entries(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_knows_fnode.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "ok.mdoc").write_text(
                "@fnode: ok-node\n"
                "@title: OK Node\n",
                encoding="utf-8",
            )
            duplicate_text = (
                "@fnode: dup-node\n"
                "@title: Dup Node\n"
            )
            (root / "dup-a.mdoc").write_text(duplicate_text, encoding="utf-8")
            (root / "dup-b.mdoc").write_text(duplicate_text, encoding="utf-8")

            cache = IndCache(root)
            cache.refresh_all()

            self.assertTrue(cache.knows_fnode("ok-node"))
            self.assertTrue(cache.knows_fnode("dup-node"))
            self.assertFalse(cache.knows_fnode("missing-node"))


    def test_in_degree_increments_on_dep_add(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_in_degree_add.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "leaf.mdoc").write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )
            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            with closing(sqlite3.connect(cache.db_path)) as conn:
                row = conn.execute(
                    "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?", ("leaf-node",)
                ).fetchone()
            self.assertIsNone(row)  # no incoming edges yet

            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )
            cache.refresh_all()

            with closing(sqlite3.connect(cache.db_path)) as conn:
                row = conn.execute(
                    "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?", ("leaf-node",)
                ).fetchone()
            self.assertIsNotNone(row)
            self.assertEqual(int(row[0]), 1)

    def test_in_degree_decrements_on_dep_remove(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_in_degree_rm.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "leaf.mdoc").write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )
            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            with closing(sqlite3.connect(cache.db_path)) as conn:
                row = conn.execute(
                    "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?", ("leaf-node",)
                ).fetchone()
            self.assertIsNotNone(row)
            self.assertEqual(int(row[0]), 1)

            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n",
                encoding="utf-8",
            )
            cache.refresh_all()

            with closing(sqlite3.connect(cache.db_path)) as conn:
                row = conn.execute(
                    "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?", ("leaf-node",)
                ).fetchone()
            self.assertIsNone(row)  # in_degree 0 is represented by absence

    def test_graph_check_report_invalidates_scc_cache_on_fnode_rename(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_scc_invalidate.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "a.mdoc").write_text(
                "@fnode: a-node\n"
                "@title: A Card\n"
                "\n"
                "@dep:\n"
                "b-node\n"
                "@end\n",
                encoding="utf-8",
            )
            (root / "b.mdoc").write_text(
                "@fnode: b-node\n"
                "@title: B Card\n"
                "\n"
                "@dep:\n"
                "a-node\n"
                "@end\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            first_report = cache.graph_check_report()
            self.assertEqual(len(first_report.cycles), 1)

            # Rename a-node → c-node, keeping dep b-node (same edge set, different fnode)
            (root / "a.mdoc").write_text(
                "@fnode: c-node\n"
                "@title: C Card\n"
                "\n"
                "@dep:\n"
                "b-node\n"
                "@end\n",
                encoding="utf-8",
            )
            cache.refresh_all()

            second_report = cache.graph_check_report()
            # cycle a-node → b-node → a-node no longer exists; SCC cache must be invalidated
            self.assertEqual(len(second_report.cycles), 0)

    def test_schema_migration_backfills_in_degree(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_migration.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "src.mdoc").write_text(
                "@fnode: src-node\n"
                "@title: Source Card\n"
                "\n"
                "@dep:\n"
                "leaf-node\n"
                "@end\n",
                encoding="utf-8",
            )
            (root / "leaf.mdoc").write_text(
                "@fnode: leaf-node\n"
                "@title: Leaf Card\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            # Simulate a v4 cache: clear mdoc_in_degree and downgrade user_version
            with closing(sqlite3.connect(cache.db_path)) as conn:
                conn.execute("PRAGMA user_version = 4")
                conn.execute("DELETE FROM mdoc_in_degree")
                conn.commit()

            # Re-open triggers ensure_index_schema → backfill
            cache2 = IndCache(root)
            roots = cache2.global_root_items()

            root_fnodes = [item.fnode for item in roots]
            self.assertIn("src-node", root_fnodes)
            self.assertNotIn("leaf-node", root_fnodes)

    def test_graph_check_report_skips_scc_on_second_call(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_scc_cache.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "a.mdoc").write_text(
                "@fnode: a-node\n"
                "@title: A Card\n"
                "\n"
                "@dep:\n"
                "b-node\n"
                "@end\n",
                encoding="utf-8",
            )
            (root / "b.mdoc").write_text(
                "@fnode: b-node\n"
                "@title: B Card\n",
                encoding="utf-8",
            )

            cache = IndCache(root)
            cache.refresh_all()

            scc_call_count = 0
            original_scc = indcache_queries_module.strongly_connected_components

            def counting_scc(graph):
                nonlocal scc_call_count
                scc_call_count += 1
                return original_scc(graph)

            with mock.patch.object(
                indcache_queries_module,
                "strongly_connected_components",
                side_effect=counting_scc,
            ):
                cache.graph_check_report()
                cache.graph_check_report()

            self.assertEqual(scc_call_count, 1)


if __name__ == "__main__":
    unittest.main()
