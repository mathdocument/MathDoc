use std::collections::HashMap;

use mathdoc::core::{
    component_has_cycle, find_cycle, representative_cycle, strongly_connected_components,
    topo_dependencies_first,
};

fn graph(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    edges
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|s| s.to_string()).collect()))
        .collect()
}

#[test]
fn find_cycle_no_cycle() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &[])]);
    assert!(find_cycle(&g, None).is_none());
}

#[test]
fn find_cycle_self_loop() {
    let g = graph(&[("a", &["a"])]);
    let cycle = find_cycle(&g, None).unwrap();
    assert_eq!(cycle, vec!["a", "a"]);
}

#[test]
fn find_cycle_simple() {
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let cycle = find_cycle(&g, None).unwrap();
    assert_eq!(cycle.first(), cycle.last());
    assert!(cycle.len() >= 2);
}

#[test]
fn find_cycle_from_root_not_in_cycle() {
    // a -> b -> c -> b (cycle), but searching from a should find it
    let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["b"])]);
    let cycle = find_cycle(&g, Some("a")).unwrap();
    assert!(cycle.contains(&"b".to_string()));
}

#[test]
fn find_cycle_root_outside_cycle() {
    // a -> b, c -> c; searching from a should not find c's cycle
    let g = graph(&[("a", &["b"]), ("b", &[]), ("c", &["c"])]);
    assert!(find_cycle(&g, Some("a")).is_none());
}

#[test]
fn topo_simple() {
    let g = graph(&[("a", &["b", "c"]), ("b", &["c"]), ("c", &[])]);
    let order = topo_dependencies_first(&g, "a");
    let c_pos = order.iter().position(|x| x == "c").unwrap();
    let b_pos = order.iter().position(|x| x == "b").unwrap();
    let a_pos = order.iter().position(|x| x == "a").unwrap();
    assert!(c_pos < b_pos);
    assert!(b_pos < a_pos);
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
