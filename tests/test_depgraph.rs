use std::fs;
use std::path::Path;

use mathdoc::depgraph::DepGraph;
use mathdoc::indcache::IndCache;
use mathdoc::mdocnode::{MdocNode, SrcBlock};

fn expect_err<T>(result: anyhow::Result<T>) -> anyhow::Error {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected Err but got Ok"),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a node file in `root` with one block. Returns the unsaved MdocNode.
fn make_node(root: &Path, title: &str, srctype: &str, content: &str) -> MdocNode {
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let mut node = MdocNode::new_at_path(root, title); // temp path
    node.path = root.join(format!("{}.mdoc", &node.fnode[..8]));
    node.blocks.push(SrcBlock {
        srctype: srctype.to_string(),
        content: content.to_string(),
        metadata: Default::default(),
    });
    node
}

fn write_node(node: &MdocNode) {
    if let Some(parent) = node.path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&node.path, node.render().unwrap()).unwrap();
}

fn make_invalid(path: &Path) {
    let mut text = fs::read_to_string(path).unwrap();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("@title: Duplicate Broken Title\n");
    fs::write(path, text).unwrap();
}

// ── from_ref ──────────────────────────────────────────────────────────────────

#[test]
fn test_from_ref_loads_root_graph() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Src", "text", "src");
    write_node(&src);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();

    let graph = DepGraph::from_ref(&mut cache, &src.fnode[..8], Some(root)).unwrap();
    assert_eq!(graph.root_item().unwrap().fnode, src.fnode);
}

#[test]
fn test_from_ref_rejects_duplicate_root_fnode_even_by_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    fs::write(root.join("dup-a.mdoc"), "@fnode: dup-node\n@title: Dup A\n").unwrap();
    fs::write(root.join("dup-b.mdoc"), "@fnode: dup-node\n@title: Dup B\n").unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();

    let err = expect_err(DepGraph::from_ref(&mut cache, "dup-a.mdoc", Some(root)));
    assert!(
        err.to_string().contains("duplicate fnode 'dup-node'"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_from_ref_detects_duplicate_via_filesystem_fallback() {
    // Scenario: a.mdoc is already indexed; b.mdoc has the same fnode but was written
    // externally after the last bootstrap.  from_ref resolves b.mdoc via filesystem
    // path lookup (it's not yet in the index), upserts it, then catches the duplicate.
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // Write and index only a.mdoc
    fs::write(root.join("a.mdoc"), "@fnode: shared-node\n@title: A\n").unwrap();
    let _cache = IndCache::open(root.to_path_buf()).unwrap();

    // Write b.mdoc with the same fnode — not yet indexed
    fs::write(root.join("b.mdoc"), "@fnode: shared-node\n@title: B\n").unwrap();

    // Opening a fresh cache discovers b.mdoc before reference resolution.
    let mut cache2 = IndCache::open(root.to_path_buf()).unwrap();
    let err = expect_err(DepGraph::from_ref(&mut cache2, "b.mdoc", Some(root)));
    assert!(
        err.to_string().contains("duplicate fnode 'shared-node'"),
        "expected duplicate error, got: {err}"
    );
}

#[test]
fn from_ref_rejects_a_stale_fnode_that_now_names_a_different_node() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let path = root.join("node.mdoc");
    fs::write(&path, "@fnode: old-node\n@title: Old\n").unwrap();
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    fs::write(&path, "@fnode: new-node\n@title: New\n").unwrap();

    let error = expect_err(DepGraph::from_ref(&mut cache, "old-node", Some(root)));

    assert!(error.to_string().contains("identity changed"));
    assert_eq!(MdocNode::load(&path).unwrap().fnode, "new-node");
}

// ── mutation ──────────────────────────────────────────────────────────────────

#[test]
fn test_direct_dependency_mutation_uses_graph_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "text", "src");
    write_node(&src);
    let dep1 = make_node(root, "Dep1", "text", "dep1");
    write_node(&dep1);
    let dep2 = make_node(root, "Dep2", "text", "dep2");
    write_node(&dep2);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();

    let (added, skipped_existing, skipped_self) = graph
        .add_direct_dependencies(vec![
            dep1.fnode.clone(),
            src.fnode.clone(),
            dep2.fnode.clone(),
        ])
        .unwrap();

    assert_eq!(added, vec![dep1.fnode.clone(), dep2.fnode.clone()]);
    assert!(skipped_existing.is_empty());
    assert_eq!(skipped_self, vec![src.fnode.clone()]);
    assert_eq!(
        graph
            .direct_dependency_items()
            .unwrap()
            .into_iter()
            .map(|item| item.fnode)
            .collect::<Vec<_>>(),
        vec![dep1.fnode.clone(), dep2.fnode.clone()]
    );

    // Add existing dep again
    let (added2, skipped2, _) = graph
        .add_direct_dependencies(vec![dep1.fnode.clone()])
        .unwrap();
    assert!(added2.is_empty());
    assert_eq!(skipped2, vec![dep1.fnode.clone()]);

    // Remove dep
    let removed = graph
        .remove_direct_dependencies(vec![
            dep1.fnode.clone(),
            "missing".to_string(),
            dep1.fnode.clone(),
        ])
        .unwrap();
    assert_eq!(removed, vec![dep1.fnode.clone()]);
    assert_eq!(
        graph
            .direct_dependency_items()
            .unwrap()
            .into_iter()
            .map(|item| item.fnode)
            .collect::<Vec<_>>(),
        vec![dep2.fnode.clone()]
    );

    // Verify file was updated
    let reloaded = MdocNode::load(&src.path).unwrap();
    assert_eq!(reloaded.depens, vec![dep2.fnode.clone()]);
}

