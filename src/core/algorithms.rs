use std::collections::{HashMap, HashSet, VecDeque};

/// Compute every node's height from a pre-loaded dependency graph: leaves are 0,
/// and every acyclic parent is one greater than its deepest dependency.
/// Nodes in cycles retain the height accumulated before the cycle closes.
pub fn all_topo_depths(graph: &HashMap<String, Vec<String>>) -> HashMap<String, u32> {
    if graph.is_empty() {
        return HashMap::new();
    }

    let mut reverse: HashMap<&str, Vec<&str>> =
        graph.keys().map(|fnode| (fnode.as_str(), vec![])).collect();
    for (source, targets) in graph {
        for target in targets {
            if graph.contains_key(target.as_str()) {
                reverse
                    .entry(target.as_str())
                    .or_default()
                    .push(source.as_str());
            }
        }
    }

    let mut remaining: HashMap<&str, usize> = graph
        .iter()
        .map(|(fnode, targets)| {
            let count = targets
                .iter()
                .filter(|target| graph.contains_key(target.as_str()))
                .count();
            (fnode.as_str(), count)
        })
        .collect();
    let mut depths: HashMap<&str, u32> = graph.keys().map(|fnode| (fnode.as_str(), 0)).collect();
    let mut queue: std::collections::VecDeque<&str> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(fnode, _)| *fnode)
        .collect();

    while let Some(fnode) = queue.pop_front() {
        let depth = depths[fnode];
        for parent in reverse.get(fnode).into_iter().flatten() {
            if let Some(parent_depth) = depths.get_mut(parent) {
                *parent_depth = (*parent_depth).max(depth + 1);
            }
            let count = remaining.entry(parent).or_default();
            *count = count.saturating_sub(1);
            if *count == 0 {
                queue.push_back(parent);
            }
        }
    }

    depths
        .into_iter()
        .map(|(fnode, depth)| (fnode.to_string(), depth))
        .collect()
}

/// Compute each selected node's weakly connected-component size.
///
/// Edges are treated as undirected, and graph nodes outside `members` are ignored.
pub fn weak_component_sizes(
    graph: &HashMap<String, Vec<String>>,
    members: &HashSet<String>,
) -> HashMap<String, u32> {
    let mut adjacency: HashMap<&str, HashSet<&str>> = members
        .iter()
        .map(|member| (member.as_str(), HashSet::new()))
        .collect();
    for (source, targets) in graph {
        if !members.contains(source) {
            continue;
        }
        for target in targets {
            if !members.contains(target) {
                continue;
            }
            adjacency
                .entry(source.as_str())
                .or_default()
                .insert(target.as_str());
            adjacency
                .entry(target.as_str())
                .or_default()
                .insert(source.as_str());
        }
    }

    let mut sizes = HashMap::new();
    let mut seen = HashSet::new();
    for start in members {
        if seen.contains(start.as_str()) {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::from([start.as_str()]);
        while let Some(node) = queue.pop_front() {
            if !seen.insert(node) {
                continue;
            }
            component.push(node);
            for neighbor in adjacency.get(node).into_iter().flatten() {
                if !seen.contains(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        let size = component.len() as u32;
        sizes.extend(component.into_iter().map(|node| (node.to_string(), size)));
    }
    sizes
}

/// Compute all strongly connected components using Kosaraju's algorithm.
/// Returns a list of components; each component is a list of fnodes.
pub fn strongly_connected_components(dep_graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    strongly_connected_component_refs(dep_graph)
        .into_iter()
        .map(|component| component.into_iter().map(str::to_string).collect())
        .collect()
}

fn strongly_connected_component_refs(dep_graph: &HashMap<String, Vec<String>>) -> Vec<Vec<&str>> {
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
    let mut components: Vec<Vec<&str>> = Vec::new();

    for &start in finish_order.iter().rev() {
        if visited2.contains(start) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack: Vec<&str> = vec![start];
        while let Some(node) = stack.pop() {
            if visited2.contains(node) {
                continue;
            }
            visited2.insert(node);
            component.push(node);
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

/// Return one representative cycle from each cyclic strongly connected component.
pub fn representative_cycles(dep_graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut cycles: Vec<Vec<String>> = strongly_connected_component_refs(dep_graph)
        .into_iter()
        .filter_map(|component| representative_cycle(dep_graph, &component))
        .collect();
    cycles.sort();
    cycles
}

/// Find a representative cycle within a strongly connected component.
/// Returns the cycle as a list of fnodes (first == last), or None if no cycle.
fn representative_cycle(
    dep_graph: &HashMap<String, Vec<String>>,
    component: &[&str],
) -> Option<Vec<String>> {
    if component.is_empty() {
        return None;
    }
    if component.len() == 1 {
        let fnode = component[0];
        if dep_graph
            .get(fnode)
            .map(|dependencies| dependencies.iter().any(|dependency| dependency == fnode))
            .unwrap_or(false)
        {
            return Some(vec![fnode.to_string(), fnode.to_string()]);
        }
        return None;
    }

    let component_set: HashSet<&str> = component.iter().copied().collect();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut path: Vec<&str> = Vec::new();
    let mut path_idx: HashMap<&str, usize> = HashMap::new();

    // Iterate in sorted order to match Python spec behavior
    let mut sorted_component = component.to_vec();
    sorted_component.sort();

    for &start in &sorted_component {
        if visited.contains(start) {
            continue;
        }

        let mut dfs_stack: Vec<(&str, usize)> = vec![(start, 0)];
        visited.insert(start);
        path_idx.insert(start, path.len());
        path.push(start);

        while let Some(frame) = dfs_stack.last_mut() {
            let fnode = frame.0;
            let children = dep_graph.get(fnode).map(Vec::as_slice).unwrap_or_default();

            // Find next child within the component
            let mut found = false;
            while frame.1 < children.len() {
                let child = children[frame.1].as_str();
                frame.1 += 1;
                if !component_set.contains(child) {
                    continue;
                }
                if !visited.contains(child) {
                    visited.insert(child);
                    path_idx.insert(child, path.len());
                    path.push(child);
                    dfs_stack.push((child, 0));
                    found = true;
                    break;
                } else if path_idx.contains_key(child) {
                    // Back edge within current path: cycle found
                    let start_idx = path_idx[child];
                    let mut cycle = path[start_idx..]
                        .iter()
                        .map(|node| node.to_string())
                        .collect::<Vec<_>>();
                    cycle.push(child.to_string());
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
