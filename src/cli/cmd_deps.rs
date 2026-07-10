use anyhow::{bail, Result};
use std::collections::HashSet;
use std::path::Path;

use crate::core::{short_fnode, DependencyItem, DependencyTraversalReport, IssueKind};
use crate::depgraph::DepGraph;
use crate::indcache::IndCache;
use crate::mdocnode::MdocNode;

use super::{
    cwd, fmt_item, open_cache, print_cycles_if_any, print_dep_report, print_missing_with_referrers,
    require_mdcroot, BLD, RST,
};

// ── Shared dep display ────────────────────────────────────────────────────────

/// Print report sections and return the appropriate exit code (1 if cycles detected).
fn print_dep_report_sections(
    cache: &IndCache,
    source_item: &DependencyItem,
    count_label: &str,
    report: &DependencyTraversalReport,
) -> i32 {
    print_dep_report(
        "source",
        source_item,
        count_label,
        &report.items,
        &report.issues_by_fnode,
    );
    let missing: Vec<_> = report
        .issues_by_fnode
        .values()
        .filter(|i| i.kind == IssueKind::Missing)
        .cloned()
        .collect();
    print_missing_with_referrers(&missing, cache);
    print_cycles_if_any(&report.cycles, cache);
    if report.cycles.is_empty() {
        0
    } else {
        1
    }
}

// ── Shared setup for read commands ───────────────────────────────────────────

/// Open cache, discover changes, do a targeted refresh up to `refresh_depth`,
/// and resolve `source` to a `DependencyItem`. Used by dep show and dep leaf.
fn open_and_resolve_source(
    source: &str,
    refresh_depth: i32,
) -> Result<(IndCache, std::path::PathBuf, DependencyItem)> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(source, Some(&cwd())) {
        let _ = cache.refresh_reachable_from_path(&src_path, refresh_depth);
    }
    let source_item = cache
        .resolve_ref(source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    Ok((cache, mdcroot, source_item))
}

// ── cmd: dep show ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_show(source: String, depth: i32) -> Result<i32> {
    let (cache, _, source_item) = open_and_resolve_source(&source, depth)?;
    let report = cache.dependency_report(&source_item.fnode, depth)?;
    Ok(print_dep_report_sections(
        &cache,
        &source_item,
        "depens",
        &report,
    ))
}

// ── cmd: dep leaf ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_leaf(source: String) -> Result<i32> {
    let (cache, _, source_item) = open_and_resolve_source(&source, -1)?;
    let report = cache.leaf_dependency_report(&source_item.fnode)?;
    Ok(print_dep_report_sections(
        &cache,
        &source_item,
        "leaves",
        &report,
    ))
}

// ── cmd: dep add ──────────────────────────────────────────────────────────────

fn resolve_existing_target(
    cache: &mut IndCache,
    mdcroot: &Path,
    target_ref: &str,
) -> Result<DependencyItem> {
    let target_ref = target_ref.trim();
    if target_ref.is_empty() {
        bail!("target reference cannot be empty");
    }

    let (_, _, path) = cache.resolve_ref(target_ref, Some(&cwd()))?;
    if path.extension().and_then(|ext| ext.to_str()) != Some("mdoc") {
        bail!("target reference must resolve to a .mdoc file");
    }

    // Refresh the complete candidate subgraph so cycle checks do not rely on
    // stale edges after an external edit.
    cache.refresh_reachable_from_path(&path, -1)?;
    let (fnode, title, path) = cache.resolve_ref(target_ref, Some(&cwd()))?;
    let duplicate_paths = cache.duplicate_fnode_paths(&fnode)?;
    if duplicate_paths.len() > 1 {
        let paths = duplicate_paths
            .iter()
            .map(|path| crate::workspace::to_rel_path(mdcroot, path))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("target reference resolves to duplicate fnode '{fnode}': {paths}");
    }

    Ok(DependencyItem {
        depth: 0,
        fnode,
        title,
        rel_path: crate::workspace::to_rel_path(mdcroot, &path),
    })
}

