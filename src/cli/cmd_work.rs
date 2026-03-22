use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::config::Config;
use crate::depgraph::workback;
use crate::depgraph::DepGraph;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, GRN, RST, YLW};

// ── Hash helpers ─────────────────────────────────────────────────────────────

fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_path(work_path: &PathBuf) -> PathBuf {
    let dir = work_path.parent().unwrap_or(work_path);
    dir.join(".mdc-work.hash")
}

fn read_stored_hash(work_path: &PathBuf) -> Option<String> {
    std::fs::read_to_string(hash_path(work_path)).ok()
}

fn write_hash(work_path: &PathBuf, hash: &str) {
    let _ = std::fs::write(hash_path(work_path), hash);
}

// ── Srctype → file extension ─────────────────────────────────────────────────

fn srctype_ext(srctype: &str) -> &str {
    crate::config::srctype_ext(srctype)
}

// ── cmd: work ────────────────────────────────────────────────────────────────

pub(super) fn cmd_work(source: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;

    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.upsert_path(&src_path);
    }

    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let root_path = graph.root_path()?;
    graph.cache.refresh_reachable_from_path(&root_path, depth)?;

    let config = Config::load(&graph.mdcroot)?;
    let files = workback::merge_work_files(&mut graph, depth, &config)?;

    if files.is_empty() {
        println!("No source blocks found in dependency subgraph");
        return Ok(0);
    }

    let mdc_dir = mdcroot.join(".mdc");
    let mut generated: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for (srctype, content) in &files {
        let ext = srctype_ext(srctype);
        let dir = mdc_dir.join(srctype);
        std::fs::create_dir_all(&dir)?;
        let work_path = dir.join(format!("mdc-work.{}", ext));

        // Check if existing file has unsaved user edits.
        if work_path.is_file() {
            let existing = std::fs::read_to_string(&work_path)?;
            let stored = read_stored_hash(&work_path);
            let current = content_hash(&existing);

            if stored.as_deref() != Some(&current) {
                // User has modified the file since last generation.
                eprintln!(
                    "{YLW}warning:{RST} {BLD}{}{RST} has unsaved changes, skipping. Run {BLD}mdc back{RST} first or delete it.",
                    work_path.display()
                );
                skipped.push(work_path.display().to_string());
                continue;
            }
        }

        std::fs::write(&work_path, content)?;
        write_hash(&work_path, &content_hash(content));
        generated.push(work_path.display().to_string());
    }

    if !generated.is_empty() {
        println!("{GRN}Generated:{RST}");
        for p in &generated {
            println!("  {p}");
        }
    }
    if !skipped.is_empty() {
        println!("{YLW}Skipped (unsaved changes):{RST}");
        for p in &skipped {
            println!("  {p}");
        }
    }

    Ok(if skipped.is_empty() { 0 } else { 1 })
}

// ── cmd: back ────────────────────────────────────────────────────────────────

pub(super) fn cmd_back() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;

    let mdc_dir = mdcroot.join(".mdc");
    let mut total_synced = 0usize;
    let mut had_errors = false;
    let mut found_any = false;

    // Scan .mdc/*/mdc-work.* for active work files.
    let entries = std::fs::read_dir(&mdc_dir)?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let srctype_dir = entry.path();
        let srctype = entry.file_name().to_string_lossy().to_string();
        let ext = srctype_ext(&srctype);
        let work_path = srctype_dir.join(format!("mdc-work.{}", ext));

        if !work_path.is_file() {
            continue;
        }
        found_any = true;

        println!("{BLD}[{srctype}]{RST} {}", work_path.display());

        let extracted = workback::extract_work_file(&work_path, &srctype)?;

        // Abort this file if any warnings (stray content between markers, unclosed blocks).
        // These indicate possible truncation or corruption — refuse to write back.
        if !extracted.warnings.is_empty() {
            for warning in &extracted.warnings {
                eprintln!("  {YLW}warning:{RST} {warning}");
            }
            eprintln!(
                "  {YLW}aborted:{RST} refusing to sync — work file may be corrupted. Fix the file and retry."
            );
            had_errors = true;
            continue;
        }

        // Write preamble/postamble back to files.
        if let Some(ref pre) = extracted.preamble {
            crate::config::write_preamble(&mdcroot, &srctype, &format!("{pre}\n"))?;
        }
        if let Some(ref post) = extracted.postamble {
            crate::config::write_postamble(&mdcroot, &srctype, &format!("{post}\n"))?;
        }

        let mut file_clean = true;

        for (fnode, content) in &extracted.nodes {
            // Resolve fnode prefix to full fnode + path.
            match cache.resolve_ref(fnode, None) {
                Ok((full_fnode, title, abs_path)) => {
                    let rel_path = crate::workspace::to_rel_path(&mdcroot, &abs_path);
                    let mut node = crate::mdocnode::MdocNode::load(&mdcroot, &abs_path)?;

                    // Find or create the @src block for this srctype.
                    let block = node.blocks.iter_mut().find(|b| b.srctype == srctype);

                    match block {
                        Some(b) => {
                            b.content = if content.is_empty() {
                                String::new()
                            } else {
                                format!("{}\n", content)
                            };
                        }
                        None => {
                            // Node had no block of this srctype — create one.
                            node.blocks.push(crate::mdocnode::SrcBlock {
                                srctype: srctype.clone(),
                                content: if content.is_empty() {
                                    String::new()
                                } else {
                                    format!("{}\n", content)
                                },
                                metadata: std::collections::HashMap::new(),
                            });
                        }
                    }

                    node.save()?;
                    println!(
                        "  synced: {}",
                        fmt_item(&full_fnode, &title, &rel_path, false)
                    );
                    total_synced += 1;
                }
                Err(_) => {
                    eprintln!("  {YLW}warning:{RST} fnode {fnode} not found in index, skipping");
                    file_clean = false;
                    had_errors = true;
                }
            }
        }

        // Update hash only on clean sync (no resolve failures).
        if file_clean {
            let current_content = std::fs::read_to_string(&work_path)?;
            write_hash(&work_path, &content_hash(&current_content));
        }
    }

    if !found_any {
        println!("No active work files found");
        return Ok(0);
    }

    println!(
        "\n{BLD}{total_synced}{RST} node{} synced",
        if total_synced == 1 { "" } else { "s" },
    );

    Ok(if had_errors { 1 } else { 0 })
}
