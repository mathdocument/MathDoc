use std::collections::{HashMap, HashSet};

/// Find a cycle in the dependency graph. Returns the cycle as a list of fnodes
/// (first element == last element), or None if no cycle exists.
/// If root_fnode is given, only searches from that node.
pub fn find_cycle(
    dep_graph: &HashMap<String, Vec<String>>,
    root_fnode: Option<&str>,
) -> Option<Vec<String>> {
    // 0 = unvisited, 1 = gray (in current path), 2 = black (done)
    let mut color: HashMap<String, u8> = HashMap::new();
    let mut path: Vec<String> = Vec::new();
    let mut path_idx: HashMap<String, usize> = HashMap::new();

    let roots: Vec<&str> = match root_fnode {
        Some(r) => vec![r],
        None => dep_graph.keys().map(String::as_str).collect(),
    };

    for start in roots {
        if !dep_graph.contains_key(start) {
            continue;
        }
        if color.get(start).copied().unwrap_or(0) != 0 {
            continue;
        }

        // Iterative DFS: each frame is (fnode, next_child_index)
        let mut dfs_stack: Vec<(String, usize)> = vec![(start.to_string(), 0)];
        color.insert(start.to_string(), 1);
        path_idx.insert(start.to_string(), path.len());
        path.push(start.to_string());

        while let Some(frame) = dfs_stack.last_mut() {
            let fnode = frame.0.clone();
            let children = dep_graph.get(&fnode).map(Vec::as_slice).unwrap_or_default();

            if frame.1 < children.len() {
                let child = children[frame.1].clone();
                frame.1 += 1;

                match color.get(&child).copied().unwrap_or(0) {
                    0 => {
                        // Unvisited: push onto DFS stack
                        color.insert(child.clone(), 1);
                        path_idx.insert(child.clone(), path.len());
                        path.push(child.clone());
                        dfs_stack.push((child, 0));
                    }
                    1 => {
                        // Back edge: cycle found
                        let start_idx = path_idx[&child];
                        let mut cycle = path[start_idx..].to_vec();
                        cycle.push(child);
                        return Some(cycle);
                    }
                    _ => {} // already done
                }
            } else {
                // All children processed: pop and mark done
                dfs_stack.pop();
                path.pop();
                path_idx.remove(&fnode);
                color.insert(fnode, 2);
            }
        }
    }
    None
}

/// Return nodes in dependency-first order (post-order DFS): dependencies
/// before the nodes that depend on them.
pub fn topo_dependencies_first(
    dep_graph: &HashMap<String, Vec<String>>,
    root_fnode: &str,
) -> Vec<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();

    // Iterative post-order DFS: (fnode, entered)
    // entered=false: first visit, push children; entered=true: emit node
    let mut stack: Vec<(String, bool)> = vec![(root_fnode.to_string(), false)];

    while let Some((fnode, entered)) = stack.pop() {
        if entered {
            order.push(fnode);
        } else if !visited.contains(&fnode) {
            visited.insert(fnode.clone());
            stack.push((fnode.clone(), true));
            let children = dep_graph.get(&fnode).map(Vec::as_slice).unwrap_or_default();
            // Push in reverse so left-most child is processed first
            for child in children.iter().rev() {
                if !visited.contains(child) {
                    stack.push((child.clone(), false));
                }
            }
        }
    }
    order
}

