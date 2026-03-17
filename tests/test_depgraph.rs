use std::fs;
use std::path::Path;

use mathdoc::compiler::CompilerRegistry;
use mathdoc::config::Config;
use mathdoc::core::IssueKind;
use mathdoc::depgraph::DepGraph;
use mathdoc::indcache::IndCache;
use mathdoc::mdoc::{MdocNode, SrcBlock};

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

fn default_registry() -> CompilerRegistry {
    CompilerRegistry::default_registry()
}

fn load_config(root: &Path) -> Config {
    Config::load(root).unwrap()
}

// ── from_ref ──────────────────────────────────────────────────────────────────

#[test]
fn test_from_ref_loads_root_graph() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Src", "natl", "src");
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

// ── dependency_items ─────────────────────────────────────────────────────────

#[test]
fn test_dependency_items_expand_incrementally_from_root_node() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep2 = make_node(root, "Dep2", "natl", "dep2");
    dep2.save().unwrap();

    let mut dep1 = make_node(root, "Dep1", "natl", "dep1");
    dep1.add_dependency(&dep2.fnode);
    dep1.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
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
    let dep = make_node(root, "Dep", "natl", "dep");
    dep.save().unwrap();
    let mut src = make_node(root, "Src", "natl", "src");
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

    let leaf_direct = make_node(root, "Leaf Direct", "natl", "leaf_direct");
    leaf_direct.save().unwrap();
    let leaf_shared = make_node(root, "Leaf Shared", "natl", "leaf_shared");
    leaf_shared.save().unwrap();

    let mut mid = make_node(root, "Mid", "natl", "mid");
    mid.add_dependency(&leaf_shared.fnode);
    mid.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
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

// ── eval_blocks ───────────────────────────────────────────────────────────────

#[test]
fn test_eval_blocks_runs_all_blocks() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let mut node = make_node(root, "Eval", "natl", "hello");
    node.blocks.push(SrcBlock {
        srctype: "py".to_string(),
        content: "print('hi')\n".to_string(),
        metadata: Default::default(),
    });
    node.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &node.fnode).unwrap();
    let results = graph
        .eval_blocks(1, &default_registry(), &load_config(root), None, None, None)
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].srctype, "natl");
    assert!(results[0].res.result);
    assert_eq!(results[0].res.stdout, "hello");
    assert_eq!(results[1].srctype, "py");
    assert!(results[1].res.result);
    assert_eq!(results[1].res.stdout.trim(), "hi");
}

#[test]
fn test_eval_blocks_merges_dependencies_with_default_depth() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep2 = make_node(root, "Dep2", "natl", "dep2");
    dep2.save().unwrap();

    let mut dep1 = make_node(root, "Dep1", "natl", "dep1");
    dep1.add_dependency(&dep2.fnode);
    dep1.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&dep1.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph
        .eval_blocks(1, &default_registry(), &load_config(root), None, None, None)
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].res.result);
    // depth=1 → only dep1 merged (dep2 is at depth 2, excluded)
    assert_eq!(results[0].res.stdout, "src\n\ndep1");
}

#[test]
fn test_eval_blocks_merges_dependencies_with_unbounded_depth() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep2 = make_node(root, "Dep2", "natl", "dep2");
    dep2.save().unwrap();

    let mut dep1 = make_node(root, "Dep1", "natl", "dep1");
    dep1.add_dependency(&dep2.fnode);
    dep1.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&dep1.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph
        .eval_blocks(
            -1,
            &default_registry(),
            &load_config(root),
            None,
            None,
            None,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].res.result);
    assert_eq!(results[0].res.stdout, "src\n\ndep1\n\ndep2");
}

#[test]
fn test_eval_blocks_respects_reverse_depens_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();
    fs::write(
        root.join(".mdc/config.toml"),
        "[src.natl]\nreverse_depens = false\n",
    )
    .unwrap();

    let dep2 = make_node(root, "Dep2", "natl", "dep2");
    dep2.save().unwrap();

    let mut dep1 = make_node(root, "Dep1", "natl", "dep1");
    dep1.add_dependency(&dep2.fnode);
    dep1.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&dep1.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph
        .eval_blocks(
            -1,
            &default_registry(),
            &load_config(root),
            None,
            None,
            None,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].res.result);
    assert_eq!(results[0].res.stdout, "dep2\n\ndep1\n\nsrc");
}

