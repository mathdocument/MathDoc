use anyhow::{bail, Result};

use crate::core::{
    short_fnode, DependencyCandidatesEmpty, DependencyItem, DependencyTraversalReport, IssueKind,
};
use crate::depgraph::DepGraph;
use crate::indcache::IndCache;

use super::{
    cwd, fmt_item, print_cycles_if_any, print_dep_report, print_missing_with_referrers,
    require_mdcroot, BLD, RST,
};

// ── Shared dep display ────────────────────────────────────────────────────────

/// Print report sections and return the appropriate exit code (1 if cycles detected).
fn print_dep_report_sections(
    cache: &IndCache,
    source_item: &DependencyItem,
    count_label: &str,
    report: &DependencyTraversalReport,
) -> Result<i32> {
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
    print_missing_with_referrers(&missing, cache)?;
    print_cycles_if_any(&report.cycles, cache)?;
    Ok(if report.cycles.is_empty() { 0 } else { 1 })
}

// ── Shared setup for read commands ───────────────────────────────────────────

/// Open cache, discover changes, do a targeted refresh up to `refresh_depth`,
/// and resolve `source` to a `DependencyItem`. Used by dep show and dep leaf.
fn open_and_resolve_source(source: &str, refresh_depth: i32) -> Result<(IndCache, DependencyItem)> {
    let mut cache = IndCache::open(require_mdcroot()?)?;
    let mdcroot = cache.root().to_path_buf();
    cache.discover_workspace_changes()?;
    let src_path = cache.resolve_edit_target_path(source, Some(&cwd()))?;
    cache.refresh_reachable_from_path(&src_path, refresh_depth)?;
    let source_item = cache
        .resolve_ref(source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    Ok((cache, source_item))
}

// ── cmd: dep show ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_show(source: String, depth: i32) -> Result<i32> {
    let (cache, source_item) = open_and_resolve_source(&source, depth)?;
    let report = cache.dependency_report(&source_item.fnode, depth)?;
    print_dep_report_sections(&cache, &source_item, "depens", &report)
}

// ── cmd: dep leaf ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_leaf(source: String) -> Result<i32> {
    let (cache, source_item) = open_and_resolve_source(&source, -1)?;
    let report = cache.leaf_dependency_report(&source_item.fnode)?;
    print_dep_report_sections(&cache, &source_item, "leaves", &report)
}

// ── cmd: dep add ──────────────────────────────────────────────────────────────

fn excluded_candidates_message(
    query: &str,
    source: usize,
    existing_dependencies: usize,
    invalid_or_duplicate: usize,
) -> String {
    if source == 0 && invalid_or_duplicate == 0 {
        return format!("All matches for '{query}' are already dependencies of this node.");
    }
    if existing_dependencies == 0 && invalid_or_duplicate == 0 {
        return format!("All matches for '{query}' refer to the source node itself.");
    }
    if source == 0 && existing_dependencies == 0 {
        return format!(
            "All matches for '{query}' are invalid or duplicate nodes ({invalid_or_duplicate} excluded)."
        );
    }
    format!(
        "All matches for '{query}' were excluded: {source} source, \
         {existing_dependencies} existing dependencies, \
         {invalid_or_duplicate} invalid or duplicate."
    )
}