pub(super) fn cmd_dep_add(
    source: String,
    query: Option<String>,
    target: Option<String>,
    max_results: usize,
) -> Result<i32> {
    if query.is_some() == target.is_some() {
        bail!("provide exactly one of <query> or --target <target-ref>");
    }

    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    let target_item = target
        .as_deref()
        .map(|target_ref| resolve_existing_target(&mut cache, &mdcroot, target_ref))
        .transpose()?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;

    if let Some(target_item) = target_item {
        if target_item.fnode == source_item.fnode {
            bail!("cannot add a node as its own dependency");
        }
        let (added, skipped_existing, _) =
            graph.add_direct_dependencies(vec![target_item.fnode.clone()])?;
        if !skipped_existing.is_empty() {
            println!(
                "already a dependency  {}",
                fmt_item(
                    &target_item.fnode,
                    &target_item.title,
                    &target_item.rel_path,
                    false
                )
            );
            return Ok(0);
        }
        if added.is_empty() {
            bail!("target was not added");
        }
        println!(
            "added  {}",
            fmt_item(
                &target_item.fnode,
                &target_item.title,
                &target_item.rel_path,
                false
            )
        );
        return Ok(0);
    }

    let q = query.unwrap_or_default().trim().to_string();
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
        if !all_rows.is_empty() {
            // Search returned matches, but they are all already dependencies
            // (or the source itself). Don't prompt to create a duplicate.
            println!("All matches for '{q}' are already dependencies of this node.");
            return Ok(0);
        }
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
        let short = short_fnode(&new_node.fnode);
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
        let new_fnode = new_node.fnode.clone();
        let node_path = new_node.path.clone();
        let added = graph.create_and_add_dependency(new_node)?;
        if added {
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

fn resolve_direct_dependency_target(
    graph: &mut DepGraph,
    direct_fnodes: &[String],
    target_ref: &str,
) -> Result<String> {
    let target_ref = target_ref.trim();
    if target_ref.is_empty() {
        bail!("target reference cannot be empty");
    }

    if let Some(exact) = direct_fnodes
        .iter()
        .find(|fnode| fnode.as_str() == target_ref)
    {
        return Ok(exact.clone());
    }

    let target_lc = target_ref.to_lowercase();
    let exact_lc: Vec<&String> = direct_fnodes
        .iter()
        .filter(|fnode| fnode.to_lowercase() == target_lc)
        .collect();
    if exact_lc.len() == 1 {
        return Ok(exact_lc[0].clone());
    }

    let prefix_matches: Vec<&String> = direct_fnodes
        .iter()
        .filter(|fnode| fnode.to_lowercase().starts_with(&target_lc))
        .collect();
    match prefix_matches.as_slice() {
        [matched] => return Ok((*matched).clone()),
        matches if matches.len() > 1 => {
            let preview = matches
                .iter()
                .map(|fnode| fnode.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("ambiguous direct dependency target '{target_ref}', matches: {preview}");
        }
        _ => {}
    }

    let mdcroot = graph.mdcroot.clone();
    let target_item = resolve_existing_target(&mut graph.cache, &mdcroot, target_ref)?;
    let canonical_matches: Vec<&String> = direct_fnodes
        .iter()
        .filter(|fnode| fnode.eq_ignore_ascii_case(&target_item.fnode))
        .collect();
    match canonical_matches.as_slice() {
        [matched] => Ok((*matched).clone()),
        _ => bail!(
            "target {} is not a direct dependency of this node",
            fmt_item(
                &target_item.fnode,
                &target_item.title,
                &target_item.rel_path,
                false
            )
        ),
    }
}

pub(super) fn cmd_dep_rm(source: String, target: Option<String>) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;
    let dep_items = graph.direct_dependency_items()?;
    let direct_fnodes = graph.direct_dependency_fnodes()?;

    if dep_items.is_empty() {
        if target.is_some() {
            bail!("source node has no direct dependencies");
        }
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

    if let Some(target_ref) = target {
        let target_fnode =
            resolve_direct_dependency_target(&mut graph, &direct_fnodes, &target_ref)?;
        let removed = graph.remove_direct_dependencies(vec![target_fnode.clone()])?;
        if removed.is_empty() {
            bail!("target is not a direct dependency of this node");
        }
        let label = dep_items
            .iter()
            .find(|item| item.fnode == target_fnode)
            .map(|item| fmt_item(&item.fnode, &item.title, &item.rel_path, false))
            .unwrap_or(target_fnode);
        println!("removed  {label}");
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
