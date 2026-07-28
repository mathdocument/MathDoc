use std::collections::HashMap;

use mathdoc::core::{
    all_topo_depths, representative_cycles, strongly_connected_components, weak_component_sizes,
};

fn graph(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    edges
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|s| s.to_string()).collect()))
        .collect()
}

#[test]
fn scc_no_cycle() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &[])]);
    let sccs = strongly_connected_components(&g);
    assert!(sccs.iter().all(|c| c.len() == 1));
}

#[test]
fn scc_with_cycle() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let sccs = strongly_connected_components(&g);
    let cyclic: Vec<_> = sccs.iter().filter(|c| c.len() > 1).collect();
    assert_eq!(cyclic.len(), 1);
    assert_eq!(cyclic[0].len(), 3);
}

#[test]
fn representative_cycles_include_self_loop() {
    let g = graph(&[("a", &["a"])]);
    assert_eq!(representative_cycles(&g), vec![vec!["a", "a"]]);
}

#[test]
fn representative_cycles_include_multi_node_cycle() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let cycle = representative_cycles(&g).pop().unwrap();
    assert_eq!(cycle.first(), cycle.last());
    assert!(cycle.len() >= 2);
}

#[test]
fn representative_cycles_exclude_acyclic_components() {
    let g = graph(&[("a", &["b"]), ("b", &[])]);
    assert!(representative_cycles(&g).is_empty());
}

#[test]
fn topo_depths_are_computed_without_database_state() {
    let g = graph(&[
        ("root", &["short", "branch"]),
        ("short", &["leaf"]),
        ("branch", &["middle"]),
        ("middle", &["leaf"]),
        ("leaf", &[]),
    ]);

    let depths = all_topo_depths(&g);

    assert_eq!(depths["leaf"], 0);
    assert_eq!(depths["middle"], 1);
    assert_eq!(depths["branch"], 2);
    assert_eq!(depths["root"], 3);
}

#[test]
fn weak_component_sizes_are_computed_without_database_state() {
    let g = graph(&[
        ("a", &["b", "outside"]),
        ("b", &["c"]),
        ("c", &[]),
        ("isolated", &[]),
        ("outside", &["other-outside"]),
    ]);
    let members = ["a", "b", "c", "isolated"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let sizes = weak_component_sizes(&g, &members);

    assert_eq!(sizes["a"], 3);
    assert_eq!(sizes["b"], 3);
    assert_eq!(sizes["c"], 3);
    assert_eq!(sizes["isolated"], 1);
    assert!(!sizes.contains_key("outside"));
}

#[test]
fn weak_component_sizes_treat_edges_as_undirected() {
    let g = graph(&[("child", &[]), ("parent", &["child"])]);
    let members = ["child", "parent"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let sizes = weak_component_sizes(&g, &members);

    assert_eq!(sizes["child"], 2);
    assert_eq!(sizes["parent"], 2);
}
