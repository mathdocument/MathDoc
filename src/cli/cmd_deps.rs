use anyhow::Result;
use std::collections::HashSet;

use crate::core::DependencyItem;
use crate::core::IssueKind;
use crate::depgraph::DepGraph;
use crate::mdoc::MdocNode;

use super::{
    cwd, eprintln_warn, fmt_item, open_cache, print_cycle, print_dep_report,
    print_missing_with_referrers, require_mdcroot, BLD, RED, RST,
};

// ── cmd: dep show ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_show(source: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.refresh_reachable_from_path(&src_path, depth);
    }
    let source_item = cache
        .resolve_ref(&source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    let report = cache.dependency_report(&source_item.fnode, depth)?;
    print_dep_report(
        "source",
        &source_item,
        "depens",
        &report.items,
        &report.issues_by_fnode,
    );
    let missing: Vec<_> = report
        .issues_by_fnode
        .values()
        .filter(|i| i.kind == IssueKind::Missing)
        .cloned()
        .collect();
    print_missing_with_referrers(&missing, &cache);
    print_cycles_if_any(&report.cycles, &cache);
    Ok(0)
}

// ── cmd: dep leaf ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_leaf(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.refresh_reachable_from_path(&src_path, -1);
    }
    let source_item = cache
        .resolve_ref(&source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    let report = cache.leaf_dependency_report(&source_item.fnode)?;
    print_dep_report(
        "source",
        &source_item,
        "leaves",
        &report.items,
        &report.issues_by_fnode,
    );
    let missing: Vec<_> = report
        .issues_by_fnode
        .values()
        .filter(|i| i.kind == IssueKind::Missing)
        .cloned()
        .collect();
    print_missing_with_referrers(&missing, &cache);
    print_cycles_if_any(&report.cycles, &cache);
    Ok(0)
}

// ── cmd: dep add ──────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_add(source: String, query: String, max_results: usize) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot)?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;

    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(anyhow::anyhow!("query cannot be empty"));
    }
    let all_rows = graph.cache.search(&q)?;
    let existing_fnodes: HashSet<String> = {
        let direct = graph.direct_dependency_fnodes().unwrap_or_default();
        std::iter::once(source_item.fnode.clone())
            .chain(direct)
            .collect()
    };
    let candidates: Vec<_> = all_rows
        .iter()
        .filter(|(f, _, _)| !existing_fnodes.contains(f))
        .take(max_results)
        .collect();

    if candidates.is_empty() {
        println!("No results for '{q}'.");
        if !dialoguer::Confirm::new()
            .with_prompt("Create a new note?")
            .default(false)
            .interact()?
        {
            println!("Canceled");
            return Ok(0);
        }
        let title: String = dialoguer::Input::new()
            .with_prompt("Title")
            .default(q.clone())
            .interact_text()?;
        let mdcroot = graph.mdcroot.clone();
        let mut new_node = MdocNode::new_at_path(&mdcroot, &mdcroot, &title);
        let short = &new_node.fnode[..8];
        let file_input: String = dialoguer::Input::new()
            .with_prompt(format!("File [{short}…]"))
            .allow_empty(true)
            .interact_text()?;
        let filename = if file_input.trim().is_empty() {
            format!("{}.mdoc", new_node.fnode)
        } else {
            format!("{}.mdoc", file_input.trim())
        };
        new_node.path = mdcroot.join(&filename);
        new_node.save()?;
        graph.cache.upsert_path(&new_node.path)?;
        let new_fnode = new_node.fnode.clone();
        let node_path = new_node.path.clone();
        graph
            .state
            .nodes_by_fnode
            .insert(new_fnode.clone(), new_node);
        graph.state.dep_graph.entry(new_fnode.clone()).or_default();
        let (added, _, _) = graph.add_direct_dependencies(vec![new_fnode.clone()])?;
        let root_path = graph.root_path()?;
        if let Err(e) = graph.cache.upsert_path(&root_path) {
            eprintln_warn(&format!("index update failed: {e}"));
        }
        if !added.is_empty() {
            let rel = crate::workspace::to_rel_path(&graph.mdcroot, &node_path);
            println!(
                "created and added  {}",
                fmt_item(&new_fnode, &title, &rel, false)
            );
        }
        return Ok(0);
    }

    let items: Vec<(&str, &str, &str, bool)> = candidates
        .iter()
        .map(|(f, t, p)| (f.as_str(), t.as_str(), p.as_str(), false))
        .collect();
    let selected = match select_multi("Select dependencies to add", &items)? {
        None => {
            println!("Canceled");
            return Ok(0);
        }
        Some(v) if v.is_empty() => {
            println!("No dependencies selected");
            return Ok(0);
        }
        Some(v) => v,
    };

    let selected_fnodes: Vec<String> = selected.iter().map(|&i| candidates[i].0.clone()).collect();
    let (added, _, _) = graph.add_direct_dependencies(selected_fnodes)?;

    let root_path = graph.root_path()?;
    if let Err(e) = graph.cache.upsert_path(&root_path) {
        eprintln_warn(&format!("index update failed: {e}"));
    }

    println!(
        "added {BLD}{}{RST} dep{}",
        added.len(),
        if added.len() == 1 { "" } else { "s" }
    );
    for fnode in &added {
        let label = candidates
            .iter()
            .find(|(f, _, _)| f == fnode)
            .map(|(f, t, p)| fmt_item(f, t, p, false))
            .unwrap_or_else(|| fnode.clone());
        println!("  + {label}");
    }
    Ok(0)
}

