use std::fs;
use std::path::Path;

use rusqlite::OptionalExtension;

use mathdoc::core::DependencyCandidatesEmpty;
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

fn index_path(cache: &IndCache) -> std::path::PathBuf {
    cache.root().join(".mdc/index.db")
}

fn indexed_fnode_count(cache: &IndCache, fnode: &str) -> usize {
    cache
        .search(fnode, usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|node| node.fnode == fnode)
        .count()
}

fn stored_missing_issue_count(cache: &IndCache) -> i64 {
    rusqlite::Connection::open(index_path(cache))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM mdoc_issues WHERE kind = 'missing'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn indexed_document_count(cache: &IndCache) -> i64 {
    rusqlite::Connection::open(index_path(cache))
        .unwrap()
        .query_row(
            "SELECT document_count FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn topo_depth(cache: &IndCache, fnode: &str) -> u32 {
    cache.node_summary(fnode).unwrap().depth
}

#[cfg(unix)]
#[test]
fn open_rejects_symlinked_control_directory_without_touching_target() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::TempDir::new().unwrap();
    let external = tempfile::TempDir::new().unwrap();
    let marker = external.path().join("marker");
    fs::write(&marker, b"unchanged").unwrap();
    symlink(external.path(), workspace.path().join(".mdc")).unwrap();

    assert!(IndCache::open(workspace.path().to_path_buf()).is_err());
    assert_eq!(fs::read(&marker).unwrap(), b"unchanged");
    assert!(!external.path().join("index.db").exists());
}

#[cfg(unix)]
#[test]
fn open_rejects_symlinked_index_without_touching_target() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::TempDir::new().unwrap();
    fs::create_dir(workspace.path().join(".mdc")).unwrap();
    let external = tempfile::TempDir::new().unwrap();
    let target = external.path().join("external.db");
    fs::write(&target, b"unchanged").unwrap();
    symlink(&target, workspace.path().join(".mdc/index.db")).unwrap();

    assert!(IndCache::open(workspace.path().to_path_buf()).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"unchanged");
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

    assert_eq!(cache.search("Parent Card", usize::MAX).unwrap().len(), 1);
    assert_eq!(cache.search("Child Card", usize::MAX).unwrap().len(), 0);
}

#[cfg(unix)]
#[test]
fn test_refresh_all_preserves_index_when_subtree_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let locked = root.join("locked");
    write(
        &locked.join("node.mdoc"),
        "@fnode: locked-node\n@title: Locked Node\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(indexed_fnode_count(&cache, "locked-node"), 1);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let result = cache.refresh_all();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err());
    assert_eq!(indexed_fnode_count(&cache, "locked-node"), 1);
}

#[cfg(unix)]
#[test]
fn focused_reconciliation_preserves_inaccessible_cached_claimants() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let locked = root.join("locked");
    write(
        &locked.join("node.mdoc"),
        "@fnode: guarded-node\n@title: Guarded\n",
    );
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let competing = root.join("competing.mdoc");
    write(&competing, "@fnode: guarded-node\n@title: Competing\n");

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let reconciliation = cache.reconcile_fnode_paths("guarded-node");
    let upsert = cache.upsert_path(&competing);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(reconciliation.is_err());
    assert!(upsert.is_err());
    assert_eq!(indexed_fnode_count(&cache, "guarded-node"), 1);
    assert_eq!(
        cache.node_summary("guarded-node").unwrap().rel_path,
        "locked/node.mdoc"
    );
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
    assert_eq!(cache.search("OLD0", usize::MAX).unwrap().len(), 1);

    // Overwrite the file with different content
    write(&file_path, "@fnode: node-ns\n@title: NEW0\n");

    cache.refresh_all().unwrap();
    assert_eq!(cache.search("NEW0", usize::MAX).unwrap().len(), 1);
    assert_eq!(cache.search("OLD0", usize::MAX).unwrap().len(), 0);
}

#[test]
fn index_accepts_file_timestamps_before_the_unix_epoch() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let path = root.join("historic.mdoc");
    write(&path, "@fnode: historic-node\n@title: Historic\n");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1)),
        )
        .unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.discover_workspace_changes().unwrap();

    assert_eq!(
        cache.node_summary("historic-node").unwrap().title,
        "Historic"
    );
}

#[test]
fn malformed_block_fallback_ignores_embedded_fake_headers() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("malformed-src.mdoc"),
        "@fnode: real-node\n@title: Real Node\n\n@src: \"unterminated\n@fnode: fake-src\n@title: Fake Src\n",
    );
    write(
        &root.join("malformed-dep.mdoc"),
        "@dep:\nnot a dependency\n@fnode: fake-dep\n@title: Fake Dep\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    assert_eq!(indexed_fnode_count(&cache, "real-node"), 1);
    assert!(cache.path_has_blocking_issue("malformed-src.mdoc").unwrap());
    assert_eq!(indexed_fnode_count(&cache, "fake-src"), 0);
    assert_eq!(indexed_fnode_count(&cache, "fake-dep"), 0);
    assert!(cache.search("Fake Src", usize::MAX).unwrap().is_empty());
    assert!(cache.search("Fake Dep", usize::MAX).unwrap().is_empty());

    let report = cache.graph_check_report().unwrap();
    assert!(report
        .invalid
        .iter()
        .any(|issue| issue.fnode == "real-node"));
    assert!(report
        .invalid
        .iter()
        .any(|issue| issue.fnode == "<unknown>"));
}

#[cfg(unix)]
#[test]
fn test_discovery_finds_new_file_when_directory_mtime_is_restored() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let original_mtime = fs::metadata(root).unwrap().modified().unwrap();

    write(
        &root.join("new.mdoc"),
        "@fnode: new-node\n@title: New Node\n",
    );
    fs::File::open(root)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    assert_eq!(
        fs::metadata(root).unwrap().modified().unwrap(),
        original_mtime
    );

    cache.discover_workspace_changes().unwrap();
    assert_eq!(cache.search("New Node", usize::MAX).unwrap().len(), 1);
}

fn rewrite_preserving_mtime_and_size(path: &Path, content: &str, atomic: bool) {
    let original = fs::metadata(path).unwrap();
    let modified = original.modified().unwrap();
    assert_eq!(original.len(), content.len() as u64);

    let target = if atomic {
        path.with_extension("replacement")
    } else {
        path.to_path_buf()
    };
    fs::write(&target, content).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&target).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    drop(file);
    if atomic {
        fs::rename(&target, path).unwrap();
    }

    let current = fs::metadata(path).unwrap();
    assert_eq!(current.len(), original.len());
    assert_eq!(current.modified().unwrap(), modified);
}