#[test]
fn direct_dependency_items_refresh_external_breakage() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let missing = make_node(root, "Missing", "text", "missing");
    write_node(&missing);
    let invalid = make_node(root, "Invalid", "text", "invalid");
    write_node(&invalid);
    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&missing.fnode);
    src.add_dependency(&invalid.fnode);
    write_node(&src);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let db_path = cache.root().join(".mdc/index.db");
    let mut graph = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();
    fs::remove_file(&missing.path).unwrap();
    make_invalid(&invalid.path);
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "INSERT INTO mdocs (path, fnode, title, title_lc, topo_depth)
             VALUES ('corrupt.mdoc', 'corrupt-node', X'00', 'corrupt', 0);
         INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
             VALUES ('unrelated.mdoc', 'invalid', 'unrelated-node', X'00');",
    )
    .unwrap();
    drop(conn);

    let items = graph.direct_dependency_items().unwrap();
    assert_eq!(items[0].title, "<missing>");
    assert_eq!(items[1].title, "<invalid>");
}

#[test]
fn test_add_direct_dependencies_rejects_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "text", "src");
    write_node(&src);
    let dep = make_node(root, "Dep", "text", "dep");
    write_node(&dep);

    // src → dep
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph_src = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();
    let (added, _, _) = graph_src
        .add_direct_dependencies(vec![dep.fnode.clone()])
        .unwrap();
    assert_eq!(added, vec![dep.fnode.clone()]);
    drop(graph_src);

    // dep → src would create a cycle — should be rejected
    let mut graph_dep = DepGraph::from_ref(&mut cache, &dep.fnode, None).unwrap();
    let result = graph_dep.add_direct_dependencies(vec![src.fnode.clone()]);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle"),
        "expected cycle error, got: {err_msg}"
    );

    // dep's file should NOT have been modified
    let reloaded = MdocNode::load(&dep.path).unwrap();
    assert!(reloaded.depens.is_empty());
}

#[test]
fn test_failed_dependency_save_does_not_mutate_graph_state() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Src", "text", "src");
    write_node(&src);
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();

    let result = graph.add_direct_dependencies(vec!["@end".to_string()]);
    assert!(result.is_err());
    assert!(MdocNode::load(&src.path).unwrap().depens.is_empty());
}

