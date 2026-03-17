use anyhow::Result;

use crate::core::{GraphIssue, IssueKind};

use super::{
    fmt_item, open_cache, print_missing_with_referrers, require_mdcroot, BLD, DIM, GRN, RED, RST,
};

// ── cmd: graph check ──────────────────────────────────────────────────────────

pub(super) fn cmd_graph_check() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let report = cache.graph_check_report()?;
    let ok = report.missing.is_empty() && report.invalid.is_empty() && report.cycles.is_empty();

    println!(
        "graph  {BLD}{}{RST} nodes  {BLD}{}{RST} edges",
        report.nodes, report.edges
    );

    if ok {
        println!("  {GRN}✓{RST} no issues");
        return Ok(0);
    }

    if !report.missing.is_empty() {
        print_missing_with_referrers(&report.missing, &cache);
    }
    if !report.invalid.is_empty() {
        println!("  {RED}invalid ({}):{RST}", report.invalid.len());
        for issue in &report.invalid {
            println!("    {}", fmt_issue(issue));
            println!("      {DIM}{}{RST}", issue.error);
        }
    }
    if !report.cycles.is_empty() {
        println!("  {RED}cycles ({}):{RST}", report.cycles.len());
        for cycle in &report.cycles {
            let fnode_refs: Vec<&str> = cycle.iter().map(|s| s.as_str()).collect();
            let label_map = cache.lookup_by_fnode(&fnode_refs).unwrap_or_default();
            super::print_cycle(cycle, &label_map);
        }
    }
    Ok(1)
}

// ── cmd: graph roots ──────────────────────────────────────────────────────────

pub(super) fn cmd_graph_roots() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let mut items = cache.global_root_items()?;
    let topo = cache.all_topo_depths().unwrap_or_default();
    items.sort_by(|a, b| {
        let da = topo.get(&a.fnode).copied().unwrap_or(0);
        let db = topo.get(&b.fnode).copied().unwrap_or(0);
        db.cmp(&da).then(b.component_size.cmp(&a.component_size))
    });
    println!(
        "{BLD}{}{RST} root node{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    );
    let depths: Vec<u32> = items
        .iter()
        .map(|i| topo.get(&i.fnode).copied().unwrap_or(0))
        .collect();
    let w = depths.iter().max().copied().unwrap_or(0).to_string().len();
    for (item, depth) in items.iter().zip(&depths) {
        println!(
            "   [{:>w$}]  {}",
            depth,
            fmt_item(&item.fnode, &item.title, &item.rel_path, item.broken)
        );
    }
    Ok(0)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn fmt_issue(issue: &GraphIssue) -> String {
    use super::{short_fnode, DIM};
    let marker = match issue.kind {
        IssueKind::Missing => format!("{RED}✗ missing{RST}"),
        IssueKind::Invalid => format!("{RED}✗ invalid{RST}"),
    };
    format!(
        "{DIM}{}{RST}  {marker}  {BLD}{}{RST}  {DIM}{}{RST}",
        short_fnode(&issue.fnode),
        issue.title,
        issue.rel_path
    )
}
