use std::fs;
use std::path::Path;

use rusqlite::OptionalExtension;

use mathdoc::indcache::IndCache;

fn setup(root: &Path) {
    fs::create_dir_all(root.join(".mdc")).unwrap();
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

// ── refresh / bootstrap ──────────────────────────────────────────────────────

#[test]
fn test_refresh_all_skips_nested_workspace_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let parent = dir.path().join("parent");
    let child = parent.join("child");
    setup(&parent);
    setup(&child);

    write(
        &parent.join("parent-card.mdoc"),
        "@fnode: parent-node\n@title: Parent Card\n",
    );
    write(
        &child.join("child-card.mdoc"),
        "@fnode: child-node\n@title: Child Card\n",
    );

    let mut cache = IndCache::open(parent).unwrap();
    cache.refresh_all().unwrap();

    assert_eq!(cache.search("Parent Card").unwrap().len(), 1);
    assert_eq!(cache.search("Child Card").unwrap().len(), 0);
}

#[test]
fn test_refresh_all_detects_subnanosecond_mtime_change() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let file_path = root.join("card.mdoc");
    write(&file_path, "@fnode: node-ns\n@title: OLD0\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    assert_eq!(cache.search("OLD0").unwrap().len(), 1);

    // Overwrite the file with different content
    write(&file_path, "@fnode: node-ns\n@title: NEW0\n");

    cache.refresh_all().unwrap();
    assert_eq!(cache.search("NEW0").unwrap().len(), 1);
    assert_eq!(cache.search("OLD0").unwrap().len(), 0);
}

#[test]
fn test_legacy_schema_is_migrated() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let mdc_dir = root.join(".mdc");
    fs::create_dir_all(&mdc_dir).unwrap();

    let file_path = root.join("legacy.mdoc");
    write(&file_path, "@fnode: legacy-node\n@title: Legacy Title\n");

    let db_path = mdc_dir.join("index.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE mdocs (
                fnode TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                title_lc TEXT NOT NULL,
                mtime_sec INTEGER NOT NULL,
                size INTEGER NOT NULL
            );
            CREATE INDEX idx_mdocs_title_lc ON mdocs(title_lc);",
        )
        .unwrap();
        let stat = fs::metadata(&file_path).unwrap();
        let mtime_sec = stat
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO mdocs (fnode, path, title, title_lc, mtime_sec, size) VALUES (?,?,?,?,?,?)",
            rusqlite::params![
                "legacy-node",
                "legacy.mdoc",
                "Legacy Title",
                "legacy title",
                mtime_sec,
                stat.len() as i64
            ],
        )
        .unwrap();
    }

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let rows = cache.search("Legacy Title").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "legacy-node");

    // Verify mtime_ns column was added
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let has_mtime_ns: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mdocs') WHERE name = 'mtime_ns'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    assert!(has_mtime_ns, "mtime_ns column should exist after migration");
}

// ── search / resolve ──────────────────────────────────────────────────────────

#[test]
fn test_search_and_resolve_surface_duplicate_fnodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("dup-a.mdoc"),
        "@fnode: dup-node\n@title: Dup A\n",
    );
    write(
        &root.join("dup-b.mdoc"),
        "@fnode: dup-node\n@title: Dup B\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let results = cache.search("dup-node").unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|(f, _, _)| f == "dup-node"));
    let titles: std::collections::HashSet<&str> =
        results.iter().map(|(_, t, _)| t.as_str()).collect();
    assert!(titles.contains("Dup A"));
    assert!(titles.contains("Dup B"));

    // Resolving should fail with "ambiguous"
    let err = cache.resolve_ref("dup-node", Some(root)).unwrap_err();
    assert!(
        err.to_string().contains("ambiguous"),
        "expected ambiguous error, got: {err}"
    );

    let dup_paths = cache.duplicate_fnode_paths("dup-node").unwrap();
    assert_eq!(dup_paths.len(), 2);
}

#[test]
fn test_knows_fnode_tracks_valid_and_issue_entries() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(&root.join("ok.mdoc"), "@fnode: ok-node\n@title: OK Node\n");
    write(
        &root.join("dup-a.mdoc"),
        "@fnode: dup-node\n@title: Dup Node\n",
    );
    write(
        &root.join("dup-b.mdoc"),
        "@fnode: dup-node\n@title: Dup Node\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    assert!(cache.knows_fnode("ok-node").unwrap());
    assert!(cache.knows_fnode("dup-node").unwrap());
    assert!(!cache.knows_fnode("missing-node").unwrap());
}

// ── graph queries ─────────────────────────────────────────────────────────────