#[test]
fn batch_add_revalidates_every_exact_target_before_writing() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Src", "text", "src");
    write_node(&src);
    let valid = make_node(root, "Valid", "text", "valid");
    write_node(&valid);
    let deleted = make_node(root, "Deleted", "text", "deleted");
    write_node(&deleted);
    fs::write(
        root.join("invalid-target.mdoc"),
        "@fnode: invalid-target\n@title: Invalid\n@title: Again\n",
    )
    .unwrap();
    fs::write(
        root.join("duplicate-a.mdoc"),
        "@fnode: duplicate-target\n@title: Duplicate A\n",
    )
    .unwrap();
    fs::write(
        root.join("duplicate-b.mdoc"),
        "@fnode: duplicate-target\n@title: Duplicate B\n",
    )
    .unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();
    fs::remove_file(&deleted.path).unwrap();
    let non_exact = valid.fnode.to_ascii_uppercase();

    for rejected in [
        "missing-target",
        "invalid-target",
        "duplicate-target",
        deleted.fnode.as_str(),
        non_exact.as_str(),
    ] {
        let error = graph
            .add_direct_dependencies(vec![valid.fnode.clone(), rejected.to_string()])
            .unwrap_err();
        assert!(
            error.to_string().contains("dependency target")
                || error.to_string().contains("duplicate fnode"),
            "unexpected error for {rejected}: {error}"
        );
        assert!(MdocNode::load(&src.path).unwrap().depens.is_empty());
    }
}

#[test]
fn prepared_dependency_paths_preserve_mdoc_suffix_and_dot_default() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Src", "text", "src");
    write_node(&src);
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &src.fnode, None).unwrap();

    let suffixed = graph
        .prepare_new_dependency_node("notes/foo.mdoc", "Suffixed", Some("suffixed-node"))
        .unwrap();
    assert_eq!(
        suffixed.path,
        root.canonicalize().unwrap().join("notes/foo.mdoc")
    );
    graph.create_and_add_dependency(suffixed).unwrap();
    assert!(root.join("notes/foo.mdoc").is_file());
    assert!(!root.join("notes/foo.mdoc.mdoc").exists());

    let defaulted = graph
        .prepare_new_dependency_node(".", "Defaulted", Some("defaulted-node"))
        .unwrap();
    assert_eq!(
        defaulted.path,
        root.canonicalize().unwrap().join("defaulted-node.mdoc")
    );
    graph.create_and_add_dependency(defaulted).unwrap();
    assert!(root.join("defaulted-node.mdoc").is_file());
    assert!(!root.join("..mdoc").exists());
}

#[test]
fn test_create_and_add_dependency_no_side_effects_on_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();

    let root_node = make_node(root_dir, "Root", "text", "root");
    write_node(&root_node);

    // Build a new node whose @dep already points back at root — this would form
    // the cycle root → new_node → root the moment we add root → new_node.
    let mut new_node = make_node(root_dir, "New", "text", "new");
    new_node.add_dependency(&root_node.fnode);
    let new_path = new_node.path.clone();
    let new_fnode = new_node.fnode.clone();

    let mut cache = IndCache::open(root_dir.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();
    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle"),
        "expected cycle error, got: {err_msg}"
    );

    // No file should have been created on disk.
    assert!(
        !new_path.exists(),
        "new node file must not exist after failure"
    );

    // The new node must not appear in the index.
    drop(graph);
    let search_results = cache.search(&new_fnode[..8], usize::MAX).unwrap();
    assert!(
        search_results.is_empty(),
        "new node must not be indexed after failure"
    );
}

#[test]
fn create_and_add_dependency_rejects_invalid_declared_targets_without_side_effects() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let root_node = make_node(root, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();
    let mut new_node = make_node(root, "New", "text", "new");
    new_node.add_dependency("missing-target");
    let new_path = new_node.path.clone();

    let error = graph.create_and_add_dependency(new_node).unwrap_err();

    assert!(error.to_string().contains("dependency target is missing"));
    assert!(!new_path.exists());
}

