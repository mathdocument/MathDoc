use anyhow::Result;

use crate::config::default_for_srctype;
use crate::depgraph::DepGraph;
use crate::indcache::IndCache;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BOLD, DIM, RESET};

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

// ── cmd: edit ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_edit(source: String) -> Result<i32> {
    use super::{eprintln_err, eprintln_warn};

    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    let src_path = cache.resolve_edit_target_path(&source, Some(&cwd()))?;

    let editor_raw = std::env::var("EDITOR").unwrap_or_default();
    let editor_raw = editor_raw.trim().to_string();
    if editor_raw.is_empty() {
        return Err(anyhow::anyhow!("$EDITOR is not set"));
    }
    let editor_parts: Vec<&str> = editor_raw.split_whitespace().collect();
    let (editor_bin, editor_args) = editor_parts
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("$EDITOR is empty"))?;

    let status = std::process::Command::new(editor_bin)
        .args(editor_args)
        .arg(&src_path)
        .status()?;
    if !status.success() {
        let code = status.code().unwrap_or(1);
        eprintln_err(&format!("editor exited with code {code}"));
        return Ok(code);
    }

    if let Err(e) = cache.upsert_path(&src_path) {
        eprintln_warn(&format!("mdoc was edited but index update failed: {e}"));
    }
    let rel = crate::workspace::to_rel_path(&mdcroot, &src_path);
    println!("indexed  {DIM}{rel}{RESET}");
    Ok(0)
}

// ── cmd: sync ─────────────────────────────────────────────────────────────────

pub(super) fn cmd_sync() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = IndCache::open(mdcroot)?;
    cache.refresh_all()?;
    let total = cache.count()?;
    println!("synced  {BOLD}{total}{RESET} mdocs");
    Ok(0)
}

// ── cmd: search ───────────────────────────────────────────────────────────────

pub(super) fn cmd_search(query: String, max_results: usize) -> Result<i32> {
    use super::CYAN;

    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(anyhow::anyhow!("query cannot be empty"));
    }
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot)?;
    let rows = cache.search(&q)?;
    let shown: Vec<_> = rows.iter().take(max_results).collect();

    println!(
        "{BOLD}{}{RESET} result{} for {CYAN}{q}{RESET}",
        shown.len(),
        if shown.len() == 1 { "" } else { "s" }
    );
    for (fnode, title, rel_path) in &shown {
        println!("  {}", fmt_item(fnode, title, rel_path, false));
    }
    Ok(0)
}
