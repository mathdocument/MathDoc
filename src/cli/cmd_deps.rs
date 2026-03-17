use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::core::DependencyItem;
use crate::depgraph::DepGraph;

use super::{
    cwd, eprintln_warn, fmt_item, open_cache, print_dep_report, require_mdcroot, BOLD, RESET,
};

// ── cmd: dep show ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_show(source: String, depth: i32, refresh: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    if refresh {
        cache.discover_workspace_changes()?;
        let src_path = cache.resolve_edit_target_path(&source, Some(&cwd())).ok();
        if let Some(path) = src_path {
            cache.refresh_reachable_from_path(&path, depth)?;
        }
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
    Ok(0)
}

// ── cmd: dep leaf ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_leaf(source: String, refresh: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    if refresh {
        cache.discover_workspace_changes()?;
        let src_path = cache.resolve_edit_target_path(&source, Some(&cwd())).ok();
        if let Some(path) = src_path {
            cache.refresh_reachable_from_path(&path, -1)?;
        }
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
    let existing_fnodes: std::collections::HashSet<String> = {
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
        println!("No new dependency candidates for: {q}");
        return Ok(0);
    }

    let selected = match select_multi(
        "Select dependencies to add",
        candidates
            .iter()
            .map(|(f, t, p)| (f.as_str(), t.as_str(), p.as_str())),
    )? {
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
        "added {BOLD}{}{RESET} dep{}",
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

    let candidates: Vec<_> = dep_items
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

    let selected = match select_multi_with_broken(
        "Select dependencies to remove",
        candidates.iter().map(|(f, t, p, b)| (*f, *t, *p, *b)),
    )? {
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
        "removed {BOLD}{}{RESET} dep{}",
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

pub(super) fn cmd_dep_refs(target: String, depth: i32, refresh: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    if refresh {
        cache.refresh_workspace_index()?;
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

// ── Interactive multi-select ──────────────────────────────────────────────────

fn select_multi<'a>(
    prompt: &str,
    items: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) -> Result<Option<Vec<usize>>> {
    let items: Vec<_> = items.collect();
    select_multi_with_broken(prompt, items.iter().map(|(f, t, p)| (*f, *t, *p, false)))
}

fn select_multi_with_broken<'a>(
    prompt: &str,
    items: impl Iterator<Item = (&'a str, &'a str, &'a str, bool)>,
) -> Result<Option<Vec<usize>>> {
    let items: Vec<_> = items.collect();
    println!("\n{BOLD}{prompt}{RESET}");
    for (i, (fnode, title, rel_path, broken)) in items.iter().enumerate() {
        println!(
            "  [{:3}]  {}",
            i + 1,
            fmt_item(fnode, title, rel_path, *broken)
        );
    }
    print!("\nSelect numbers (e.g. 1,3-5) or q to cancel: ");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let line = match stdin.lock().lines().next() {
        Some(Ok(l)) => l,
        _ => return Ok(None),
    };
    let trimmed = line.trim();
    if trimmed.eq_ignore_ascii_case("q") || trimmed.is_empty() {
        return Ok(if trimmed.is_empty() {
            Some(vec![])
        } else {
            None
        });
    }

    let mut selected = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            let a: usize = a
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid selection: {part}"))?;
            let b: usize = b
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid selection: {part}"))?;
            if a < 1 || b > items.len() || a > b {
                return Err(anyhow::anyhow!("invalid range: {part}"));
            }
            for i in a..=b {
                let idx = i - 1;
                if !selected.contains(&idx) {
                    selected.push(idx);
                }
            }
        } else {
            let n: usize = part
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid selection: {part}"))?;
            if n < 1 || n > items.len() {
                return Err(anyhow::anyhow!(
                    "invalid selection: {n} (max {})",
                    items.len()
                ));
            }
            let idx = n - 1;
            if !selected.contains(&idx) {
                selected.push(idx);
            }
        }
    }
    Ok(Some(selected))
}