#[test]
fn create_and_add_dependency_rolls_back_file_and_index_when_linking_fails() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let root_node = make_node(root, "Root", "text", "root");
    write_node(&root_node);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let connection = rusqlite::Connection::open(cache.root().join(".mdc/index.db")).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER reject_root_link
             BEFORE UPDATE ON mdocs
             WHEN NEW.fnode = '{}'
             BEGIN
                 SELECT RAISE(ABORT, 'injected link failure');
             END;",
            root_node.fnode
        ))
        .unwrap();
    drop(connection);
    let new_node = make_node(root, "Created", "text", "created");
    let new_path = new_node.path.clone();
    let new_fnode = new_node.fnode.clone();

    let error = graph.create_and_add_dependency(new_node).unwrap_err();

    assert!(error.to_string().contains("injected link failure"));
    assert!(!new_path.exists());
    drop(graph);
    assert!(cache.search(&new_fnode, usize::MAX).unwrap().is_empty());
}

#[test]
fn test_create_root_rejects_duplicate_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // First create succeeds and establishes the fnode in the index.
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let graph = DepGraph::create_root(&mut cache, "first", "First", None).unwrap();
    let existing_fnode = graph.root_item().unwrap().fnode;
    drop(graph);

    // Second create with the same fnode must fail before writing anything.
    let second_path = root.join("second.mdoc");
    let result = DepGraph::create_root(&mut cache, "second", "Second", Some(&existing_fnode));

    let err_msg = result.err().expect("expected error").to_string();
    assert!(
        err_msg.contains("already used"),
        "expected duplicate fnode error, got: {err_msg}"
    );
    // No file should have been created for the second node.
    assert!(
        !second_path.exists(),
        "second node file must not exist after failure"
    );
}

/// P2 regression: create_root() must bootstrap before the duplicate-fnode check, so that a
/// file already on disk (but not yet in the index) is discovered and the collision is caught.
#[test]
fn test_create_root_rejects_duplicate_fnode_unindexed() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // Write an existing mdoc file to disk WITHOUT indexing it.
    let existing = make_node(root, "Existing", "text", "");
    write_node(&existing);
    let existing_fnode = existing.fnode.clone();

    // Attempt to create a new root with the same fnode — bootstrap must surface
    // the on-disk file so the duplicate check fires before any write.
    let second_path = root.join("second.mdoc");
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let result = DepGraph::create_root(&mut cache, "second", "Second", Some(&existing_fnode));

    let err_msg = result
        .err()
        .expect("expected error for unindexed duplicate fnode")
        .to_string();
    assert!(
        err_msg.contains("already used"),
        "expected duplicate fnode error, got: {err_msg}"
    );
    assert!(
        !second_path.exists(),
        "second node file must not exist after failure"
    );
}

#[test]
fn test_create_and_add_dependency_rejects_duplicate_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();

    let root_node = make_node(root_dir, "Root", "text", "root");
    write_node(&root_node);

    let mut cache = IndCache::open(root_dir.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    // Build a new node whose fnode is deliberately set to root's fnode.
    let mut dup_node = make_node(root_dir, "Dup", "text", "dup");
    dup_node.fnode = root_node.fnode.clone();
    let dup_path = dup_node.path.clone();

    let result = graph.create_and_add_dependency(dup_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("already used"),
        "expected duplicate fnode error, got: {err_msg}"
    );
    // No file should have been created.
    assert!(
        !dup_path.exists(),
        "duplicate node file must not exist after failure"
    );
}

/// P2 regression: create_and_add_dependency() must reject non-.mdoc extensions to avoid
/// creating index entries that workspace discovery would never scan.
#[test]
fn test_create_and_add_dependency_rejects_non_mdoc_extension() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();

    let root_node = make_node(root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    let mut new_node = make_node(root_dir, "Txt", "text", "txt");
    new_node.path = root_dir.join("note.txt");

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains(".mdoc"),
        "expected .mdoc extension error, got: {err_msg}"
    );
    assert!(
        !root_dir.join("note.txt").exists(),
        "non-.mdoc file must not be written"
    );
}