#[test]
fn test_upsert_path_updates_cached_edges_and_missing_issues() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    let leaf_path = root.join("leaf.mdoc");
    let src_path = root.join("src.mdoc");
    write(&leaf_path, "@fnode: leaf-node\n@title: Leaf Card\n");
    write(
        &src_path,
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let referrers = cache.referrer_items("leaf-node", 1).unwrap();
    assert_eq!(
        referrers
            .iter()
            .map(|i| i.fnode.as_str())
            .collect::<Vec<_>>(),
        ["src-node"]
    );
    assert!(cache.graph_check_report().unwrap().missing.is_empty());

    // Change src to reference a missing target
    write(
        &src_path,
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nmissing-target-001\n@end\n",
    );
    cache.upsert_path(&src_path).unwrap();

    let report = cache.graph_check_report().unwrap();
    assert_eq!(
        report
            .missing
            .iter()
            .map(|i| i.fnode.as_str())
            .collect::<Vec<_>>(),
        ["missing-target-001"]
    );
    assert!(cache.referrer_items("leaf-node", 1).unwrap().is_empty());
}

#[test]
fn test_cached_graph_queries_cover_roots_refs_and_invalid() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("leaf.mdoc"),
        "@fnode: leaf-node\n@title: Leaf Card\n",
    );
    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );
    let bad_path = root.join("bad.mdoc");
    write(
        &bad_path,
        "@fnode: bad-node\n@title: Broken Card\n@title: Duplicate Broken Title\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let roots = cache.global_root_items().unwrap();
    assert_eq!(roots[0].fnode, "src-node");
    assert_eq!(roots[0].component_size, 2);
    assert_eq!(roots[1].fnode, "bad-node");
    assert_eq!(roots[1].title, "<invalid>");

    let refs = cache.referrer_items("leaf-node", 1).unwrap();
    assert_eq!(
        refs.iter().map(|i| i.fnode.as_str()).collect::<Vec<_>>(),
        ["src-node"]
    );

    let report = cache.graph_check_report().unwrap();
    assert_eq!(report.nodes, 3);
    assert_eq!(report.edges, 1);
    assert_eq!(report.invalid.len(), 1);
    assert_eq!(report.invalid[0].fnode, "bad-node");
}

#[test]
fn test_discover_workspace_changes_finds_external_duplicate_paths() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    let dep_path = root.join("dep.mdoc");
    write(&dep_path, "@fnode: dep-node\n@title: Dup Discovery Dep\n");
    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Dup Discovery Src\n\n@dep:\ndep-node\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let copies_dir = root.join("copies");
    fs::create_dir_all(&copies_dir).unwrap();
    let dup_path = copies_dir.join("dep-copy.mdoc");
    fs::copy(&dep_path, &dup_path).unwrap();

    cache.discover_workspace_changes().unwrap();
    let report = cache.dependency_report("src-node", -1).unwrap();

    assert_eq!(
        report
            .items
            .iter()
            .map(|i| i.fnode.as_str())
            .collect::<Vec<_>>(),
        ["dep-node"]
    );
    assert!(
        report.issues_by_fnode.contains_key("dep-node"),
        "dep-node should have an issue (duplicate)"
    );
    assert_eq!(
        report.issues_by_fnode["dep-node"].kind,
        mathdoc::core::IssueKind::Invalid
    );
}

// ── in_degree tracking ────────────────────────────────────────────────────────

#[test]
fn test_in_degree_increments_on_dep_add() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("leaf.mdoc"),
        "@fnode: leaf-node\n@title: Leaf Card\n",
    );
    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        let row: Option<i64> = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?",
                rusqlite::params!["leaf-node"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(row.is_none(), "leaf-node should have no in_degree yet");
    }

    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );
    cache.refresh_all().unwrap();

    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        let row: Option<i64> = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?",
                rusqlite::params!["leaf-node"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(row, Some(1));
    }
}

#[test]
fn test_in_degree_decrements_on_dep_remove() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("leaf.mdoc"),
        "@fnode: leaf-node\n@title: Leaf Card\n",
    );
    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        let in_degree: i64 = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?",
                rusqlite::params!["leaf-node"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_degree, 1);
    }

    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n",
    );
    cache.refresh_all().unwrap();

    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        let row: Option<i64> = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = ?",
                rusqlite::params!["leaf-node"],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(row.is_none(), "leaf-node in_degree should be absent (0)");
    }
}

// ── topo_depth migration / crash-safe backfill ────────────────────────────────

