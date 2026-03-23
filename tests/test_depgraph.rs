use std::fs;
use std::path::Path;

use mathdoc::core::IssueKind;
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
    let mut node = MdocNode::new_at_path(root, root, title); // temp path
    node.path = root.join(format!("{}.mdoc", &node.fnode[..8]));
    node.blocks.push(SrcBlock {
        srctype: srctype.to_string(),
        content: content.to_string(),
        metadata: Default::default(),
    });
    node
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
    src.save().unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.bootstrap_if_needed().unwrap();

    let (graph, rel_path) = DepGraph::from_ref(cache, &src.fnode[..8], Some(root)).unwrap();
    assert_eq!(graph.root_fnode(), src.fnode);
    assert!(rel_path.ends_with(".mdoc"));
}

#[test]
fn test_from_ref_rejects_duplicate_root_fnode_even_by_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    fs::write(root.join("dup-a.mdoc"), "@fnode: dup-node\n@title: Dup A\n").unwrap();
    fs::write(root.join("dup-b.mdoc"), "@fnode: dup-node\n@title: Dup B\n").unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.bootstrap_if_needed().unwrap();

    let err = expect_err(DepGraph::from_ref(cache, "dup-a.mdoc", Some(root)));
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
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.bootstrap_if_needed().unwrap();

    // Write b.mdoc with the same fnode — not yet indexed
    fs::write(root.join("b.mdoc"), "@fnode: shared-node\n@title: B\n").unwrap();

    // Open a fresh cache (no bootstrap) so b.mdoc is still absent from the index
    let cache2 = IndCache::open(root.to_path_buf()).unwrap();
    let err = expect_err(DepGraph::from_ref(cache2, "b.mdoc", Some(root)));
    assert!(
        err.to_string().contains("duplicate fnode 'shared-node'"),
        "expected duplicate error, got: {err}"
    );
}

// ── dependency_items ─────────────────────────────────────────────────────────

#[test]
fn test_dependency_items_expand_incrementally_from_root_node() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep2 = make_node(root, "Dep2", "text", "dep2");
    dep2.save().unwrap();

    let mut dep1 = make_node(root, "Dep1", "text", "dep1");
    dep1.add_dependency(&dep2.fnode);
    dep1.save().unwrap();

    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&dep1.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();

    let depth_1 = graph.dependency_items(1).unwrap();
    assert_eq!(depth_1.len(), 1);
    assert_eq!(depth_1[0].fnode, dep1.fnode);
    assert_eq!(depth_1[0].depth, 1);

    let depth_inf = graph.dependency_items(-1).unwrap();
    assert_eq!(depth_inf.len(), 2);
    assert_eq!(depth_inf[0].fnode, dep1.fnode);
    assert_eq!(depth_inf[1].fnode, dep2.fnode);
    assert_eq!(depth_inf[1].depth, 2);
}

#[test]
fn test_dependency_items_depth_zero_returns_empty() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let dep = make_node(root, "Dep", "text", "dep");
    dep.save().unwrap();
    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let items = graph.dependency_items(0).unwrap();
    assert!(items.is_empty());
}

#[test]
fn test_leaf_dependency_items_only_return_reachable_leaves() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let leaf_direct = make_node(root, "Leaf Direct", "text", "leaf_direct");
    leaf_direct.save().unwrap();
    let leaf_shared = make_node(root, "Leaf Shared", "text", "leaf_shared");
    leaf_shared.save().unwrap();

    let mut mid = make_node(root, "Mid", "text", "mid");
    mid.add_dependency(&leaf_shared.fnode);
    mid.save().unwrap();

    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&mid.fnode);
    src.add_dependency(&leaf_direct.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let items = graph.leaf_dependency_items().unwrap();

    let fnodes: Vec<&str> = items.iter().map(|i| i.fnode.as_str()).collect();
    assert!(fnodes.contains(&leaf_direct.fnode.as_str()));
    assert!(fnodes.contains(&leaf_shared.fnode.as_str()));
    assert!(!fnodes.contains(&mid.fnode.as_str()));
}

// ── mutation ──────────────────────────────────────────────────────────────────

#[test]
fn test_direct_dependency_mutation_uses_graph_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "text", "src");
    src.save().unwrap();
    let dep1 = make_node(root, "Dep1", "text", "dep1");
    dep1.save().unwrap();
    let dep2 = make_node(root, "Dep2", "text", "dep2");
    dep2.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();

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
        graph.direct_dependency_fnodes().unwrap(),
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
        graph.direct_dependency_fnodes().unwrap(),
        vec![dep2.fnode.clone()]
    );

    // Verify file was updated
    let reloaded = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(reloaded.depens, vec![dep2.fnode.clone()]);
}