/// P1 regression: create_and_add_dependency() must refuse to write when the target path
/// already exists on disk, even if fnode and cycle checks pass.
#[test]
fn test_create_and_add_dependency_rejects_existing_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();

    let root_node = make_node(root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    // Write an unrelated victim file at the path the new node would occupy.
    let mut new_node = make_node(root_dir, "New", "text", "new");
    fs::write(&new_node.path, b"victim content").unwrap();
    // Give the new node a different fnode so the duplicate-fnode check doesn't fire first.
    new_node.fnode = format!("{}x", &new_node.fnode[..new_node.fnode.len() - 1]);
    let victim_path = new_node.path.clone();

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("already exists"),
        "expected path-collision error, got: {err_msg}"
    );
    // Victim file content must be untouched.
    assert_eq!(fs::read(&victim_path).unwrap(), b"victim content");
}

/// P1 regression: create_and_add_dependency() must refuse to write a file outside
/// the workspace root.
#[test]
fn test_create_and_add_dependency_rejects_path_outside_workspace() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();
    let outside_dir = tempfile::TempDir::new().unwrap();

    let root_node = make_node(root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.to_path_buf()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    let mut new_node = make_node(root_dir, "Outside", "text", "outside");
    // Point the path to a location outside the workspace.
    new_node.path = outside_dir.path().join("outside.mdoc");
    let outside_path = new_node.path.clone();

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    assert!(
        !outside_path.exists(),
        "file must not be written outside workspace"
    );
}

/// P1 regression: create_and_add_dependency() must refuse to write a file inside
/// a nested workspace (a directory that itself contains a .mdc/ subdirectory).
#[test]
fn test_create_and_add_dependency_rejects_path_in_nested_workspace() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path().canonicalize().unwrap();
    // Create a nested workspace inside the outer one.
    let nested = root_dir.join("sub");
    fs::create_dir_all(nested.join(".mdc")).unwrap();

    let root_node = make_node(&root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.clone()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    let mut new_node = make_node(&root_dir, "Nested", "text", "nested");
    new_node.path = nested.join("nested.mdoc");
    let nested_path = new_node.path.clone();

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("nested mdoc root"),
        "expected nested-workspace error, got: {err_msg}"
    );
    assert!(
        !nested_path.exists(),
        "file must not be written inside nested workspace"
    );
}

/// P1 regression: .. components in new_node.path must not allow escaping the
/// workspace root, even when intermediate directories don't yet exist on disk.
/// Covers create_and_add_dependency().
#[test]
fn test_create_and_add_dependency_rejects_dotdot_escape() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path().canonicalize().unwrap();

    let root_node = make_node(&root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.clone()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    // Construct a path that looks like it starts under root but uses .. to escape.
    // Use the unique temp dir name as the stem so parallel runs don't share a filename.
    let stem = root_dir.file_name().unwrap().to_str().unwrap();
    let escaped_name = format!("{stem}-escaped.mdoc");
    let mut new_node = make_node(&root_dir, "Escape", "text", "escape");
    new_node.path = root_dir
        .join("nope")
        .join("..")
        .join("..")
        .join(&escaped_name);
    let escaped_path = root_dir.parent().unwrap().join(&escaped_name);

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    assert!(!escaped_path.exists(), "escaped file must not be written");
}

/// P1 regression: same .. escape via create_root().
#[test]
fn test_create_root_rejects_dotdot_escape() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root_dir.join(".mdc")).unwrap();

    // A relative target with .. that would escape the workspace when joined to mdcroot.
    // Use the unique temp dir name so parallel runs don't share a filename.
    let stem = root_dir.file_name().unwrap().to_str().unwrap();
    let file_target = format!("nope/../../{stem}-escaped");
    let mut cache = IndCache::open(root_dir.clone()).unwrap();
    let result = DepGraph::create_root(&mut cache, &file_target, "Escape", None);

    assert!(result.is_err());
    let escaped_path = root_dir
        .parent()
        .unwrap()
        .join(format!("{stem}-escaped.mdoc"));
    assert!(!escaped_path.exists(), "escaped file must not be written");
}

#[test]
fn test_create_root_rejects_workspace_control_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join(".mdc")).unwrap();

    let mut cache = IndCache::open(root.clone()).unwrap();
    let result = DepGraph::create_root(&mut cache, ".mdc/hidden", "Hidden", None);

    assert!(result.is_err());
    assert!(!root.join(".mdc/hidden.mdoc").exists());
}

