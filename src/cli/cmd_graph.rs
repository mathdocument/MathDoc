use anyhow::Result;

use crate::core::{GraphIssue, IssueKind};

use super::{fmt_item, open_cache, require_mdcroot, BOLD, DIM, GREEN, RED, RESET, YELLOW};

// ── cmd: graph check ──────────────────────────────────────────────────────────

pub(super) fn cmd_graph_check(full: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    if full {
        cache.refresh_all()?;
    }
    let report = cache.graph_check_report()?;
    let ok = report.missing.is_empty() && report.invalid.is_empty() && report.cycles.is_empty();

    println!(
        "graph  {BOLD}{}{RESET} nodes  {BOLD}{}{RESET} edges",
        report.nodes, report.edges
    );

    if ok {
        println!("  {GREEN}✓{RESET} no issues");
        return Ok(0);
    }

    if !report.missing.is_empty() {
        println!("  {RED}missing ({}):{RESET}", report.missing.len());
        for issue in &report.missing {
            println!("    {}", fmt_issue(issue));
        }
    }
    if !report.invalid.is_empty() {
        println!("  {RED}invalid ({}):{RESET}", report.invalid.len());
        for issue in &report.invalid {
            println!("    {}", fmt_issue(issue));
            println!("      {DIM}{}{RESET}", issue.error);
        }
    }
    if !report.cycles.is_empty() {
        println!("  {RED}cycles ({}):{RESET}", report.cycles.len());
        for cycle in &report.cycles {
            let fnode_refs: Vec<&str> = cycle.iter().map(|s| s.as_str()).collect();
            let label = match cache.lookup_by_fnode(&fnode_refs) {
                Ok(rows) => {
                    let m: std::collections::HashMap<_, _> = rows.into_iter().collect();
                    cycle
                        .iter()
                        .map(|f| m.get(f).map(|(t, _)| t.as_str()).unwrap_or(f.as_str()))
                        .collect::<Vec<_>>()
                        .join(" → ")
                }
                Err(_) => cycle.join(" → "),
            };
            println!("    {YELLOW}↺{RESET} {label}");
        }
    }
    Ok(1)
}

// ── cmd: graph roots ──────────────────────────────────────────────────────────

pub(super) fn cmd_graph_roots(refresh: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    if refresh {
        cache.refresh_workspace_index()?;
    }
    let items = cache.global_root_items()?;
    println!(
        "{BOLD}{}{RESET} root node{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    );
    for item in &items {
        let comp = if item.component_size > 1 {
            format!("  {DIM}(component: {} nodes){RESET}", item.component_size)
        } else {
            String::new()
        };
        println!(
            "  {}{comp}",
            fmt_item(&item.fnode, &item.title, &item.rel_path, item.broken)
        );
    }
    Ok(0)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn fmt_issue(issue: &GraphIssue) -> String {
    use super::{short_fnode, DIM};
    let marker = match issue.kind {
        IssueKind::Missing => format!("{RED}✗ missing{RESET}"),
        IssueKind::Invalid => format!("{RED}✗ invalid{RESET}"),
    };
    format!(
        "{DIM}{}{RESET}  {marker}  {BOLD}{}{RESET}  {DIM}{}{RESET}",
        short_fnode(&issue.fnode),
        issue.title,
        issue.rel_path
    )
}