fn setup_preserved_metadata_change() -> (tempfile::TempDir, IndCache, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("dep-old.mdoc"),
        "@fnode: dep-old\n@title: Dep Old\n",
    );
    write(
        &root.join("dep-new.mdoc"),
        "@fnode: dep-new\n@title: Dep New\n\n@dep:\nroot-new\n@end\n",
    );
    let source = root.join("source.mdoc");
    write(
        &source,
        "@fnode: root-old\n@title: Title Old\n\n@dep:\ndep-old\n@end\n",
    );
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    (dir, cache, source)
}

fn assert_preserved_metadata_change_detected(cache: &mut IndCache) {
    assert!(cache.search("Title Old", usize::MAX).unwrap().is_empty());
    assert_eq!(cache.search("Title New", usize::MAX).unwrap().len(), 1);
    assert_eq!(indexed_fnode_count(cache, "root-old"), 0);
    assert_eq!(indexed_fnode_count(cache, "root-new"), 1);
    let edges: std::collections::HashSet<_> =
        cache.all_valid_edges().unwrap().into_iter().collect();
    assert!(edges.contains(&("root-new".to_string(), "dep-new".to_string())));
    assert!(edges.contains(&("dep-new".to_string(), "root-new".to_string())));
    assert!(!cache.graph_check_report().unwrap().cycles.is_empty());
}

fn assert_preserved_metadata_change_deferred(cache: &IndCache) {
    assert_eq!(cache.search("Title Old", usize::MAX).unwrap().len(), 1);
    assert!(cache.search("Title New", usize::MAX).unwrap().is_empty());
    assert_eq!(indexed_fnode_count(cache, "root-old"), 1);
    assert_eq!(indexed_fnode_count(cache, "root-new"), 0);
}

#[test]
fn test_refresh_all_reparses_same_mtime_same_size_content() {
    let (_dir, mut cache, source) = setup_preserved_metadata_change();
    rewrite_preserving_mtime_and_size(
        &source,
        "@fnode: root-new\n@title: Title New\n\n@dep:\ndep-new\n@end\n",
        false,
    );

    cache.refresh_all().unwrap();
    assert_preserved_metadata_change_detected(&mut cache);
}

#[test]
fn test_incremental_refresh_defers_in_place_same_metadata_edit() {
    let (_dir, mut cache, source) = setup_preserved_metadata_change();
    rewrite_preserving_mtime_and_size(
        &source,
        "@fnode: root-new\n@title: Title New\n\n@dep:\ndep-new\n@end\n",
        false,
    );

    cache.discover_workspace_changes().unwrap();
    assert_preserved_metadata_change_deferred(&cache);
    cache.refresh_all().unwrap();
    assert_preserved_metadata_change_detected(&mut cache);
}

#[test]
fn test_incremental_refresh_defers_atomic_same_metadata_replacement() {
    let (_dir, mut cache, source) = setup_preserved_metadata_change();
    rewrite_preserving_mtime_and_size(
        &source,
        "@fnode: root-new\n@title: Title New\n\n@dep:\ndep-new\n@end\n",
        true,
    );

    cache.discover_workspace_changes().unwrap();
    assert_preserved_metadata_change_deferred(&cache);
    cache.refresh_all().unwrap();
    assert_preserved_metadata_change_detected(&mut cache);
}

#[test]
fn test_legacy_schema_is_rebuilt() {
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
            CREATE INDEX idx_mdocs_title_lc ON mdocs(title_lc);
            PRAGMA user_version = 8;",
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

    let rows = cache.search("Legacy Title", usize::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fnode, "legacy-node");

    write(
        &root.join("legacy-copy.mdoc"),
        "@fnode: legacy-node\n@title: Legacy Copy\n",
    );
    cache.upsert_path(&root.join("legacy-copy.mdoc")).unwrap();

    assert_eq!(
        indexed_fnode_count(&cache, "legacy-node"),
        2,
        "the rebuilt fnode index must allow duplicates"
    );
    assert!(cache.path_has_blocking_issue("legacy.mdoc").unwrap());
    assert!(cache.path_has_blocking_issue("legacy-copy.mdoc").unwrap());

    // Verify the current columns and constraints were installed.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let has_topo_depth: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mdocs') WHERE name = 'topo_depth'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    assert!(
        has_topo_depth,
        "topo_depth column should exist after rebuild"
    );
    let primary_key: String = conn
        .query_row(
            "SELECT name FROM pragma_table_info('mdocs') WHERE pk = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(primary_key, "id");
    let fnode_is_unique: bool = conn
        .query_row(
            "SELECT EXISTS (
                 SELECT 1
                 FROM pragma_index_list('mdocs') AS indexes
                 WHERE indexes.[unique] = 1
                   AND (SELECT COUNT(*) FROM pragma_index_info(indexes.name)) = 1
                   AND (SELECT name FROM pragma_index_info(indexes.name) LIMIT 1) = 'fnode'
             )",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!fnode_is_unique);
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

    let results = cache.search("dup-node", usize::MAX).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|node| node.fnode == "dup-node"));
    let titles: std::collections::HashSet<&str> =
        results.iter().map(|node| node.title.as_str()).collect();
    assert!(titles.contains("Dup A"));
    assert!(titles.contains("Dup B"));

    // Resolving should fail with "ambiguous"
    let err = cache.resolve_ref("dup-node", Some(root)).unwrap_err();
    assert!(
        err.to_string().contains("ambiguous"),
        "expected ambiguous error, got: {err}"
    );

    let dup_paths = cache.reconcile_fnode_paths("dup-node").unwrap();
    assert_eq!(dup_paths.len(), 2);
}

#[test]
fn start_ref_does_not_mask_an_ambiguous_fnode_with_a_title() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(&root.join("dup-a.mdoc"), "@fnode: token\n@title: A\n");
    write(&root.join("dup-b.mdoc"), "@fnode: token\n@title: B\n");
    write(
        &root.join("titled.mdoc"),
        "@fnode: titled-node\n@title: token\n",
    );

    let cache = IndCache::open(root.to_path_buf()).unwrap();
    let error = cache.resolve_start_ref("token", Some(root)).unwrap_err();

    assert!(error.to_string().contains("ambiguous mdoc reference"));
}