#[cfg(unix)]
#[test]
fn test_create_root_rejects_symlinked_parent_inside_workspace() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join(".mdc")).unwrap();
    fs::create_dir(root.join("real")).unwrap();
    symlink(root.join("real"), root.join("alias")).unwrap();

    let mut cache = IndCache::open(root.clone()).unwrap();
    let result = DepGraph::create_root(&mut cache, "alias/node", "Alias", None);

    assert!(result.is_err());
    assert!(!root.join("real/node.mdoc").exists());
}

/// P1 regression: symlink/.. must not allow escaping the workspace.
/// `root/link` → outside; POSIX `link/..` = parent-of-outside, not `root/`.
/// Covers create_and_add_dependency().
#[cfg(unix)]
#[test]
fn test_create_and_add_dependency_rejects_symlink_dotdot_escape() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path().canonicalize().unwrap();
    let outside_dir = tempfile::TempDir::new().unwrap();
    let outside_canonical = outside_dir.path().canonicalize().unwrap();

    // Symlink inside workspace → outside
    let link_path = root_dir.join("external_link");
    symlink(&outside_canonical, &link_path).unwrap();

    let root_node = make_node(&root_dir, "Root", "text", "root");
    write_node(&root_node);
    let mut cache = IndCache::open(root_dir.clone()).unwrap();
    let mut graph = DepGraph::from_ref(&mut cache, &root_node.fnode, None).unwrap();

    // root/external_link/../<name>.mdoc: lexically looks like root/<name>.mdoc,
    // but POSIX resolves external_link → outside, so link/.. = outside/.., OUTSIDE.
    let stem = root_dir.file_name().unwrap().to_str().unwrap();
    let escaped_name = format!("{stem}-sym-escaped.mdoc");
    let mut new_node = make_node(&root_dir, "Escape", "text", "escape");
    new_node.path = root_dir
        .join("external_link")
        .join("..")
        .join(&escaped_name);
    // Actual POSIX-resolved location the file would be written to:
    let potential_escape = outside_canonical.parent().unwrap().join(&escaped_name);

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err(), "symlink-dotdot escape must be rejected");
    assert!(
        !potential_escape.exists(),
        "escaped file must not be written"
    );
}

/// P1 regression: same symlink/.. escape via create_root().
#[cfg(unix)]
#[test]
fn test_create_root_rejects_symlink_dotdot_escape() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path().canonicalize().unwrap();
    let outside_dir = tempfile::TempDir::new().unwrap();
    let outside_canonical = outside_dir.path().canonicalize().unwrap();

    fs::create_dir_all(root_dir.join(".mdc")).unwrap();
    let link_path = root_dir.join("ext_link");
    symlink(&outside_canonical, &link_path).unwrap();

    let stem = root_dir.file_name().unwrap().to_str().unwrap();
    let file_target = format!("ext_link/../{stem}-sym-root-escaped");
    let mut cache = IndCache::open(root_dir.clone()).unwrap();
    let result = DepGraph::create_root(&mut cache, &file_target, "Escape", None);

    assert!(result.is_err(), "symlink-dotdot escape must be rejected");
    let escaped_name = format!("{stem}-sym-root-escaped.mdoc");
    let potential_escape = outside_canonical.parent().unwrap().join(&escaped_name);
    assert!(
        !potential_escape.exists(),
        "escaped file must not be written"
    );
}

#[cfg(unix)]
#[test]
fn test_create_root_rejects_dangling_final_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    let outside_target = outside.path().join("escaped.mdoc");
    symlink(&outside_target, root.join("link.mdoc")).unwrap();

    let mut cache = IndCache::open(root).unwrap();
    let result = DepGraph::create_root(&mut cache, "link", "Escape", None);
    assert!(result.is_err());
    assert!(!outside_target.exists());
}

