import os
import sqlite3
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SRC_PATH = PROJECT_ROOT / "src"
if str(SRC_PATH) not in sys.path:
    sys.path.insert(0, str(SRC_PATH))

from mathdoc.indcache import IndCache
import mathdoc.indcache as indcache_module


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

    def test_cache_operations_close_sqlite_connections(self) -> None:
        with tempfile.TemporaryDirectory(prefix="mdc_indcache_close.") as tmp:
            root = Path(tmp)
            (root / ".mdc").mkdir(parents=True, exist_ok=True)
            (root / "card.mdoc").write_text(
                "@fnode: node-close\n"
                "@title: Close Check\n",
                encoding="utf-8",
            )

            original_connect = indcache_module.sqlite3.connect
            closed_count = 0

            class TrackingConnection(sqlite3.Connection):
                def close(self) -> None:
                    nonlocal closed_count
                    closed_count += 1
                    super().close()

            def tracking_connect(*args: object, **kwargs: object) -> sqlite3.Connection:
                kwargs.setdefault("factory", TrackingConnection)
                return original_connect(*args, **kwargs)

            indcache_module.sqlite3.connect = tracking_connect
            try:
                cache = IndCache(root)
                cache.count()
                cache.refresh_all()
                cache.search("close")
            finally:
                indcache_module.sqlite3.connect = original_connect

            self.assertEqual(closed_count, 3)


if __name__ == "__main__":
    unittest.main()