fn empty_candidates_message(query: &str, empty: &DependencyCandidatesEmpty) -> Option<String> {
    match empty {
        DependencyCandidatesEmpty::NoMatch => None,
        DependencyCandidatesEmpty::Excluded {
            source,
            existing_dependencies,
            invalid_or_duplicate,
        } => Some(excluded_candidates_message(
            query,
            *source,
            *existing_dependencies,
            *invalid_or_duplicate,
        )),
        DependencyCandidatesEmpty::ResultLimit { available } => Some(format!(
            "Found {available} available match(es) for '{query}', but --max-results is zero."
        )),
    }
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

    let q = query
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if query.is_some() && q.is_empty() {
        bail!("query cannot be empty");
    }

    let mut cache = IndCache::open(require_mdcroot()?)?;
    let mdcroot = cache.root().to_path_buf();
    cache.discover_workspace_changes()?;
    let mut graph = DepGraph::from_ref(&mut cache, &source, Some(&cwd()))?;

    if let Some(target_ref) = target.as_deref() {
        let (added, skipped_existing, skipped_self) =
            graph.add_direct_dependency_ref(target_ref, Some(&cwd()))?;
        if !skipped_self.is_empty() {
            bail!("cannot add a node as its own dependency");
        }
        let target_fnode = added
            .first()
            .or_else(|| skipped_existing.first())
            .ok_or_else(|| anyhow::anyhow!("target was not added"))?;
        let target_item = graph.ref_item_for_fnode(target_fnode, 0)?;
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

    let candidate_report = graph.dependency_candidates(&q, max_results)?;
    let candidates = candidate_report.nodes;

    if candidates.is_empty() {
        let empty = candidate_report
            .empty
            .expect("an empty candidate list has a semantic result");
        if let Some(message) = empty_candidates_message(&q, &empty) {
            println!("{message}");
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
        let new_fnode = uuid::Uuid::new_v4().to_string();
        let short = short_fnode(&new_fnode);
        let file_input: String = dialoguer::Input::new()
            .with_prompt(format!("File [{short}…]"))
            .allow_empty(true)
            .interact_text()?;
        let new_node =
            graph.prepare_new_dependency_node(file_input.trim(), &title, Some(&new_fnode))?;
        let node_path = new_node.path.clone();
        let added = graph.create_and_add_dependency(new_node)?;
        if added {
            let rel = crate::workspace::to_rel_path(&mdcroot, &node_path);
            println!(
                "created and added  {}",
                fmt_item(&new_fnode, &title, &rel, false)
            );
        }
        return Ok(0);
    }

    let items: Vec<(&str, &str, &str, bool)> = candidates
        .iter()
        .map(|item| {
            (
                item.fnode.as_str(),
                item.title.as_str(),
                item.rel_path.as_str(),
                item.broken,
            )
        })
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

    let selected_fnodes: Vec<String> = selected
        .iter()
        .map(|&i| candidates[i].fnode.clone())
        .collect();
    let (added, _, _) = graph.add_direct_dependencies(selected_fnodes)?;

    println!(
        "added {BLD}{}{RST} dep{}",
        added.len(),
        if added.len() == 1 { "" } else { "s" }
    );
    for fnode in &added {
        let label = candidates
            .iter()
            .find(|item| &item.fnode == fnode)
            .map(|item| fmt_item(&item.fnode, &item.title, &item.rel_path, item.broken))
            .unwrap_or_else(|| fnode.clone());
        println!("  + {label}");
    }
    Ok(0)
}

// ── cmd: dep rm ───────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_rm(source: String, target: Option<String>) -> Result<i32> {
    let mut cache = IndCache::open(require_mdcroot()?)?;
    cache.discover_workspace_changes()?;
    let mut graph = DepGraph::from_ref(&mut cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;
    let dep_items = graph.direct_dependency_items()?;

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
        let target_fnode = graph.resolve_direct_dependency_ref(&target_ref, Some(&cwd()))?;
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
        .map(|item| -> Result<_> {
            let broken = graph.is_broken_fnode(&item.fnode)?;
            Ok((
                item.fnode.as_str(),
                item.title.as_str(),
                item.rel_path.as_str(),
                broken,
            ))
        })
        .collect::<Result<_>>()?;

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
    let mut cache = IndCache::open(require_mdcroot()?)?;
    let mdcroot = cache.root().to_path_buf();
    cache.discover_workspace_changes()?;
    let target_path = cache.resolve_edit_target_path(&target, Some(&cwd()))?;
    cache.upsert_path(&target_path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_match_is_the_only_empty_candidate_state_that_offers_creation() {
        assert_eq!(
            empty_candidates_message("Missing", &DependencyCandidatesEmpty::NoMatch),
            None
        );
        assert!(empty_candidates_message(
            "Existing",
            &DependencyCandidatesEmpty::Excluded {
                source: 0,
                existing_dependencies: 1,
                invalid_or_duplicate: 0,
            }
        )
        .is_some());
        assert!(empty_candidates_message(
            "Invalid",
            &DependencyCandidatesEmpty::Excluded {
                source: 0,
                existing_dependencies: 0,
                invalid_or_duplicate: 1,
            }
        )
        .is_some());
        assert!(empty_candidates_message(
            "Mixed",
            &DependencyCandidatesEmpty::Excluded {
                source: 1,
                existing_dependencies: 2,
                invalid_or_duplicate: 3,
            }
        )
        .is_some());
        assert!(empty_candidates_message(
            "Limited",
            &DependencyCandidatesEmpty::ResultLimit { available: 1 }
        )
        .is_some());
    }
}