#[test]
fn test_search_treats_like_metacharacters_literally() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    for (path, fnode, title) in [
        ("percent.mdoc", "percent-node", "Literal 100% Result"),
        ("underscore.mdoc", "underscore-node", "Literal_under Score"),
        ("slash.mdoc", "slash-node", "Literal\\Path"),
        ("quote.mdoc", "quote-node", "Literal a\"b Phrase"),
        ("plain.mdoc", "plain-node", "Plain Result"),
    ] {
        write(
            &root.join(path),
            &format!("@fnode: {fnode}\n@title: {title}\n"),
        );
    }

    let cache = IndCache::open(root.to_path_buf()).unwrap();

    assert_eq!(
        cache.search("%", usize::MAX).unwrap()[0].fnode,
        "percent-node"
    );
    assert_eq!(cache.search("%", usize::MAX).unwrap().len(), 1);
    assert_eq!(
        cache.search("_", usize::MAX).unwrap()[0].fnode,
        "underscore-node"
    );
    assert_eq!(cache.search("_", usize::MAX).unwrap().len(), 1);
    assert_eq!(
        cache.search("\\", usize::MAX).unwrap()[0].fnode,
        "slash-node"
    );
    assert_eq!(cache.search("\\", usize::MAX).unwrap().len(), 1);
    for (query, expected) in [
        ("100%", "percent-node"),
        ("al_under", "underscore-node"),
        ("ral\\pa", "slash-node"),
        ("a\"b", "quote-node"),
    ] {
        let results = cache.search(query, usize::MAX).unwrap();
        assert_eq!(results.len(), 1, "unexpected results for {query:?}");
        assert_eq!(results[0].fnode, expected);
    }
}

#[test]
fn full_refresh_preserves_unicode_case_insensitive_title_search() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("unicode.mdoc"),
        "@fnode: unicode-title-node\n@title: École\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        cache.search("école", 10).unwrap()[0].fnode,
        "unicode-title-node"
    );

    cache.refresh_all().unwrap();
    assert_eq!(
        cache.search("école", 10).unwrap()[0].fnode,
        "unicode-title-node"
    );
}

#[test]
fn full_refresh_crosses_bulk_insert_boundaries() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    for index in 0..201 {
        write(
            &root.join(format!("node-{index:03}.mdoc")),
            &format!(
                "@fnode: node-{index:03}\n@title: Node {index:03}\n\n@dep:\nmissing-{index:03}\n@end\n"
            ),
        );
    }

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let report = cache.graph_check_report().unwrap();

    assert_eq!(report.nodes, 201);
    assert_eq!(report.edges, 201);
    assert_eq!(report.missing.len(), 201);
    assert_eq!(stored_missing_issue_count(&cache), 0);
    assert_eq!(indexed_document_count(&cache), 201);
}

#[test]
fn incremental_upsert_batches_large_ordered_dependency_lists() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    write(&source, "@fnode: source-node\n@title: Source\n");
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();

    let dependencies: Vec<_> = (0..405).map(|index| format!("dep-{index:03}")).collect();
    write(
        &source,
        &format!(
            "@fnode: source-node\n@title: Source\n\n@dep:\n{}\n@end\n",
            dependencies.join("\n")
        ),
    );
    cache.upsert_path(&source).unwrap();

    let connection = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let rows: Vec<(String, i64)> = connection
        .prepare(
            "SELECT dst_fnode, ord
             FROM mdoc_valid_edges
             WHERE src_path = 'source.mdoc'
             ORDER BY ord",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows.len(), dependencies.len());
    for (order, ((actual, stored_order), expected)) in rows.iter().zip(&dependencies).enumerate() {
        assert_eq!(actual, expected);
        assert_eq!(*stored_order, order as i64);
    }
    drop(connection);

    let peer = root.join("peer.mdoc");
    write(
        &peer,
        "@fnode: peer-node\n@title: Peer\n\n@dep:\ndep-000\n@end\n",
    );
    cache.upsert_path(&peer).unwrap();
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\nreplacement-dep\n@end\n",
    );
    cache.upsert_path(&source).unwrap();
    let connection = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let symbols: Vec<String> = connection
        .prepare("SELECT fnode FROM mdoc_symbols ORDER BY fnode")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        symbols,
        ["dep-000", "peer-node", "replacement-dep", "source-node"]
    );
    drop(connection);

    fs::remove_file(peer).unwrap();
    cache.discover_workspace_changes().unwrap();
    let connection = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let symbols: Vec<String> = connection
        .prepare("SELECT fnode FROM mdoc_symbols ORDER BY fnode")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(symbols, ["replacement-dep", "source-node"]);
}

#[test]
fn discovery_rejects_a_hard_link_added_after_indexing() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let path = root.join("node.mdoc");
    write(&path, "@fnode: hard-link-node\n@title: Hard Link\n");
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    std::fs::hard_link(&path, root.join("alias.bin")).unwrap();

    let error = cache.discover_workspace_changes().unwrap_err();

    assert!(error.to_string().contains("hard-linked file"));
}

#[test]
fn full_refresh_does_not_duplicate_placeholder_fnodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    for path in ["first.mdoc", "second.mdoc"] {
        write(
            &root.join(path),
            "@fnode: <invalid>\n@title: Invalid\n@unknown: value\n",
        );
    }

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let report = cache.graph_check_report().unwrap();

    assert_eq!(report.invalid.len(), 2);
    assert!(report
        .invalid
        .iter()
        .all(|issue| !issue.error.contains("duplicate fnode")));
}

#[test]
fn search_returns_ranked_summaries_and_all_nodes_are_unbounded() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("a-fnode-match.mdoc"),
        "@fnode: needle-node\n@title: Zeta\n",
    );
    write(
        &root.join("b-title-match.mdoc"),
        "@fnode: other-node\n@title: Needle\n",
    );
    write(
        &root.join("c-invalid.mdoc"),
        "@fnode: invalid-node\n@title: Invalid\n@unknown: value\n",
    );

    let cache = IndCache::open(root.to_path_buf()).unwrap();

    let results = cache.search("needle", 1).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].fnode, "needle-node");
    assert_eq!(results[0].rel_path, "a-fnode-match.mdoc");
    assert!(!results[0].broken);

    let all = cache.all_node_summaries().unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter()
            .map(|node| node.rel_path.as_str())
            .collect::<Vec<_>>(),
        ["a-fnode-match.mdoc", "b-title-match.mdoc", "c-invalid.mdoc"]
    );
    assert!(
        all.iter()
            .find(|node| node.fnode == "invalid-node")
            .unwrap()
            .broken
    );
}

