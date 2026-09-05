use anyhow::Result;
use std::path::Path;

use crate::depgraph::DepGraph;
use crate::indcache::IndCache;

use super::{cwd, fmt_item, print_workdraft_issues, require_mdcroot, BLD, CYN, RST};

// ── cmd: edit ─────────────────────────────────────────────────────────────────

pub(super) fn launch_editor(path: &Path) -> Result<()> {
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let status = std::process::Command::new(&editor).arg(path).status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}

pub(super) fn cmd_edit(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = IndCache::open(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    let path = cache.resolve_edit_target_path(&source, Some(&cwd()))?;
    drop(cache);
    launch_editor(&path)?;
    let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&mdcroot)?;
    let mut cache = IndCache::open_under_mutation_lock(&mutation_lock)?;
    cache.upsert_path(&path)?;
    Ok(0)
}

// ── cmd: init ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_init() -> Result<i32> {
    let mdcroot = std::env::current_dir()?;
    let ancestor_lock = mdcroot
        .parent()
        .and_then(crate::workspace::find_mdcroot)
        .map(|root| crate::workspace::WorkspaceMutationLock::acquire(&root))
        .transpose()?;
    let changed = crate::workspace::initialize(&mdcroot)?;
    if let Some(lock) = &ancestor_lock {
        lock.root()?;
    }
    if changed {
        println!("mdoc folder initialized");
    } else {
        println!(
            "Already initialized as mdoc directory: {}",
            mdcroot.join(".mdc").display()
        );
    }
    Ok(0)
}

// ── cmd: new ──────────────────────────────────────────────────────────────────

pub(super) fn cmd_new(title: String, file: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = IndCache::open(mdcroot)?;
    let graph = DepGraph::create_root(&mut cache, &file, &title, None)?;
    let item = graph.root_item()?;
    println!(
        "created  {}",
        fmt_item(&item.fnode, &item.title, &item.rel_path, false)
    );
    Ok(0)
}

// ── cmd: sync ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_sync() -> Result<i32> {
    let _profile = crate::profile::scope("cli::cmd_sync");
    let (total, draft) = crate::application::work::export_mirrors(require_mdcroot()?)?;
    println!("synced  {BLD}{total}{RST} mdocs");
    println!(
        "exported {BLD}{}{RST} source files from {} valid mdocs ({} updated, {} removed)",
        draft.source_files, draft.valid_mdocs, draft.updated, draft.removed
    );
    print_workdraft_issues(&draft.warnings);
    print_workdraft_issues(&draft.dirty);
    print_workdraft_issues(&draft.conflicts);
    Ok(if draft.dirty.is_empty() && draft.conflicts.is_empty() {
        0
    } else {
        1
    })
}

// ── cmd: search ───────────────────────────────────────────────────────────────

pub(super) fn cmd_search(query: String, max_results: usize) -> Result<i32> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(anyhow::anyhow!("query cannot be empty"));
    }
    let mdcroot = require_mdcroot()?;
    let mut cache = IndCache::open(mdcroot)?;
    cache.discover_workspace_changes()?;
    let rows = cache.search(&q, max_results)?;

    println!(
        "{BLD}{}{RST} result{} for {CYN}{}{RST}",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        crate::core::escape_terminal(&q),
    );
    for node in &rows {
        println!(
            "  {}",
            fmt_item(&node.fnode, &node.title, &node.rel_path, node.broken)
        );
    }
    Ok(0)
}