// ── cmd: dep rm ───────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_rm(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot)?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;
    let dep_items = graph.direct_dependency_items()?;

    if dep_items.is_empty() {
        println!(
            "source  {}",
            fmt_item(
                &source_item.fnode,
                &source_item.title,
                &source_item.rel_path,
                false
            )
        );
        println!("  No dependencies to remove");
        return Ok(0);
    }

    let items: Vec<(&str, &str, &str, bool)> = dep_items
        .iter()
        .map(|item| {
            let broken = graph.is_broken_fnode(&item.fnode);
            (
                item.fnode.as_str(),
                item.title.as_str(),
                item.rel_path.as_str(),
                broken,
            )
        })
        .collect();

    let selected = match select_multi("Select dependencies to remove", &items)? {
        None => {
            println!("Canceled");
            return Ok(0);
        }
        Some(v) if v.is_empty() => {
            println!("No dependencies selected");
            return Ok(0);
        }
        Some(v) => v,
    };

    let selected_fnodes: Vec<String> = selected
        .iter()
        .map(|&i| dep_items[i].fnode.clone())
        .collect();
    let removed = graph.remove_direct_dependencies(selected_fnodes)?;

    let root_path = graph.root_path()?;
    if let Err(e) = graph.cache.upsert_path(&root_path) {
        eprintln_warn(&format!("index update failed: {e}"));
    }

    println!(
        "removed {BLD}{}{RST} dep{}",
        removed.len(),
        if removed.len() == 1 { "" } else { "s" }
    );
    for fnode in &removed {
        let label = dep_items
            .iter()
            .find(|item| &item.fnode == fnode)
            .map(|item| fmt_item(&item.fnode, &item.title, &item.rel_path, false))
            .unwrap_or_else(|| fnode.clone());
        println!("  - {label}");
    }
    Ok(0)
}

// ── cmd: dep refs ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_refs(target: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&target, Some(&cwd())) {
        let _ = cache.upsert_path(&src_path);
    }
    let (fnode, title, path) = cache.resolve_ref(&target, Some(&cwd()))?;
    let rel_path = crate::workspace::to_rel_path(&mdcroot, &path);
    let target_item = DependencyItem {
        depth: 0,
        fnode: fnode.clone(),
        title,
        rel_path,
    };
    let ref_items = cache.referrer_items(&fnode, depth)?;
    print_dep_report(
        "target",
        &target_item,
        "refers",
        &ref_items,
        &std::collections::HashMap::new(),
    );
    Ok(0)
}

// ── Cycle display helper ──────────────────────────────────────────────────────

fn print_cycles_if_any(cycles: &[Vec<String>], cache: &crate::indcache::IndCache) {
    if cycles.is_empty() {
        return;
    }
    println!("   {RED}cycles ({}):{RST}", cycles.len());

    for cycle in cycles {
        let fnode_refs: Vec<&str> = cycle.iter().map(|s| s.as_str()).collect();
        let label_map = cache.lookup_by_fnode(&fnode_refs).unwrap_or_default();
        print_cycle(cycle, &label_map);
    }
}

// ── Interactive multi-select (dialoguer) ─────────────────────────────────────

/// Presents an interactive checkbox list.
/// Returns `None` on cancel (Esc/Ctrl-C), `Some(sorted_indices)` on Enter.
fn select_multi(prompt: &str, items: &[(&str, &str, &str, bool)]) -> Result<Option<Vec<usize>>> {
    if items.is_empty() {
        return Ok(Some(vec![]));
    }
    let labels: Vec<String> = items
        .iter()
        .map(|(fnode, title, rel_path, broken)| fmt_item(fnode, title, rel_path, *broken))
        .collect();
    Ok(dialoguer::MultiSelect::new()
        .with_prompt(prompt)
        .items(&labels)
        .interact_opt()?)
}