#[test]
fn dependency_candidates_filter_before_limit_and_classify_empty_results() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    let mut dependency_body =
        String::from("@fnode: source-node\n@title: Candidate Excluded Source\n\n@dep:\n");
    for index in 0..25 {
        let fnode = format!("excluded-{index:02}");
        write(
            &root.join(format!("excluded-{index:02}.mdoc")),
            &format!("@fnode: {fnode}\n@title: Candidate Excluded {index:02}\n"),
        );
        dependency_body.push_str(&fnode);
        dependency_body.push('\n');
    }
    dependency_body.push_str("@end\n");
    write(&root.join("source.mdoc"), &dependency_body);

    write(
        &root.join("invalid.mdoc"),
        "@fnode: invalid-node\n@title: Candidate Excluded Invalid\n@title: Duplicate Title\n",
    );
    write(
        &root.join("duplicate-a.mdoc"),
        "@fnode: duplicate-node\n@title: Candidate Excluded Duplicate A\n",
    );
    write(
        &root.join("duplicate-b.mdoc"),
        "@fnode: duplicate-node\n@title: Candidate Excluded Duplicate B\n",
    );
    write(
        &root.join("valid.mdoc"),
        "@fnode: valid-node\n@title: Candidate Valid\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    let report = cache
        .dependency_candidates("source-node", "Candidate", 1)
        .unwrap();
    assert_eq!(report.empty, None);
    assert_eq!(report.nodes.len(), 1);
    assert_eq!(report.nodes[0].fnode, "valid-node");
    assert!(!report.nodes[0].broken);

    let source = cache
        .dependency_candidates("source-node", "Excluded Source", 1)
        .unwrap();
    assert!(source.nodes.is_empty());
    assert_eq!(
        source.empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 1,
            existing_dependencies: 0,
            invalid_or_duplicate: 0,
        })
    );

    let existing = cache
        .dependency_candidates("source-node", "Excluded 00", 1)
        .unwrap();
    assert!(existing.nodes.is_empty());
    assert_eq!(
        existing.empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 0,
            existing_dependencies: 1,
            invalid_or_duplicate: 0,
        })
    );

    let invalid = cache
        .dependency_candidates("source-node", "Excluded Invalid", 1)
        .unwrap();
    assert!(invalid.nodes.is_empty());
    assert_eq!(
        invalid.empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 0,
            existing_dependencies: 0,
            invalid_or_duplicate: 1,
        })
    );

    let duplicate = cache
        .dependency_candidates("source-node", "Excluded Duplicate", 1)
        .unwrap();
    assert!(duplicate.nodes.is_empty());
    assert_eq!(
        duplicate.empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 0,
            existing_dependencies: 0,
            invalid_or_duplicate: 2,
        })
    );

    let mixed = cache
        .dependency_candidates("source-node", "Candidate Excluded", 1)
        .unwrap();
    assert!(mixed.nodes.is_empty());
    assert_eq!(
        mixed.empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 1,
            existing_dependencies: 25,
            invalid_or_duplicate: 3,
        })
    );

    let absent = cache
        .dependency_candidates("source-node", "No Such Candidate", 1)
        .unwrap();
    assert!(absent.nodes.is_empty());
    assert_eq!(absent.empty, Some(DependencyCandidatesEmpty::NoMatch));

    let limited = cache
        .dependency_candidates("source-node", "Candidate Valid", 0)
        .unwrap();
    assert!(limited.nodes.is_empty());
    assert_eq!(
        limited.empty,
        Some(DependencyCandidatesEmpty::ResultLimit { available: 1 })
    );
}

#[test]
fn single_reference_lookups_ignore_unrelated_corrupt_rows() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("target.mdoc"),
        "@fnode: target-node\n@title: Target\n",
    );
    write(
        &root.join("source.mdoc"),
        "@fnode: source-node\n@title: Source\n\n@dep:\nmissing-node\n@end\n",
    );

    let cache = IndCache::open(root.to_path_buf()).unwrap();
    let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
    conn.execute_batch(
        "INSERT INTO mdocs (path, fnode, title, title_lc, topo_depth)
             VALUES ('corrupt.mdoc', 'corrupt-node', X'00', 'corrupt', 0);
         INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
             VALUES ('unrelated.mdoc', 'invalid', 'unrelated-node', X'00');",
    )
    .unwrap();
    drop(conn);

    let issue = cache.issue_for_fnode("missing-node").unwrap().unwrap();
    assert_eq!(issue.kind, mathdoc::core::IssueKind::Missing);
    assert_eq!(issue.fnode, "missing-node");

    let target = cache.ref_item_for_fnode("target-node", 7).unwrap();
    assert_eq!(target.title, "Target");
    assert_eq!(target.depth, 7);
    let missing = cache.ref_item_for_fnode("missing-node", 1).unwrap();
    assert_eq!(missing.title, "<missing>");
}

#[test]
fn test_resolve_ref_supports_suffixless_paths() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("theorem.mdoc"),
        "@fnode: root-theorem\n@title: Root Theorem\n",
    );
    write(
        &root.join("notes/lemma.mdoc"),
        "@fnode: nested-lemma\n@title: Nested Lemma\n",
    );

    let cache = IndCache::open(root.to_path_buf()).unwrap();

    assert_eq!(
        cache.resolve_ref("theorem", Some(root)).unwrap().0,
        "root-theorem"
    );
    assert_eq!(
        cache.resolve_ref("notes/lemma", Some(root)).unwrap().0,
        "nested-lemma"
    );
}

#[test]
fn test_resolve_ref_ignores_crafted_cache_path_without_reading_outside() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir(&root).unwrap();
    setup(&root);
    let outside = dir.path().join("outside.mdoc");
    write(&outside, "@fnode: outside-node\n@title: Outside\n");

    let mut cache = IndCache::open(root.clone()).unwrap();
    cache.refresh_all().unwrap();
    let db_path = index_path(&cache);
    drop(cache);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO mdocs
         (path, fnode, title, title_lc, topo_depth)
         VALUES ('../outside.mdoc', 'crafted-node', 'Crafted', 'crafted', 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let mut cache = IndCache::open(root).unwrap();
    assert!(cache.resolve_ref("crafted-node", None).is_err());
    assert_eq!(indexed_fnode_count(&cache, "crafted-node"), 1);
    cache.discover_workspace_changes().unwrap();
    assert_eq!(indexed_fnode_count(&cache, "crafted-node"), 0);
    assert_eq!(
        fs::read_to_string(outside).unwrap(),
        "@fnode: outside-node\n@title: Outside\n"
    );
}

#[cfg(unix)]
#[test]
fn test_fnode_reconciliation_prunes_cached_parent_symlink_without_reading_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir(&root).unwrap();
    setup(&root);
    let notes = root.join("notes");
    fs::create_dir(&notes).unwrap();
    write(
        &notes.join("node.mdoc"),
        "@fnode: cached-node\n@title: Cached\n",
    );

    let mut cache = IndCache::open(root.clone()).unwrap();
    cache.refresh_all().unwrap();
    drop(cache);

    fs::remove_dir_all(&notes).unwrap();
    let external = tempfile::TempDir::new().unwrap();
    let target = external.path().join("node.mdoc");
    write(&target, "@fnode: cached-node\n@title: External Target\n");
    symlink(external.path(), &notes).unwrap();

    let mut cache = IndCache::open(root).unwrap();
    assert!(cache
        .reconcile_fnode_paths("cached-node")
        .unwrap()
        .is_empty());
    assert_eq!(indexed_fnode_count(&cache, "cached-node"), 0);
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "@fnode: cached-node\n@title: External Target\n"
    );
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

    assert!(cache.referrer_items("leaf-node", 0).unwrap().is_empty());
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
    let missing = cache.node_summary("missing-target-001").unwrap();
    assert!(missing.broken);
    assert_eq!(missing.title, "<missing>");
    assert_eq!(
        cache.direct_dependency_summaries("src-node").unwrap(),
        vec![missing]
    );
    assert_eq!(stored_missing_issue_count(&cache), 0);
}