#[test]
fn test_eval_blocks_does_not_merge_when_depens_disabled() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Dep", "py", "print('dep')\n");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "py", "print('src')\n");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph
        .eval_blocks(
            -1,
            &default_registry(),
            &load_config(root),
            None,
            None,
            None,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].res.result);
    // py has depens=false by default → only root content
    assert_eq!(results[0].res.stdout.trim(), "src");
}

#[test]
fn test_eval_blocks_raises_on_dependency_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let mut dep = make_node(root, "Dep", "natl", "dep");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    // Create cycle: dep → src
    dep.add_dependency(&src.fnode);
    dep.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let err = expect_err(graph.eval_blocks(
        -1,
        &default_registry(),
        &load_config(root),
        None,
        None,
        None,
    ));
    assert!(
        err.to_string().contains("dependency cycle detected"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_eval_blocks_depth_zero_compiles_root_only_no_merge() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Dep", "natl", "dep");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph
        .eval_blocks(0, &default_registry(), &load_config(root), None, None, None)
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].res.stdout, "src");
}

// ── mutation ──────────────────────────────────────────────────────────────────

#[test]
fn test_direct_dependency_mutation_uses_graph_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "natl", "src");
    src.save().unwrap();
    let dep1 = make_node(root, "Dep1", "natl", "dep1");
    dep1.save().unwrap();
    let dep2 = make_node(root, "Dep2", "natl", "dep2");
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
fn test_add_direct_dependencies_allows_cycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Src", "natl", "src");
    src.save().unwrap();
    let dep = make_node(root, "Dep", "natl", "dep");
    dep.save().unwrap();

    // src → dep
    let mut graph_src = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let (added, _, _) = graph_src
        .add_direct_dependencies(vec![dep.fnode.clone()])
        .unwrap();
    assert_eq!(added, vec![dep.fnode.clone()]);

    // dep → src creates a cycle — this is now allowed
    let mut graph_dep = DepGraph::new(root.to_path_buf(), &dep.fnode).unwrap();
    let (added2, _, _) = graph_dep
        .add_direct_dependencies(vec![src.fnode.clone()])
        .unwrap();
    assert_eq!(added2, vec![src.fnode.clone()]);

    // dep's file should have been updated
    let reloaded = MdocNode::load(root, &dep.path).unwrap();
    assert_eq!(reloaded.depens, vec![src.fnode.clone()]);
}

// ── scan_all ──────────────────────────────────────────────────────────────────

#[test]
fn test_scan_all_builds_global_graph() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let leaf = make_node(root, "Leaf", "natl", "leaf");
    leaf.save().unwrap();
    let mut src = make_node(root, "Src", "natl", "src");
    src.add_dependency(&leaf.fnode);
    src.save().unwrap();
    let other = make_node(root, "Other", "natl", "other");
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

    let leaf = make_node(root, "Leaf", "natl", "leaf");
    leaf.save().unwrap();

    let mut root_valid = make_node(root, "Root Valid", "natl", "root_valid");
    root_valid.add_dependency(&leaf.fnode);
    root_valid.save().unwrap();

    let other_root = make_node(root, "Other Root", "natl", "other_root");
    other_root.save().unwrap();

    let bad_root = make_node(root, "Broken Root", "natl", "bad_root");
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

    let bad = make_node(root, "Broken Node", "natl", "bad");
    bad.save().unwrap();
    make_invalid(&bad.path);

    let mut a = make_node(root, "Cycle A", "natl", "a");
    a.save().unwrap();
    let mut b = make_node(root, "Cycle B", "natl", "b");
    b.save().unwrap();
    a.add_dependency(&b.fnode);
    a.save().unwrap();
    b.add_dependency(&a.fnode);
    b.save().unwrap();

    let mut src = make_node(root, "Source", "natl", "src");
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

    let dep = make_node(root, "Broken Dep", "natl", "dep");
    dep.save().unwrap();
    make_invalid(&dep.path);

    let mut src = make_node(root, "Src", "natl", "src");
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

    let mut src = make_node(root, "Src", "natl", "src");
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
