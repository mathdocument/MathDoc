use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;

use crate::compiler::CompilerRegistry;
use crate::config::Config;
use crate::depgraph::workback;
use crate::depgraph::DepGraph;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, DIM, GRN, RED, RST, YLW};

// ── Hash helpers ─────────────────────────────────────────────────────────────

fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_path(work_path: &Path) -> std::path::PathBuf {
    let dir = work_path.parent().unwrap_or(work_path);
    dir.join(".MdcWork.hash")
}

fn read_hashes(work_path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(hash_path(work_path))
        .map(|s| {
            s.lines()
                .filter_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_hashes(work_path: &Path, hashes: &HashMap<String, String>) {
    let mut lines: Vec<String> = hashes.iter().map(|(k, v)| format!("{k}={v}")).collect();
    lines.sort();
    let _ = std::fs::write(hash_path(work_path), lines.join("\n"));
}

// ── cmd: work ────────────────────────────────────────────────────────────────

pub(super) fn cmd_work(source: String, depth: i32, compile: bool) -> Result<i32> {
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
    let mut generated: Vec<(String, String)> = Vec::new(); // (srctype, path)
    let mut skipped: Vec<String> = Vec::new();

    let mut sorted_srctypes: Vec<&String> = files.keys().collect();
    sorted_srctypes.sort();
    for srctype in sorted_srctypes {
        let work_file = &files[srctype];
        let ext = crate::config::srctype_ext(srctype);
        let dir = mdc_dir.join(srctype);
        std::fs::create_dir_all(&dir)?;
        let work_path = dir.join(format!("MdcWork.{}", ext));

        // Check if existing file has unsaved user edits.
        if work_path.is_file() {
            let existing = std::fs::read_to_string(&work_path)?;
            let stored = read_hashes(&work_path);
            let current = content_hash(&existing);

            if stored.get("@file").map(|h| h.as_str()) != Some(&current) {
                eprintln!(
                    "{YLW}warning:{RST} {BLD}{}{RST} has unsaved changes, skipping. Run {BLD}mdc back{RST} first or delete it.",
                    work_path.display()
                );
                skipped.push(work_path.display().to_string());
                continue;
            }
        }

        std::fs::write(&work_path, &work_file.content)?;
        let mut hashes = HashMap::new();
        hashes.insert("@file".to_string(), content_hash(&work_file.content));
        hashes.insert(
            "@preamble".to_string(),
            content_hash(work_file.preamble.as_deref().unwrap_or("")),
        );
        hashes.insert(
            "@postamble".to_string(),
            content_hash(work_file.postamble.as_deref().unwrap_or("")),
        );
        for (fnode, node_content) in &work_file.nodes {
            hashes.insert(fnode.clone(), content_hash(node_content));
        }
        write_hashes(&work_path, &hashes);
        generated.push((srctype.clone(), work_path.display().to_string()));
    }

    if !generated.is_empty() {
        println!("{GRN}Generated:{RST}");
        for (_, p) in &generated {
            println!("  {p}");
        }
    }
    if !skipped.is_empty() {
        println!("{YLW}Skipped (unsaved changes):{RST}");
        for p in &skipped {
            println!("  {p}");
        }
    }

    if !compile || generated.is_empty() {
        return Ok(if skipped.is_empty() { 0 } else { 1 });
    }

    // ── Compile each generated work file ────────────────────────────────
    println!();
    let registry = CompilerRegistry::default_registry();
    let total = generated.len();
    let mut failed = 0;

    for (i, (srctype, _work_path_str)) in generated.iter().enumerate() {
        println!("[{}/{}] {BLD}{srctype}{RST}", i + 1, total);
        let _ = std::io::stdout().flush();

        let src_cfg = config.src_config(srctype);
        let compcfg = src_cfg.to_compiler_cfg();

        fn compile_progress(msg: &str) {
            println!("  {DIM}{msg}{RST}");
        }

        let req = crate::compiler::CompilerReq {
            mdcroot: mdcroot.clone(),
            compcfg,
            progress: Some(Box::new(compile_progress)),
        };

        let res = match registry.resolve(srctype) {
            Some(compiler) => compiler.compile(&req),
            None => crate::compiler::CompilerRes::err(format!("unknown srctype: {srctype}")),
        };

        if !res.stdout.is_empty() {
            for line in res.stdout.lines() {
                println!("  {line}");
            }
        }
        if !res.stderr.is_empty() {
            for line in res.stderr.lines() {
                eprintln!("  {RED}{line}{RST}");
            }
        }
        if res.result {
            println!("{GRN}✓{RST} (exit {})", res.rtcode);
        } else {
            failed += 1;
            println!("{RED}✗{RST} (exit {})", res.rtcode);
        }
        println!();
    }

    Ok(if failed > 0 || !skipped.is_empty() {
        1
    } else {
        0
    })
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

    // Scan .mdc/*/MdcWork.* for active work files.
    let entries = std::fs::read_dir(&mdc_dir)?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let srctype_dir = entry.path();
        let srctype = entry.file_name().to_string_lossy().to_string();
        let ext = crate::config::srctype_ext(&srctype);
        let work_path = srctype_dir.join(format!("MdcWork.{}", ext));

        if !work_path.is_file() {
            continue;
        }
        found_any = true;

        println!("{BLD}[{srctype}]{RST} {}", work_path.display());

        let extracted = workback::extract_work_file(&work_path, &srctype)?;

        // Abort this file if any warnings (stray content between markers, unclosed blocks).
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

        let stored = read_hashes(&work_path);
        let mut new_hashes = HashMap::new();
        let file_content = std::fs::read_to_string(&work_path)?;
        new_hashes.insert("@file".to_string(), content_hash(&file_content));

        // Write preamble/postamble back only if changed. None (empty block) is treated as ""
        // so that clearing a preamble/postamble in the work file propagates back.
        {
            let pre = extracted.preamble.as_deref().unwrap_or("");
            let hash = content_hash(pre);
            let changed = stored.get("@preamble").map(|h| h != &hash).unwrap_or(true);
            if changed {
                let content = if pre.is_empty() {
                    String::new()
                } else {
                    format!("{pre}\n")
                };
                crate::config::write_preamble(&mdcroot, &srctype, &content)?;
                println!("  synced: preamble");
                total_synced += 1;
            }
            new_hashes.insert("@preamble".to_string(), hash);
        }
        {
            let post = extracted.postamble.as_deref().unwrap_or("");
            let hash = content_hash(post);
            let changed = stored.get("@postamble").map(|h| h != &hash).unwrap_or(true);
            if changed {
                let content = if post.is_empty() {
                    String::new()
                } else {
                    format!("{post}\n")
                };
                crate::config::write_postamble(&mdcroot, &srctype, &content)?;
                println!("  synced: postamble");
                total_synced += 1;
            }
            new_hashes.insert("@postamble".to_string(), hash);
        }

        // Pre-check: title: lines are read-only; abort the whole file if any were modified.
        {
            let mut title_ok = true;
            for (fnode, extracted_title, _) in &extracted.nodes {
                if let Ok((_, actual_title, _)) = cache.resolve_ref(fnode, None) {
                    if extracted_title != &actual_title {
                        let short = &fnode[..fnode.len().min(8)];
                        eprintln!(
                            "  {YLW}warning:{RST} title of {short} was modified\n\
                             {DIM}    original:{RST} {BLD}{actual_title}{RST}\n\
                             {DIM}    modified:{RST} {BLD}{extracted_title}{RST}"
                        );
                        title_ok = false;
                    }
                }
            }
            if !title_ok {
                eprintln!(
                    "  {YLW}aborted:{RST} title fields are read-only. \
                     Restore the original titles and retry."
                );
                had_errors = true;
                continue;
            }
        }

        let mut file_clean = true;

        for (fnode, _title, content) in &extracted.nodes {
            let hash = content_hash(content);
            new_hashes.insert(fnode.clone(), hash.clone());

            // Skip unchanged nodes.
            if stored.get(fnode).map(|h| h == &hash).unwrap_or(false) {
                continue;
            }

            match cache.resolve_ref(fnode, None) {
                Ok((full_fnode, title, abs_path)) => {
                    let rel_path = crate::workspace::to_rel_path(&mdcroot, &abs_path);
                    let mut node = crate::mdocnode::MdocNode::load(&mdcroot, &abs_path)?;

                    let block = node.blocks.iter_mut().find(|b| b.srctype == srctype);
                    let new_content = if content.is_empty() {
                        String::new()
                    } else {
                        format!("{}\n", content)
                    };

                    match block {
                        Some(b) => b.content = new_content,
                        None => {
                            node.blocks.push(crate::mdocnode::SrcBlock {
                                srctype: srctype.clone(),
                                content: new_content,
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

        // Update hashes only on clean sync (no resolve failures).
        if file_clean {
            write_hashes(&work_path, &new_hashes);
        }
    }

    if !found_any {
        println!("No active work files found");
        return Ok(0);
    }

    println!(
        "\n{BLD}{total_synced}{RST} change{} synced",
        if total_synced == 1 { "" } else { "s" },
    );

    Ok(if had_errors { 1 } else { 0 })
}