/// Compute all strongly connected components using Kosaraju's algorithm.
/// Returns a list of components; each component is a list of fnodes.
pub fn strongly_connected_components(dep_graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    // Step 1: DFS on original graph; collect finish order
    let mut visited: HashSet<&str> = HashSet::new();
    let mut finish_order: Vec<&str> = Vec::new();

    for start in dep_graph.keys() {
        if visited.contains(start.as_str()) {
            continue;
        }
        let mut stack: Vec<(&str, bool)> = vec![(start.as_str(), false)];
        while let Some((node, done)) = stack.pop() {
            if done {
                finish_order.push(node);
            } else if !visited.contains(node) {
                visited.insert(node);
                stack.push((node, true));
                let children = dep_graph.get(node).map(Vec::as_slice).unwrap_or_default();
                for child in children.iter().rev() {
                    if !visited.contains(child.as_str()) {
                        stack.push((child.as_str(), false));
                    }
                }
            }
        }
    }

    // Step 2: Build transpose graph
    let mut transpose: HashMap<&str, Vec<&str>> = HashMap::new();
    for (src, dsts) in dep_graph {
        for dst in dsts {
            transpose
                .entry(dst.as_str())
                .or_default()
                .push(src.as_str());
        }
    }

    // Step 3: DFS on transpose in reverse finish order; each tree = one SCC
    let mut visited2: HashSet<&str> = HashSet::new();
    let mut components: Vec<Vec<String>> = Vec::new();

    for &start in finish_order.iter().rev() {
        if visited2.contains(start) {
            continue;
        }
        let mut component: Vec<String> = Vec::new();
        let mut stack: Vec<&str> = vec![start];
        while let Some(node) = stack.pop() {
            if visited2.contains(node) {
                continue;
            }
            visited2.insert(node);
            component.push(node.to_string());
            let children = transpose.get(node).map(Vec::as_slice).unwrap_or_default();
            for &child in children.iter().rev() {
                if !visited2.contains(child) {
                    stack.push(child);
                }
            }
        }
        components.push(component);
    }

    components
}

/// Check whether a component contains a cycle.
pub fn component_has_cycle(dep_graph: &HashMap<String, Vec<String>>, component: &[String]) -> bool {
    if component.len() > 1 {
        return true;
    }
    match component.first() {
        None => false,
        Some(fnode) => dep_graph
            .get(fnode)
            .map(|deps| deps.contains(fnode))
            .unwrap_or(false),
    }
}

/// Find a representative cycle within a strongly connected component.
/// Returns the cycle as a list of fnodes (first == last), or None if no cycle.
pub fn representative_cycle(
    dep_graph: &HashMap<String, Vec<String>>,
    component: &[String],
) -> Option<Vec<String>> {
    if component.is_empty() {
        return None;
    }
    if component.len() == 1 {
        let fnode = &component[0];
        if dep_graph
            .get(fnode)
            .map(|d| d.contains(fnode))
            .unwrap_or(false)
        {
            return Some(vec![fnode.clone(), fnode.clone()]);
        }
        return None;
    }

    let component_set: HashSet<&str> = component.iter().map(String::as_str).collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut path: Vec<String> = Vec::new();
    let mut path_idx: HashMap<String, usize> = HashMap::new();

    // Iterate in sorted order to match Python spec behavior
    let mut sorted_component = component.to_vec();
    sorted_component.sort();

    for start in &sorted_component {
        if visited.contains(start) {
            continue;
        }

        let mut dfs_stack: Vec<(String, usize)> = vec![(start.clone(), 0)];
        visited.insert(start.clone());
        path_idx.insert(start.clone(), path.len());
        path.push(start.clone());

        while let Some(frame) = dfs_stack.last_mut() {
            let fnode = frame.0.clone();
            let children = dep_graph.get(&fnode).map(Vec::as_slice).unwrap_or_default();

            // Find next child within the component
            let mut found = false;
            while frame.1 < children.len() {
                let child = &children[frame.1];
                frame.1 += 1;
                if !component_set.contains(child.as_str()) {
                    continue;
                }
                if !visited.contains(child) {
                    visited.insert(child.clone());
                    path_idx.insert(child.clone(), path.len());
                    path.push(child.clone());
                    dfs_stack.push((child.clone(), 0));
                    found = true;
                    break;
                } else if path_idx.contains_key(child) {
                    // Back edge within current path: cycle found
                    let start_idx = path_idx[child];
                    let mut cycle = path[start_idx..].to_vec();
                    cycle.push(child.clone());
                    return Some(cycle);
                }
            }

            if !found {
                dfs_stack.pop();
                path.pop();
                path_idx.remove(&fnode);
            }
        }
    }
    None
}
