use anyhow::Result;

use crate::config::default_for_srctype;
use crate::depgraph::DepGraph;
use crate::indcache::IndCache;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, CYN, RST};

// ── cmd: edit ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_edit(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let path = cache.resolve_edit_target_path(&source, Some(&cwd()))?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    std::process::Command::new(&editor).arg(&path).status()?;
    cache.upsert_path(&path)?;
    Ok(0)
}

// ── cmd: init ─────────────────────────────────────────────────────────────────

fn generate_config_toml() -> String {
    const SRCTYPES: &[&str] = &["natl", "latex", "py", "lean"];

    let mut out = String::from(
        "# MathDoc configuration\n\
         # Uncomment and edit sections below to override built-in defaults.\n",
    );

    for srctype in SRCTYPES {
        let cfg = default_for_srctype(srctype);
        out.push('\n');
        out.push_str(&format!("# [src.{srctype}]\n"));

        if let Some(v) = cfg.depens {
            out.push_str(&format!("# depens = {v}\n"));
        }
        if let Some(v) = cfg.reverse_depens {
            out.push_str(&format!("# reverse_depens = {v}\n"));
        }
        if let Some(v) = cfg.timeout_sec {
            out.push_str(&format!("# timeout_sec = {v}\n"));
        }
        if let Some(v) = cfg.setup_timeout_sec {
            out.push_str(&format!("# setup_timeout_sec = {v}\n"));
        }
        if let Some(ref v) = cfg.preamble {
            if v.contains('\n') {
                out.push_str("# preamble = \"\"\"\n");
                for line in v.lines() {
                    out.push_str(&format!("# {line}\n"));
                }
                out.push_str("# \"\"\"\n");
            } else {
                out.push_str(&format!("# preamble = \"{v}\"\n"));
            }
        }
        if let Some(ref v) = cfg.postamble {
            if v.contains('\n') {
                out.push_str("# postamble = \"\"\"\n");
                for line in v.lines() {
                    out.push_str(&format!("# {line}\n"));
                }
                out.push_str("# \"\"\"\n");
            } else {
                out.push_str(&format!("# postamble = \"{v}\"\n"));
            }
        }
        if let Some(ref v) = cfg.imports {
            let items: Vec<String> = v.iter().map(|s| format!("\"{s}\"")).collect();
            out.push_str(&format!("# imports = [{}]\n", items.join(", ")));
        }
    }

    out
}

pub(super) fn cmd_init() -> Result<i32> {
    let mdcroot = cwd();
    let mdc = mdcroot.join(".mdc");
    if mdc.is_dir() {
        println!("Already initialized as mdoc directory: {}", mdc.display());
        return Ok(0);
    }
    std::fs::create_dir_all(&mdc)?;
    std::fs::write(mdc.join("config.toml"), generate_config_toml())?;
    println!("mdoc folder initialized");
    Ok(0)
}

// ── cmd: new ──────────────────────────────────────────────────────────────────

pub(super) fn cmd_new(title: String, file: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot.clone())?;
    let (graph, _) = DepGraph::create_root(mdcroot, &file, &title, None, Some(cache))?;
    let item = {
        let mut g = graph;
        g.root_item()?
    };
    println!(
        "created  {}",
        fmt_item(&item.fnode, &item.title, &item.rel_path, false)
    );
    Ok(0)
}

// ── cmd: sync ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_sync() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = IndCache::open(mdcroot)?;
    cache.refresh_all()?;
    let total = cache.count()?;
    println!("synced  {BLD}{total}{RST} mdocs");
    Ok(0)
}

// ── cmd: search ───────────────────────────────────────────────────────────────

pub(super) fn cmd_search(query: String, max_results: usize) -> Result<i32> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(anyhow::anyhow!("query cannot be empty"));
    }
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let rows = cache.search(&q)?;
    let shown: Vec<_> = rows.iter().take(max_results).collect();

    println!(
        "{BLD}{}{RST} result{} for {CYN}{q}{RST}",
        shown.len(),
        if shown.len() == 1 { "" } else { "s" }
    );
    for (fnode, title, rel_path) in &shown {
        println!("  {}", fmt_item(fnode, title, rel_path, false));
    }
    Ok(0)
}