#[test]
fn test_migration_backfills_topo_depth() {
    // Simulate upgrading from a pre-v6 database that has edges but no topo_depth column.
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("parent.mdoc"),
        "@fnode: parent-node\n@title: Parent\n\n@dep:\nchild-node\n@end\n",
    );
    write(
        &root.join("child.mdoc"),
        "@fnode: child-node\n@title: Child\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    // Simulate a pre-v6 database: drop topo_depth column by downgrading user_version
    // and zeroing out depths, plus clear the backfilled flag.
    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 5;
             UPDATE mdocs SET topo_depth = 0;
             UPDATE mdoc_index_state SET topo_depth_backfilled = 0 WHERE id = 1;",
        )
        .unwrap();
    }

    // Re-open: migration detects needs_topo_backfill and backfills real depths.
    let cache2 = IndCache::open(root.to_path_buf()).unwrap();
    let depths = cache2.all_topo_depths().unwrap();
    assert_eq!(
        depths.get("parent-node").copied().unwrap_or(0),
        1,
        "parent-node should have topo_depth = 1 after backfill"
    );
    assert_eq!(
        depths.get("child-node").copied().unwrap_or(999),
        0,
        "child-node (leaf) should have topo_depth = 0"
    );
}

#[test]
fn test_crash_safe_topo_backfill() {
    // Simulate a crash window: schema is v8 (version already bumped), topo_depth
    // column exists, but topo_depth_backfilled flag was never set to 1.
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("parent.mdoc"),
        "@fnode: parent-node\n@title: Parent\n\n@dep:\nchild-node\n@end\n",
    );
    write(
        &root.join("child.mdoc"),
        "@fnode: child-node\n@title: Child\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    // Simulate the crash window: zero out depths and reset the flag.
    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        conn.execute_batch(
            "UPDATE mdocs SET topo_depth = 0;
             UPDATE mdoc_index_state SET topo_depth_backfilled = 0 WHERE id = 1;",
        )
        .unwrap();
    }

    // Re-open: sees topo_depth_backfilled = 0 even though user_version = 8, runs backfill.
    let cache2 = IndCache::open(root.to_path_buf()).unwrap();
    let depths = cache2.all_topo_depths().unwrap();
    assert_eq!(
        depths.get("parent-node").copied().unwrap_or(0),
        1,
        "parent-node should have topo_depth = 1 after crash-safe recovery"
    );
}

#[test]
fn test_schema_migration_backfills_in_degree() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );
    write(
        &root.join("leaf.mdoc"),
        "@fnode: leaf-node\n@title: Leaf Card\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    // Simulate v4 cache: clear in_degree and downgrade user_version
    {
        let conn = rusqlite::Connection::open(cache.db_path()).unwrap();
        conn.execute_batch("PRAGMA user_version = 4; DELETE FROM mdoc_in_degree;")
            .unwrap();
    }

    // Re-open triggers migration → backfills in_degree
    let mut cache2 = IndCache::open(root.to_path_buf()).unwrap();
    let roots = cache2.global_root_items().unwrap();

    let root_fnodes: Vec<&str> = roots.iter().map(|i| i.fnode.as_str()).collect();
    assert!(
        root_fnodes.contains(&"src-node"),
        "src-node should be a root"
    );
    assert!(
        !root_fnodes.contains(&"leaf-node"),
        "leaf-node should not be a root"
    );
}

// ── SCC cache invalidation ────────────────────────────────────────────────────

#[test]
fn test_graph_check_report_invalidates_scc_cache_on_fnode_rename() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("a.mdoc"),
        "@fnode: a-node\n@title: A Card\n\n@dep:\nb-node\n@end\n",
    );
    write(
        &root.join("b.mdoc"),
        "@fnode: b-node\n@title: B Card\n\n@dep:\na-node\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let first_report = cache.graph_check_report().unwrap();
    assert_eq!(first_report.cycles.len(), 1);

    // Rename a-node → c-node; cycle a-node ↔ b-node no longer exists
    write(
        &root.join("a.mdoc"),
        "@fnode: c-node\n@title: C Card\n\n@dep:\nb-node\n@end\n",
    );
    cache.refresh_all().unwrap();

    let second_report = cache.graph_check_report().unwrap();
    assert_eq!(
        second_report.cycles.len(),
        0,
        "SCC cache must be invalidated after fnode rename"
    );
}

// ── Dependency report ─────────────────────────────────────────────────────────

#[test]
fn test_dependency_reports_basic() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    write(
        &root.join("leaf.mdoc"),
        "@fnode: leaf-node\n@title: Leaf Card\n",
    );
    write(
        &root.join("src.mdoc"),
        "@fnode: src-node\n@title: Source Card\n\n@dep:\nleaf-node\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let report = cache.dependency_report("src-node", -1).unwrap();
    assert_eq!(
        report
            .items
            .iter()
            .map(|i| i.fnode.as_str())
            .collect::<Vec<_>>(),
        ["leaf-node"]
    );

    let leaf_report = cache.leaf_dependency_report("src-node").unwrap();
    assert_eq!(
        leaf_report
            .items
            .iter()
            .map(|i| i.fnode.as_str())
            .collect::<Vec<_>>(),
        ["leaf-node"]
    );
}