#[test]
fn set_based_referrer_traversal_preserves_depth_order_and_cycles() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(&root.join("leaf.mdoc"), "@fnode: leaf\n@title: Leaf\n");
    write(
        &root.join("a.mdoc"),
        "@fnode: a\n@title: A\n\n@dep:\nleaf\nc\n@end\n",
    );
    write(
        &root.join("b.mdoc"),
        "@fnode: b\n@title: B\n\n@dep:\nleaf\n@end\n",
    );
    write(
        &root.join("c.mdoc"),
        "@fnode: c\n@title: C\n\n@dep:\na\n@end\n",
    );
    write(
        &root.join("d.mdoc"),
        "@fnode: d\n@title: D\n\n@dep:\nb\n@end\n",
    );
    let cache = IndCache::open(root.to_path_buf()).unwrap();

    let refs = |depth| {
        cache
            .referrer_items("leaf", depth)
            .unwrap()
            .into_iter()
            .map(|item| (item.fnode, item.depth))
            .collect::<Vec<_>>()
    };
    assert_eq!(refs(1), [("a".into(), 1), ("b".into(), 1)]);
    assert_eq!(
        refs(2),
        [
            ("a".into(), 1),
            ("b".into(), 1),
            ("c".into(), 2),
            ("d".into(), 2),
        ]
    );
    assert_eq!(refs(-1), refs(2));
    assert!(cache.is_reachable("c", "leaf").unwrap());
    assert!(!cache.is_reachable("leaf", "c").unwrap());
    assert!(cache.is_reachable("a", "a").unwrap());
}

#[test]
fn partial_identity_issue_does_not_poison_a_valid_node_with_the_same_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("valid.mdoc"),
        "@fnode: shared-node\n@title: Valid Node\n",
    );
    write(&root.join("broken.mdoc"), "@fnode: shared-node\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();

    let summary = cache.node_summary("shared-node").unwrap();
    assert!(!summary.broken);
    assert_eq!(summary.title, "Valid Node");
    assert!(cache.issue_for_fnode("shared-node").unwrap().is_none());
    assert!(cache
        .dependency_report("shared-node", -1)
        .unwrap()
        .issues_by_fnode
        .is_empty());
    assert_eq!(cache.node_degrees("shared-node").unwrap().in_degree, 0);
    assert!(cache
        .graph_check_report()
        .unwrap()
        .invalid
        .iter()
        .any(|issue| issue.rel_path == "broken.mdoc"));
}

#[test]
fn unblocked_duplicate_claimant_exposes_derived_missing_issue() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let duplicate = root.join("duplicate.mdoc");
    write(
        &root.join("source.mdoc"),
        "@fnode: shared-node\n@title: Source\n\n@dep:\nmissing-node\n@end\n",
    );
    write(&duplicate, "@fnode: shared-node\n@title: Duplicate\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert!(cache.graph_check_report().unwrap().missing.is_empty());

    fs::remove_file(&duplicate).unwrap();
    cache.upsert_path(&duplicate).unwrap();

    let report = cache.graph_check_report().unwrap();
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].fnode, "missing-node");
    assert!(cache
        .dependency_report("shared-node", -1)
        .unwrap()
        .issues_by_fnode
        .contains_key("missing-node"));
}

#[test]
fn source_blocking_transitions_refresh_derived_missing_issues() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    let duplicate = root.join("duplicate.mdoc");
    write(
        &source,
        "@fnode: shared-node\n@title: Source\n\n@dep:\nmissing-node\n@end\n",
    );
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(cache.graph_check_report().unwrap().missing.len(), 1);

    write(&duplicate, "@fnode: shared-node\n@title: Duplicate\n");
    cache.upsert_path(&duplicate).unwrap();
    assert!(cache.graph_check_report().unwrap().missing.is_empty());

    fs::remove_file(&duplicate).unwrap();
    cache.discover_workspace_changes().unwrap();
    assert_eq!(cache.graph_check_report().unwrap().missing.len(), 1);

    write(
        &source,
        "@fnode: shared-node\n@title: Source\n@title: Invalid\n\n@dep:\nmissing-node\n@end\n",
    );
    cache.upsert_path(&source).unwrap();
    assert!(cache.graph_check_report().unwrap().missing.is_empty());
    assert_eq!(stored_missing_issue_count(&cache), 0);
}

#[test]
fn graph_roots_deduplicate_multiple_blocking_issues_for_one_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("valid.mdoc"),
        "@fnode: shared-node\n@title: Valid\n",
    );
    write(
        &root.join("invalid.mdoc"),
        "@fnode: shared-node\n@title: Invalid\n@title: Again\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let roots = cache.global_root_items().unwrap();

    assert_eq!(
        roots
            .iter()
            .filter(|item| item.rel_path == "invalid.mdoc")
            .count(),
        1
    );
}

