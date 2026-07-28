use std::collections::HashMap;

use mathdoc::core::{
    all_topo_depths, component_has_cycle, representative_cycle, strongly_connected_components,
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
fn component_has_cycle_self_loop() {
    let g = graph(&[("a", &["a"])]);
    assert!(component_has_cycle(&g, &["a".to_string()]));
}

#[test]
fn component_has_cycle_multi() {
    let g = graph(&[("a", &["b"]), ("b", &["a"])]);
    assert!(component_has_cycle(&g, &["a".to_string(), "b".to_string()]));
}

#[test]
fn representative_cycle_self_loop() {
    let g = graph(&[("a", &["a"])]);
    let cycle = representative_cycle(&g, &["a".to_string()]).unwrap();
    assert_eq!(cycle, vec!["a", "a"]);
}

#[test]
fn representative_cycle_multi() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let component = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let cycle = representative_cycle(&g, &component).unwrap();
    assert_eq!(cycle.first(), cycle.last());
    assert!(cycle.len() >= 2);
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
