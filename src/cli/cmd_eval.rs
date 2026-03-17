use anyhow::Result;

use crate::compiler::CompilerRegistry;
use crate::config::Config;
use crate::depgraph::DepGraph;

use super::{
    cwd, eprintln_err, open_cache, print_dep_report, require_mdcroot, BOLD, GREEN,
    RED, RESET,
};

// ── cmd: eval ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_eval(source: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;

    cache.discover_workspace_changes()?;
    // Also ensure the source is in the cache if it was just created on disk.
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.upsert_path(&src_path);
    }

    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let root_path = graph.root_path()?;
    graph.cache.refresh_reachable_from_path(&root_path, depth)?;

    let root_item = graph.root_item()?;
    let report = graph.cache.dependency_report(&root_item.fnode, depth)?;
    print_dep_report(
        "source",
        &root_item,
        "depens",
        &report.items,
        &report.issues_by_fnode,
    );

    let broken_count: usize = report.issues_by_fnode.len();
    if broken_count > 0 {
        eprintln_err(&format!(
            "{broken_count} broken dependenc{}",
            if broken_count == 1 { "y" } else { "ies" }
        ));
        return Ok(1);
    }

    if !graph.root_has_blocks()? {
        println!("No blocks to eval");
        return Ok(0);
    }

    let config = Config::load(&graph.mdcroot)?;
    let registry = CompilerRegistry::default_registry();
    fn eval_progress(msg: &str) {
        println!("  \x1b[2m{msg}\x1b[0m");
    }
    let results = graph.eval_blocks(depth, &registry, &config, Some(eval_progress))?;

    if results.is_empty() {
        println!("No blocks to eval");
        return Ok(0);
    }

    let total = results.len();
    let mut failed = 0;
    for (i, br) in results.iter().enumerate() {
        let n = i + 1;
        let ok_str = if br.res.result {
            format!("{GREEN}✓{RESET}")
        } else {
            failed += 1;
            format!("{RED}✗{RESET}")
        };
        println!(
            "[{n}/{total}] {ok_str} {BOLD}{}{RESET}  (exit {})",
            br.srctype, br.res.rtcode
        );
        if !br.res.stdout.is_empty() {
            for line in br.res.stdout.lines() {
                println!("  {line}");
            }
        }
        if !br.res.stderr.is_empty() {
            for line in br.res.stderr.lines() {
                eprintln!("  {RED}{line}{RESET}");
            }
        }
    }
    Ok(if failed > 0 { 1 } else { 0 })
}