#[test]
fn unchanged_fnode_upsert_refreshes_duplicate_issues_once() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let first = root.join("dup-a.mdoc");
    write(&first, "@fnode: duplicate-node\n@title: Duplicate A\n");
    write(
        &root.join("dup-b.mdoc"),
        "@fnode: duplicate-node\n@title: Duplicate B\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
    conn.execute_batch(
        "CREATE TABLE duplicate_issue_refresh_log (path TEXT NOT NULL);
         CREATE TRIGGER log_duplicate_issue_refresh
         AFTER INSERT ON mdoc_issues
         WHEN NEW.kind = 'duplicate'
         BEGIN
             INSERT INTO duplicate_issue_refresh_log (path) VALUES (NEW.path);
         END;",
    )
    .unwrap();
    drop(conn);

    cache.upsert_path(&first).unwrap();

    let refreshes: i64 = rusqlite::Connection::open(index_path(&cache))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM duplicate_issue_refresh_log",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refreshes, 2, "each duplicate path should be inserted once");
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

    let source = cache.node_summary("src-node").unwrap();
    assert_eq!(source.depth, 1);
    assert!(!source.broken);

    let referrer_summaries = cache.direct_referrer_summaries("leaf-node").unwrap();
    assert_eq!(referrer_summaries, vec![source]);

    let dependency_summaries = cache.direct_dependency_summaries("src-node").unwrap();
    assert_eq!(dependency_summaries.len(), 1);
    assert_eq!(dependency_summaries[0].fnode, "leaf-node");
    assert_eq!(dependency_summaries[0].depth, 0);
    assert!(!dependency_summaries[0].broken);

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

#[test]
fn test_discover_workspace_changes_finds_in_place_content_edit() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let path = root.join("edited.mdoc");
    write(&path, "@fnode: edited-node\n@title: Before Edit\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    assert_eq!(cache.search("Before Edit", usize::MAX).unwrap().len(), 1);

    // Replacing file contents does not normally change the parent directory mtime.
    write(&path, "@fnode: edited-node\n@title: After External Edit\n");
    cache.discover_workspace_changes().unwrap();

    assert!(cache.search("Before Edit", usize::MAX).unwrap().is_empty());
    assert_eq!(
        cache
            .search("After External Edit", usize::MAX)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_in_place_fnode_rename_refreshes_old_referrers() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let leaf_path = root.join("leaf.mdoc");
    let dep_path = root.join("dep.mdoc");
    let root_path = root.join("root.mdoc");
    write(&leaf_path, "@fnode: leaf-node\n@title: Leaf\n");
    write(
        &dep_path,
        "@fnode: old-dep\n@title: Dep\n\n@dep:\nleaf-node\n@end\n",
    );
    write(
        &root_path,
        "@fnode: root-node\n@title: Root\n\n@dep:\nold-dep\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    assert_eq!(topo_depth(&cache, "root-node"), 2);

    write(
        &dep_path,
        "@fnode: new-dep\n@title: Dep\n\n@dep:\nleaf-node\n@end\n",
    );
    cache.discover_workspace_changes().unwrap();

    // The old dependency token is now missing and therefore contributes one level.
    assert_eq!(topo_depth(&cache, "root-node"), 1);
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
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
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
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
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
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
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
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
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

// ── topo_depth rebuild / crash-safe backfill ──────────────────────────────────

#[test]
fn test_incremental_topo_depth_converges_across_short_and_long_paths() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    let d_path = root.join("d.mdoc");
    write(
        &root.join("a.mdoc"),
        "@fnode: a-node\n@title: A\n\n@dep:\nd-node\nb-node\n@end\n",
    );
    write(
        &root.join("b.mdoc"),
        "@fnode: b-node\n@title: B\n\n@dep:\nd-node\n@end\n",
    );
    write(&d_path, "@fnode: d-node\n@title: D\n");
    write(&root.join("e.mdoc"), "@fnode: e-node\n@title: E\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    write(
        &d_path,
        "@fnode: d-node\n@title: D\n\n@dep:\ne-node\n@end\n",
    );
    cache.upsert_path(&d_path.canonicalize().unwrap()).unwrap();

    assert_eq!(topo_depth(&cache, "d-node"), 1);
    assert_eq!(topo_depth(&cache, "b-node"), 2);
    assert_eq!(topo_depth(&cache, "a-node"), 3);
}

#[test]
fn batched_topo_refresh_merges_overlapping_ancestor_sets() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let a_path = root.join("a.mdoc");
    let b_path = root.join("b.mdoc");
    write(
        &root.join("root.mdoc"),
        "@fnode: root-node\n@title: Root\n\n@dep:\na-node\nb-node\n@end\n",
    );
    write(&a_path, "@fnode: a-node\n@title: A\n");
    write(&b_path, "@fnode: b-node\n@title: B\n");
    write(&root.join("x.mdoc"), "@fnode: x-node\n@title: X\n");
    write(&root.join("y.mdoc"), "@fnode: y-node\n@title: Y\n");
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();

    write(
        &a_path,
        "@fnode: a-node\n@title: A\n\n@dep:\nx-node\n@end\n",
    );
    write(
        &b_path,
        "@fnode: b-node\n@title: B\n\n@dep:\ny-node\n@end\n",
    );
    cache.discover_workspace_changes().unwrap();

    assert_eq!(topo_depth(&cache, "a-node"), 1);
    assert_eq!(topo_depth(&cache, "b-node"), 1);
    assert_eq!(topo_depth(&cache, "root-node"), 2);
}

#[test]
fn test_lazy_component_rebuild_updates_every_member_size() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);

    let a_path = root.join("a.mdoc");
    write(
        &a_path,
        "@fnode: a-node\n@title: A\n\n@dep:\nb-node\n@end\n",
    );
    write(&root.join("b.mdoc"), "@fnode: b-node\n@title: B\n");
    write(
        &root.join("c.mdoc"),
        "@fnode: c-node\n@title: C\n\n@dep:\nd-node\n@end\n",
    );
    write(&root.join("d.mdoc"), "@fnode: d-node\n@title: D\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    write(
        &a_path,
        "@fnode: a-node\n@title: A\n\n@dep:\nb-node\nc-node\n@end\n",
    );
    cache.upsert_path(&a_path.canonicalize().unwrap()).unwrap();
    assert!(cache
        .all_valid_edges()
        .unwrap()
        .contains(&("a-node".to_string(), "c-node".to_string())));
    cache.global_root_items().unwrap();

    let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let rows: Vec<(String, u32)> = conn
        .prepare(
            "SELECT fnode, component_size
             FROM mdoc_weak_component ORDER BY fnode",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter().all(|(_, size)| *size == 4),
        "unexpected component rows: {rows:?}"
    );
    let has_component_id: bool = conn
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM pragma_table_info('mdoc_weak_component')
                 WHERE name = 'component_id'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!has_component_id);
}

#[test]
fn test_old_schema_rebuilds_topo_depth() {
    // Simulate upgrading from an old derived index.
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

    // Simulate an old index with stale derived depths.
    {
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 5;
             UPDATE mdocs SET topo_depth = 0;",
        )
        .unwrap();
    }

    // Re-open: schema rebuild and workspace bootstrap restore real depths.
    let cache2 = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        topo_depth(&cache2, "parent-node"),
        1,
        "parent-node should have topo_depth = 1 after backfill"
    );
    assert_eq!(
        topo_depth(&cache2, "child-node"),
        0,
        "child-node (leaf) should have topo_depth = 0"
    );
}

#[test]
fn test_interrupted_bootstrap_rebuilds_topo_depth() {
    // A crash after schema creation leaves bootstrapped=0, so the next open
    // rebuilds all derived data from source files.
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

    // Simulate the crash window: stale derived data with bootstrap incomplete.
    {
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
        conn.execute_batch(
            "UPDATE mdocs SET topo_depth = 0;
             UPDATE mdoc_index_state SET bootstrapped = 0 WHERE id = 1;",
        )
        .unwrap();
    }

    // Re-open performs the normal full bootstrap.
    let cache2 = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        topo_depth(&cache2, "parent-node"),
        1,
        "parent-node should have topo_depth = 1 after bootstrap recovery"
    );
}