#[test]
fn test_add_direct_dependencies_rejects_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "text", "src");
    src.save().unwrap();
    let dep = make_node(root, "Dep", "text", "dep");
    dep.save().unwrap();

    // src → dep
    let mut graph_src = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let (added, _, _) = graph_src
        .add_direct_dependencies(vec![dep.fnode.clone()])
        .unwrap();
    assert_eq!(added, vec![dep.fnode.clone()]);

    // dep → src would create a cycle — should be rejected
    let mut graph_dep = DepGraph::new(root.to_path_buf(), &dep.fnode).unwrap();
    let result = graph_dep.add_direct_dependencies(vec![src.fnode.clone()]);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle"),
        "expected cycle error, got: {err_msg}"
    );

    // dep's file should NOT have been modified
    let reloaded = MdocNode::load(root, &dep.path).unwrap();
    assert!(reloaded.depens.is_empty());
}

#[test]
fn test_create_and_add_dependency_no_side_effects_on_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root_dir = dir.path();

    let root_node = make_node(root_dir, "Root", "text", "root");
    root_node.save().unwrap();

    // Build a new node whose @dep already points back at root — this would form
    // the cycle root → new_node → root the moment we add root → new_node.
    let mut new_node = make_node(root_dir, "New", "text", "new");
    new_node.add_dependency(&root_node.fnode);
    let new_path = new_node.path.clone();
    let new_fnode = new_node.fnode.clone();

    let mut graph = DepGraph::new(root_dir.to_path_buf(), &root_node.fnode).unwrap();
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
    let search_results = graph.cache.search(&new_fnode[..8]).unwrap();
    assert!(
        search_results.is_empty(),
        "new node must not be indexed after failure"
    );
}