/// P1 regression: create_root() with file_path="." or "" must not silently
/// overwrite an existing file — the default path still goes through validation.
#[test]
fn test_create_root_dot_target_rejects_existing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // Write a pre-existing file whose name matches the fnode we will force.
    let fnode = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let victim_path = root.join(format!("{fnode}.mdoc"));
    fs::write(&victim_path, b"victim content").unwrap();

    // create_root with file_path="." should refuse because the default path already exists.
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let result = DepGraph::create_root(&mut cache, ".", "New", Some(fnode));

    let err_msg = result
        .err()
        .expect("expected error for existing default path")
        .to_string();
    assert!(
        err_msg.contains("already exists"),
        "expected path-collision error, got: {err_msg}"
    );
    // Victim must be untouched.
    assert_eq!(fs::read(&victim_path).unwrap(), b"victim content");
}

// ── global_root_items ─────────────────────────────────────────────────────────

#[test]
fn test_global_root_items_include_unreferenced_valid_and_invalid_nodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    let leaf = make_node(root, "Leaf", "text", "leaf");
    write_node(&leaf);

    let mut root_valid = make_node(root, "Root Valid", "text", "root_valid");
    root_valid.add_dependency(&leaf.fnode);
    write_node(&root_valid);

    let other_root = make_node(root, "Other Root", "text", "other_root");
    write_node(&other_root);

    let bad_root = make_node(root, "Broken Root", "text", "bad_root");
    write_node(&bad_root);
    make_invalid(&bad_root.path);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let items = cache.global_root_items().unwrap();

    let by_fnode: std::collections::HashMap<&str, _> =
        items.iter().map(|i| (i.fnode.as_str(), i)).collect();

    assert!(
        by_fnode.contains_key(root_valid.fnode.as_str()),
        "root_valid should be a root"
    );
    assert_eq!(by_fnode[root_valid.fnode.as_str()].title, "Root Valid");
    assert_eq!(by_fnode[root_valid.fnode.as_str()].component_size, 2);

    assert!(
        by_fnode.contains_key(other_root.fnode.as_str()),
        "other_root should be a root"
    );
    assert_eq!(by_fnode[other_root.fnode.as_str()].component_size, 1);

    assert!(
        by_fnode.contains_key(bad_root.fnode.as_str()),
        "bad_root should be in roots (invalid)"
    );
    assert_eq!(by_fnode[bad_root.fnode.as_str()].title, "<invalid>");

    assert!(
        !by_fnode.contains_key(leaf.fnode.as_str()),
        "leaf should NOT be a root"
    );
}

// ── graph_check_report ────────────────────────────────────────────────────────

#[test]
fn test_graph_check_report_collects_missing_invalid_and_cycles() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    let bad = make_node(root, "Broken Node", "text", "bad");
    write_node(&bad);
    make_invalid(&bad.path);

    let mut a = make_node(root, "Cycle A", "text", "a");
    write_node(&a);
    let mut b = make_node(root, "Cycle B", "text", "b");
    write_node(&b);
    a.add_dependency(&b.fnode);
    write_node(&a);
    b.add_dependency(&a.fnode);
    write_node(&b);

    let mut src = make_node(root, "Source", "text", "src");
    src.add_dependency("missing-target-001");
    src.add_dependency(&bad.fnode);
    write_node(&src);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    let report = cache.graph_check_report().unwrap();

    assert_eq!(report.nodes, 4); // bad, a, b, src (missing not counted)
    assert_eq!(report.edges, 4); // a→b, b→a, src→missing, src→bad
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].fnode, "missing-target-001");
    assert_eq!(report.invalid.len(), 1);
    assert_eq!(report.cycles.len(), 1);
    let cycle_fnodes: std::collections::HashSet<&str> =
        report.cycles[0].iter().map(|s| s.as_str()).collect();
    assert!(cycle_fnodes.contains(a.fnode.as_str()));
    assert!(cycle_fnodes.contains(b.fnode.as_str()));
}