#[test]
fn test_old_schema_rebuild_recomputes_stale_topo_depths() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("root.mdoc"),
        "@fnode: root-node\n@title: Root\n\n@dep:\nleaf-node\n@end\n",
    );
    write(&root.join("leaf.mdoc"), "@fnode: leaf-node\n@title: Leaf\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    let db_path = index_path(&cache);
    drop(cache);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("UPDATE mdocs SET topo_depth = 99", [])
        .unwrap();
    conn.execute_batch("PRAGMA user_version = 9;").unwrap();
    drop(conn);

    let cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(topo_depth(&cache, "leaf-node"), 0);
    assert_eq!(topo_depth(&cache, "root-node"), 1);
}

#[cfg(unix)]
#[test]
fn test_upsert_path_rejects_final_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let outside = tempfile::TempDir::new().unwrap();
    let outside_path = outside.path().join("outside.mdoc");
    write(&outside_path, "@fnode: outside-node\n@title: Outside\n");
    let link = root.join("link.mdoc");
    symlink(&outside_path, &link).unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert!(cache.upsert_path(&link).is_err());
    assert_eq!(indexed_fnode_count(&cache, "outside-node"), 0);
}

#[cfg(unix)]
#[test]
fn test_path_resolution_enforces_node_path_invariant() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join(".mdc/hidden.mdoc"),
        "@fnode: hidden-node\n@title: Hidden\n",
    );
    write(
        &root.join("note.txt"),
        "@fnode: text-node\n@title: Text Node\n",
    );
    fs::create_dir(root.join("real")).unwrap();
    write(
        &root.join("real/node.mdoc"),
        "@fnode: real-node\n@title: Real Node\n",
    );
    symlink(root.join("real"), root.join("alias")).unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    assert!(cache.resolve_ref(".mdc/hidden.mdoc", Some(root)).is_err());
    assert!(cache.resolve_ref("./note.txt", Some(root)).is_err());
    assert!(cache.resolve_ref("alias/node.mdoc", Some(root)).is_err());
    assert_eq!(
        cache.resolve_ref("real/node.mdoc", Some(root)).unwrap().0,
        "real-node"
    );
}

#[test]
fn test_reachable_refresh_propagates_old_fnode_after_rename() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let dep_path = root.join("dep.mdoc");
    write(&root.join("leaf.mdoc"), "@fnode: leaf-node\n@title: Leaf\n");
    write(
        &dep_path,
        "@fnode: old-dep\n@title: Dep\n\n@dep:\nleaf-node\n@end\n",
    );
    write(
        &root.join("root.mdoc"),
        "@fnode: root-node\n@title: Root\n\n@dep:\nold-dep\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    assert_eq!(topo_depth(&cache, "root-node"), 2);

    write(
        &dep_path,
        "@fnode: new-dep\n@title: Dep\n\n@dep:\nleaf-node\n@end\n",
    );
    cache.refresh_reachable_from_path(&dep_path, -1).unwrap();
    assert_eq!(topo_depth(&cache, "root-node"), 1);
}

#[test]
fn reachable_refresh_detects_same_metadata_semantic_edits() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    let dependency = root.join("dependency.mdoc");
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\ndep-node\n@end\n",
    );
    write(&dependency, "@fnode: dep-node\n@title: Before\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    rewrite_preserving_mtime_and_size(&dependency, "@fnode: dep-node\n@title: After!\n", false);

    cache.refresh_reachable_from_path(&source, -1).unwrap();

    assert_eq!(cache.node_summary("dep-node").unwrap().title, "After!");
}

#[test]
fn test_upsert_path_removes_stale_entry_after_parent_directory_deletion() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let nested = root.join("removed");
    fs::create_dir(&nested).unwrap();
    let path = nested.join("node.mdoc");
    write(&path, "@fnode: removed-node\n@title: Removed\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.upsert_path(&path).unwrap();
    assert_eq!(indexed_fnode_count(&cache, "removed-node"), 1);

    fs::remove_dir_all(&nested).unwrap();
    cache.upsert_path(&path).unwrap();
    assert_eq!(indexed_fnode_count(&cache, "removed-node"), 0);
}

#[test]
fn test_old_schema_rebuilds_in_degree() {
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
        let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
        conn.execute_batch("PRAGMA user_version = 4; DELETE FROM mdoc_in_degree;")
            .unwrap();
    }

    // Re-open rebuilds the index from source files, including in_degree.
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

#[test]
fn full_refresh_skips_graph_rebuild_when_index_semantics_are_unchanged() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@src: text\nfirst body\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let epoch = |cache: &IndCache| -> i64 {
        rusqlite::Connection::open(index_path(cache))
            .unwrap()
            .query_row(
                "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let initial_epoch = epoch(&cache);

    cache.refresh_all().unwrap();
    assert_eq!(epoch(&cache), initial_epoch);

    write(
        &source,
        "@fnode: source-node\n@title: Renamed Source\n\n@src: text\nchanged body\n@end\n",
    );
    cache.refresh_all().unwrap();
    assert_eq!(epoch(&cache), initial_epoch);
    assert_eq!(
        cache.node_summary("source-node").unwrap().title,
        "Renamed Source"
    );

    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@src: text\nchanged body\n@end\n",
    );
    cache.refresh_all().unwrap();
    assert_eq!(epoch(&cache), initial_epoch);

    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\nmissing-node\n@end\n",
    );
    cache.refresh_all().unwrap();
    assert!(epoch(&cache) > initial_epoch);
    assert_eq!(cache.graph_check_report().unwrap().missing.len(), 1);
}

#[test]
fn unchanged_reachable_refresh_preserves_the_workspace_digest() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\ndep-node\n@end\n",
    );
    write(
        &root.join("dep.mdoc"),
        "@fnode: dep-node\n@title: Dependency\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    let digest = |cache: &IndCache| -> String {
        rusqlite::Connection::open(index_path(cache))
            .unwrap()
            .query_row(
                "SELECT index_digest FROM mdoc_index_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let initial_digest = digest(&cache);
    assert!(!initial_digest.is_empty());

    cache.refresh_reachable_from_path(&source, -1).unwrap();

    assert_eq!(digest(&cache), initial_digest);
}

#[test]
fn semantic_upsert_invalidates_digest_for_an_aba_full_refresh() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    let initial = "@fnode: source-node\n@title: Initial\n\n@dep:\nfirst-node\nsecond-node\n@end\n";
    write(&source, initial);
    write(
        &root.join("first.mdoc"),
        "@fnode: first-node\n@title: First\n",
    );
    write(
        &root.join("second.mdoc"),
        "@fnode: second-node\n@title: Second\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    write(
        &source,
        "@fnode: source-node\n@title: Changed\n\n@dep:\nsecond-node\nfirst-node\n@end\n",
    );
    cache.upsert_path(&source).unwrap();
    write(&source, initial);

    cache.refresh_all().unwrap();

    assert_eq!(cache.node_summary("source-node").unwrap().title, "Initial");
    assert_eq!(
        cache
            .direct_dependency_summaries("source-node")
            .unwrap()
            .into_iter()
            .map(|node| node.fnode)
            .collect::<Vec<_>>(),
        ["first-node", "second-node"]
    );
}