#[test]
fn test_create_root_rejects_duplicate_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // First create succeeds and establishes the fnode in the index.
    let (graph, _) =
        DepGraph::create_root(root.to_path_buf(), "first", "First", None, None).unwrap();
    let existing_fnode = graph
        .cache
        .resolve_ref("first.mdoc", None)
        .map(|(f, _, _)| f)
        .unwrap_or_else(|_| {
            // Fall back: read fnode from indexed nodes
            graph
                .cache
                .search("First")
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
                .0
        });

    // Second create with the same fnode must fail before writing anything.
    let second_path = root.join("second.mdoc");
    let result = DepGraph::create_root(
        root.to_path_buf(),
        "second",
        "Second",
        Some(&existing_fnode),
        None,
    );

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
    existing.save().unwrap();
    let existing_fnode = existing.fnode.clone();

    // Attempt to create a new root with the same fnode — bootstrap must surface
    // the on-disk file so the duplicate check fires before any write.
    let second_path = root.join("second.mdoc");
    let result = DepGraph::create_root(
        root.to_path_buf(),
        "second",
        "Second",
        Some(&existing_fnode),
        None,
    );

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
    root_node.save().unwrap();

    // Index the root so the fnode is known.
    let mut graph = DepGraph::new(root_dir.to_path_buf(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.to_path_buf(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.to_path_buf(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.to_path_buf(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

    let mut new_node = make_node(root_dir, "Outside", "text", "outside");
    // Point the path to a location outside the workspace.
    new_node.path = outside_dir.path().join("outside.mdoc");
    let outside_path = new_node.path.clone();

    let result = graph.create_and_add_dependency(new_node);

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("under mdoc root"),
        "expected out-of-workspace error, got: {err_msg}"
    );
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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.clone(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.clone(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("under mdoc root"),
        "expected out-of-workspace error for .. escape, got: {err_msg}"
    );
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
    let result = DepGraph::create_root(root_dir.clone(), &file_target, "Escape", None, None);

    assert!(result.is_err());
    let err_msg = result.err().expect("expected error").to_string();
    assert!(
        err_msg.contains("under mdoc root"),
        "expected out-of-workspace error for .. escape, got: {err_msg}"
    );
    let escaped_path = root_dir
        .parent()
        .unwrap()
        .join(format!("{stem}-escaped.mdoc"));
    assert!(!escaped_path.exists(), "escaped file must not be written");
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
    root_node.save().unwrap();
    let mut graph = DepGraph::new(root_dir.clone(), &root_node.fnode).unwrap();
    graph.cache.upsert_path(&root_node.path).unwrap();

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
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("under mdoc root"),
        "expected out-of-workspace error, got: {err_msg}"
    );
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
    let result = DepGraph::create_root(root_dir.clone(), &file_target, "Escape", None, None);

    assert!(result.is_err(), "symlink-dotdot escape must be rejected");
    let err_msg = result.err().expect("expected error").to_string();
    assert!(
        err_msg.contains("under mdoc root"),
        "expected out-of-workspace error, got: {err_msg}"
    );
    let escaped_name = format!("{stem}-sym-root-escaped.mdoc");
    let potential_escape = outside_canonical.parent().unwrap().join(&escaped_name);
    assert!(
        !potential_escape.exists(),
        "escaped file must not be written"
    );
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
    let result = DepGraph::create_root(root.to_path_buf(), ".", "New", Some(fnode), None);

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

// ── scan_all ──────────────────────────────────────────────────────────────────

#[test]
fn test_scan_all_builds_global_graph() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let leaf = make_node(root, "Leaf", "text", "leaf");
    leaf.save().unwrap();
    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&leaf.fnode);
    src.save().unwrap();
    let other = make_node(root, "Other", "text", "other");
    other.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    graph.scan_all().unwrap();

    let node_keys: std::collections::HashSet<&str> = graph
        .state
        .nodes_by_fnode
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert!(node_keys.contains(src.fnode.as_str()));
    assert!(node_keys.contains(leaf.fnode.as_str()));
    assert!(node_keys.contains(other.fnode.as_str()));

    assert_eq!(graph.state.dep_graph[&src.fnode], vec![leaf.fnode.clone()]);
    assert_eq!(graph.state.dep_graph[&leaf.fnode], Vec::<String>::new());
    assert_eq!(graph.state.dep_graph[&other.fnode], Vec::<String>::new());

    let items = graph.dependency_items(-1).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fnode, leaf.fnode);
}

// ── global_root_items ─────────────────────────────────────────────────────────

#[test]
fn test_global_root_items_include_unreferenced_valid_and_invalid_nodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    let leaf = make_node(root, "Leaf", "text", "leaf");
    leaf.save().unwrap();

    let mut root_valid = make_node(root, "Root Valid", "text", "root_valid");
    root_valid.add_dependency(&leaf.fnode);
    root_valid.save().unwrap();

    let other_root = make_node(root, "Other Root", "text", "other_root");
    other_root.save().unwrap();

    let bad_root = make_node(root, "Broken Root", "text", "bad_root");
    bad_root.save().unwrap();
    make_invalid(&bad_root.path);

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.bootstrap_if_needed().unwrap();
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
    bad.save().unwrap();
    make_invalid(&bad.path);

    let mut a = make_node(root, "Cycle A", "text", "a");
    a.save().unwrap();
    let mut b = make_node(root, "Cycle B", "text", "b");
    b.save().unwrap();
    a.add_dependency(&b.fnode);
    a.save().unwrap();
    b.add_dependency(&a.fnode);
    b.save().unwrap();

    let mut src = make_node(root, "Source", "text", "src");
    src.add_dependency("missing-target-001");
    src.add_dependency(&bad.fnode);
    src.save().unwrap();

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.bootstrap_if_needed().unwrap();
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

// ── invalid placeholder ───────────────────────────────────────────────────────

#[test]
fn test_dependency_items_show_invalid_placeholder() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Broken Dep", "text", "dep");
    dep.save().unwrap();
    make_invalid(&dep.path);

    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let items = graph.dependency_items(1).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fnode, dep.fnode);
    assert_eq!(items[0].title, "<invalid>");
    assert!(graph.state.invalid_fnodes.contains(&dep.fnode));

    let issue = graph.issue_for_fnode(&dep.fnode).unwrap().unwrap();
    assert_eq!(issue.kind, IssueKind::Invalid);
}

#[test]
fn test_dependency_items_show_duplicate_fnode_placeholder() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    fs::write(root.join("dup-a.mdoc"), "@fnode: dup-node\n@title: Dup A\n").unwrap();
    fs::write(root.join("dup-b.mdoc"), "@fnode: dup-node\n@title: Dup B\n").unwrap();

    let mut src = make_node(root, "Src", "text", "src");
    src.add_dependency("dup-node");
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let items = graph.dependency_items(1).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fnode, "dup-node");
    assert_eq!(items[0].title, "<invalid>");

    let issue = graph.issue_for_fnode("dup-node").unwrap().unwrap();
    assert!(
        issue.error.contains("duplicate fnode 'dup-node'"),
        "unexpected error: {}",
        issue.error
    );
}
