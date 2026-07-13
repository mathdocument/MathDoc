use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;

use crate::config::default_for_srctype;
use crate::depgraph::DepGraph;
use crate::indcache::IndCache;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, CYN, RST};

// ── cmd: edit ─────────────────────────────────────────────────────────────────

pub(super) fn launch_editor(path: &Path) -> Result<()> {
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    launch_editor_with(&editor, path)
}

pub(super) fn launch_editor_with(editor: &OsStr, path: &Path) -> Result<()> {
    let status = std::process::Command::new(editor).arg(path).status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}

pub(super) fn cmd_edit(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot)?;
    cache.discover_workspace_changes()?;
    let path = cache.resolve_edit_target_path(&source, Some(&cwd()))?;
    launch_editor(&path)?;
    cache.upsert_path(&path)?;
    Ok(0)
}

// ── cmd: init ─────────────────────────────────────────────────────────────────

fn generate_config_toml() -> String {
    let mut out = String::from(
        "# MathDoc configuration\n\
         # Uncomment and edit sections below to override built-in defaults.\n\
         # Preamble/postamble are managed as files in .mdc/<srctype>/.\n",
    );

    for srctype in crate::config::BUILTIN_SRCTYPES {
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
    }

    out
}

pub(super) fn cmd_init() -> Result<i32> {
    let mdcroot = cwd();
    let changed = init_workspace(&mdcroot)?;
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

fn init_workspace(mdcroot: &Path) -> Result<bool> {
    let mdc = mdcroot.join(".mdc");
    let mut changed = crate::workspace::ensure_regular_directory_exists(&mdc)?;
    changed |= crate::workspace::atomic_create_if_missing(
        &mdc.join("config.toml"),
        generate_config_toml().as_bytes(),
    )?;
    changed |= crate::config::init_amble_files(mdcroot)?;
    Ok(changed)
}

// ── cmd: new ──────────────────────────────────────────────────────────────────

pub(super) fn cmd_new(title: String, file: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot.clone())?;
    let graph = DepGraph::create_root(mdcroot, &file, &title, None, Some(cache))?;
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

#[cfg(test)]
mod init_tests {
    use super::*;

    #[test]
    fn init_repairs_partial_workspace_without_overwriting_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let mdc = dir.path().join(".mdc");
        std::fs::create_dir(&mdc).unwrap();
        std::fs::write(mdc.join("config.toml"), "# custom\n").unwrap();
        std::fs::create_dir(mdc.join("latex")).unwrap();
        std::fs::write(mdc.join("latex/preamble.tex"), "custom preamble\n").unwrap();

        assert!(init_workspace(dir.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(mdc.join("config.toml")).unwrap(),
            "# custom\n"
        );
        assert_eq!(
            std::fs::read_to_string(mdc.join("latex/preamble.tex")).unwrap(),
            "custom preamble\n"
        );
        assert_eq!(
            std::fs::read_to_string(mdc.join("latex/postamble.tex")).unwrap(),
            crate::config::default_postamble("latex")
        );
        assert!(!init_workspace(dir.path()).unwrap());
    }

    #[test]
    fn init_rejects_non_directory_control_path() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mdc"), "not a directory").unwrap();
        assert!(init_workspace(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn init_rejects_symlinked_control_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        symlink(outside.path(), dir.path().join(".mdc")).unwrap();

        assert!(init_workspace(dir.path()).is_err());
        assert!(!outside.path().join("config.toml").exists());
    }
}