#[test]
fn full_refresh_recognizes_an_incrementally_synchronized_index() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let source = root.join("source.mdoc");
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\nfirst-node\n@end\n",
    );
    write(
        &root.join("first.mdoc"),
        "@fnode: first-node\n@title: First\n",
    );
    write(
        &root.join("second.mdoc"),
        "@fnode: second-node\n@title: Second\n",
    );
    write(
        &root.join("broken.mdoc"),
        "@fnode: broken-node\n@title: Broken\n\n@dep:\ninvalid dependency\n@end\n",
    );
    write(&root.join("unknown.mdoc"), "not an mdoc\n");
    write(
        &root.join("duplicate-a.mdoc"),
        "@fnode: duplicate-node\n@title: Duplicate A\n",
    );
    write(
        &root.join("duplicate-b.mdoc"),
        "@fnode: duplicate-node\n@title: Duplicate B\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    write(
        &source,
        "@fnode: source-node\n@title: Source\n\n@dep:\nsecond-node\n@end\n",
    );
    cache.upsert_path(&source).unwrap();
    let connection = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let epoch: i64 = connection
        .query_row(
            "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);

    cache.refresh_all().unwrap();

    let connection = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let refreshed_epoch: i64 = connection
        .query_row(
            "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let digest: String = connection
        .query_row(
            "SELECT index_digest FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refreshed_epoch, epoch);
    assert!(!digest.is_empty());
    assert_eq!(
        cache
            .direct_dependency_summaries("source-node")
            .unwrap()
            .into_iter()
            .map(|node| node.fnode)
            .collect::<Vec<_>>(),
        ["second-node"]
    );
}

#[test]
fn strong_refresh_applies_small_addition_and_deletion_deltas() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(
        &root.join("source.mdoc"),
        "@fnode: source-node\n@title: Source\n\n@dep:\ntarget-node\n@end\n",
    );
    let target = root.join("target.mdoc");
    write(&target, "@fnode: target-node\n@title: Target\n");
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert!(cache.graph_check_report().unwrap().missing.is_empty());
    assert_eq!(indexed_document_count(&cache), 2);

    std::fs::remove_file(&target).unwrap();
    cache.refresh_all().unwrap();
    let report = cache.graph_check_report().unwrap();
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].fnode, "target-node");
    assert_eq!(indexed_document_count(&cache), 1);

    write(&target, "@fnode: target-node\n@title: Restored\n");
    cache.refresh_all().unwrap();
    assert!(cache.graph_check_report().unwrap().missing.is_empty());
    assert_eq!(cache.node_summary("target-node").unwrap().title, "Restored");
    assert_eq!(indexed_document_count(&cache), 2);
}

#[test]
fn blocking_claimant_transition_refreshes_graph_caches_and_invalid_deletion_epoch() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    let claimant_b = root.join("claimant-b.mdoc");
    write(
        &root.join("claimant-a.mdoc"),
        "@fnode: shared-node\n@title: Claimant A\n\n@dep:\ncycle-node\nleaf-node\n@end\n",
    );
    write(&claimant_b, "@fnode: shared-node\n@title: Claimant B\n");
    write(
        &root.join("cycle.mdoc"),
        "@fnode: cycle-node\n@title: Cycle\n\n@dep:\nshared-node\n@end\n",
    );
    write(
        &root.join("root.mdoc"),
        "@fnode: root-node\n@title: Root\n\n@dep:\nshared-node\n@end\n",
    );
    write(&root.join("leaf.mdoc"), "@fnode: leaf-node\n@title: Leaf\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();

    assert!(cache.graph_check_report().unwrap().cycles.is_empty());
    let initial_roots = cache.global_root_items().unwrap();
    assert_eq!(
        initial_roots
            .iter()
            .find(|item| item.fnode == "root-node")
            .unwrap()
            .component_size,
        3
    );
    assert_eq!(
        initial_roots
            .iter()
            .find(|item| item.fnode == "leaf-node")
            .unwrap()
            .component_size,
        1
    );

    // The fallback identity keeps the same fnode but loses its title. Claimant A
    // becomes valid, activating its stored edges without either claimant B's
    // identity token or edge set changing.
    write(&claimant_b, "@fnode: shared-node\n");
    cache.upsert_path(&claimant_b).unwrap();

    let report = cache.graph_check_report().unwrap();
    assert_eq!(report.cycles.len(), 1, "stale SCC cache: {report:?}");
    let refreshed_roots = cache.global_root_items().unwrap();
    assert_eq!(
        refreshed_roots
            .iter()
            .find(|item| item.fnode == "root-node")
            .unwrap()
            .component_size,
        4
    );
    assert!(refreshed_roots.iter().all(|item| item.fnode != "leaf-node"));
    assert_eq!(
        indexed_fnode_count(&cache, "shared-node"),
        1,
        "the malformed claimant must have no mdocs identity"
    );

    let epoch_before_delete: i64 = rusqlite::Connection::open(index_path(&cache))
        .unwrap()
        .query_row(
            "SELECT graph_epoch FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    fs::remove_file(&claimant_b).unwrap();
    cache.discover_workspace_changes().unwrap();

    let conn = rusqlite::Connection::open(index_path(&cache)).unwrap();
    let (epoch_after_delete, component_dirty): (i64, bool) = conn
        .query_row(
            "SELECT graph_epoch, weak_component_dirty FROM mdoc_index_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(epoch_after_delete > epoch_before_delete);
    assert!(component_dirty);
}

#[test]
fn test_noncanonical_case_variants_are_reported_invalid() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    setup(root);
    write(&root.join("lower.mdoc"), "@fnode: node\n@title: Lower\n");
    write(&root.join("upper.mdoc"), "@fnode: NODE\n@title: Upper\n");
    write(
        &root.join("a.mdoc"),
        "@fnode: cycle-a\n@title: A\n\n@dep:\nCYCLE-B\n@end\n",
    );
    write(
        &root.join("b.mdoc"),
        "@fnode: cycle-b\n@title: B\n\n@dep:\nCYCLE-A\n@end\n",
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let report = cache.graph_check_report().unwrap();

    assert_eq!(report.invalid.len(), 3);
    assert!(report.cycles.is_empty());
    assert!(cache
        .search("node", usize::MAX)
        .unwrap()
        .iter()
        .any(|node| node.fnode == "NODE"));
    assert_eq!(
        cache
            .dependency_candidates("node", "NODE", 10)
            .unwrap()
            .empty,
        Some(DependencyCandidatesEmpty::Excluded {
            source: 1,
            existing_dependencies: 0,
            invalid_or_duplicate: 1,
        })
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