#[test]
fn graph_mutation_process_worker() {
    let Ok(root) = std::env::var("MDC_GRAPH_WORKER_ROOT") else {
        return;
    };
    let source = std::env::var("MDC_GRAPH_WORKER_SOURCE").unwrap();
    let target = std::env::var("MDC_GRAPH_WORKER_TARGET").unwrap();
    let ready = std::env::var("MDC_GRAPH_WORKER_READY").unwrap();
    let go = std::env::var("MDC_GRAPH_WORKER_GO").unwrap();
    let result_path = std::env::var("MDC_GRAPH_WORKER_RESULT").unwrap();

    // Construct before the barrier so both processes start with the same cached view.
    let mut cache = IndCache::open(std::path::PathBuf::from(&root));
    let graph = cache
        .as_mut()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .and_then(|cache| DepGraph::from_ref(cache, &source, None));
    std::fs::write(&ready, b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::path::Path::new(&go).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker barrier timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let text = match graph {
        Ok(mut graph) => match graph.add_direct_dependencies(vec![target]) {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("mutation-error:{error:#}"),
        },
        Err(error) => format!("init-error:{error:#}"),
    };
    std::fs::write(result_path, text).unwrap();
}

fn run_barriered_mutations(
    root: &std::path::Path,
    first: (&str, &str),
    second: (&str, &str),
) -> [String; 2] {
    use std::process::{Command, Stdio};

    let go = root.join(".mdc/test-mutation-go");
    let mut children = Vec::new();
    let mut ready_paths = Vec::new();
    let mut result_paths = Vec::new();
    for (index, (source, target)) in [first, second].into_iter().enumerate() {
        let ready = root.join(format!(".mdc/test-mutation-ready-{index}"));
        let result = root.join(format!(".mdc/test-mutation-result-{index}"));
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "graph_mutation_process_worker", "--nocapture"])
            .env("MDC_GRAPH_WORKER_ROOT", root)
            .env("MDC_GRAPH_WORKER_SOURCE", source)
            .env("MDC_GRAPH_WORKER_TARGET", target)
            .env("MDC_GRAPH_WORKER_READY", &ready)
            .env("MDC_GRAPH_WORKER_GO", &go)
            .env("MDC_GRAPH_WORKER_RESULT", &result)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        children.push(child);
        ready_paths.push(ready);
        result_paths.push(result);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ready_paths.iter().any(|path| !path.exists()) {
        assert!(
            std::time::Instant::now() < deadline,
            "parent barrier timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::fs::write(&go, b"go").unwrap();

    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "worker failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    [
        std::fs::read_to_string(&result_paths[0]).unwrap(),
        std::fs::read_to_string(&result_paths[1]).unwrap(),
    ]
}

#[test]
fn interprocess_opposite_edges_yield_one_success_and_no_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".mdc")).unwrap();
    fs::write(root.join("a.mdoc"), "@fnode: process-a\n@title: A\n").unwrap();
    fs::write(root.join("b.mdoc"), "@fnode: process-b\n@title: B\n").unwrap();
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    drop(cache);

    let results =
        run_barriered_mutations(root, ("process-a", "process-b"), ("process-b", "process-a"));
    assert_eq!(results.iter().filter(|result| *result == "ok").count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result.contains("cycle"))
            .count(),
        1,
        "unexpected worker results: {results:?}"
    );

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    assert!(cache.graph_check_report().unwrap().cycles.is_empty());
}

#[test]
fn interprocess_additions_to_one_source_preserve_both_edges() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".mdc")).unwrap();
    fs::write(
        root.join("source.mdoc"),
        "@fnode: process-source\n@title: Source\n",
    )
    .unwrap();
    fs::write(root.join("x.mdoc"), "@fnode: process-x\n@title: X\n").unwrap();
    fs::write(root.join("y.mdoc"), "@fnode: process-y\n@title: Y\n").unwrap();
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    drop(cache);

    let results = run_barriered_mutations(
        root,
        ("process-source", "process-x"),
        ("process-source", "process-y"),
    );
    assert_eq!(results, ["ok", "ok"]);

    let source = MdocNode::load(&root.join("source.mdoc")).unwrap();
    let deps: std::collections::HashSet<_> = source.depens.into_iter().collect();
    assert_eq!(
        deps,
        std::collections::HashSet::from(["process-x".to_string(), "process-y".to_string()])
    );
}
