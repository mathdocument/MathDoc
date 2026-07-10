use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;

use crate::compiler::CompilerRegistry;
use crate::config::Config;
use crate::core::escape_terminal;
use crate::depgraph::workback;
use crate::depgraph::DepGraph;
use crate::safe_file::{atomic_replace, ensure_regular_directory, AppliedWrite, FileSnapshot};

use super::{
    cwd, fmt_item, open_cache, require_mdcroot, short_fnode, BLD, DIM, GRN, RED, RST, YLW,
};

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

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkHashes {
    version: u32,
    file: String,
    preamble: String,
    postamble: String,
    nodes: HashMap<String, String>,
}

fn parse_hashes(snapshot: &FileSnapshot) -> Result<Option<WorkHashes>> {
    let Some(content) = snapshot.content() else {
        return Ok(None);
    };
    let text = std::str::from_utf8(content)?;
    if let Ok(sidecar) = serde_json::from_str::<WorkHashes>(text) {
        if sidecar.version != 1 {
            anyhow::bail!("unsupported work hash sidecar version {}", sidecar.version);
        }
        return Ok(Some(sidecar));
    }

    // Compatibility with sidecars written before the versioned JSON format.
    let legacy: HashMap<String, String> = text
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect();
    let Some(file) = legacy.get("@file") else {
        return Ok(None);
    };
    Ok(Some(WorkHashes {
        version: 1,
        file: file.clone(),
        preamble: legacy.get("@preamble").cloned().unwrap_or_default(),
        postamble: legacy.get("@postamble").cloned().unwrap_or_default(),
        nodes: legacy
            .into_iter()
            .filter(|(key, _)| !matches!(key.as_str(), "@file" | "@preamble" | "@postamble"))
            .collect(),
    }))
}

fn render_hashes(hashes: &WorkHashes) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(hashes)?)
}

fn rollback_writes(writes: Vec<AppliedWrite>) -> Result<()> {
    let mut errors = Vec::new();
    for write in writes.into_iter().rev() {
        if let Err(error) = write.rollback() {
            errors.push(error.to_string());
        }
    }
    if !errors.is_empty() {
        anyhow::bail!("rollback failed: {}", errors.join("; "));
    }
    Ok(())
}

fn srctype_dir(mdcroot: &Path, srctype: &str) -> Result<std::path::PathBuf> {
    crate::config::validate_srctype_name(srctype)?;
    let mdc_dir = mdcroot.join(".mdc");
    ensure_regular_directory(&mdc_dir)?;
    let dir = mdc_dir.join(srctype);
    match std::fs::symlink_metadata(&dir) {
        Ok(_) => ensure_regular_directory(&dir)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&dir)?;
            ensure_regular_directory(&dir)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(dir)
}

fn amble_path(mdcroot: &Path, srctype: &str, kind: &str) -> std::path::PathBuf {
    mdcroot
        .join(".mdc")
        .join(srctype)
        .join(format!("{kind}.{}", crate::config::srctype_ext(srctype)))
}

fn snapshot_text(snapshot: &FileSnapshot, default: &str) -> Result<String> {
    match snapshot.content() {
        Some(content) => Ok(std::str::from_utf8(content)?.to_string()),
        None => Ok(default.to_string()),
    }
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

    let mut generated: Vec<(String, String)> = Vec::new(); // (srctype, path)
    let mut skipped: Vec<String> = Vec::new();
    struct PreparedWork {
        srctype: String,
        work_path: std::path::PathBuf,
        work_snapshot: FileSnapshot,
        sidecar_path: std::path::PathBuf,
        sidecar_snapshot: FileSnapshot,
        work_content: Vec<u8>,
        sidecar_content: Vec<u8>,
        input_snapshots: Vec<(std::path::PathBuf, FileSnapshot)>,
    }
    let mut prepared = Vec::new();

    let mut sorted_srctypes: Vec<&String> = files.keys().collect();
    sorted_srctypes.sort();
    for srctype in sorted_srctypes {
        let work_file = &files[srctype];
        let ext = crate::config::srctype_ext(srctype);
        let dir = srctype_dir(&mdcroot, srctype)?;
        let work_path = dir.join(format!("MdcWork.{}", ext));
        let work_snapshot = FileSnapshot::capture(&work_path)?;
        let sidecar_path = hash_path(&work_path);
        let sidecar_snapshot = FileSnapshot::capture(&sidecar_path)?;

        // Check if existing file has unsaved user edits.
        if let Some(existing) = work_snapshot.content() {
            let existing = std::str::from_utf8(existing)?;
            let stored = parse_hashes(&sidecar_snapshot)?;
            let current = content_hash(existing);

            if stored.as_ref().map(|hashes| hashes.file.as_str()) != Some(&current) {
                eprintln!(
                    "{YLW}warning:{RST} {BLD}{}{RST} has unsaved changes, skipping. Run {BLD}mdc back{RST} first or delete it.",
                    work_path.display()
                );
                skipped.push(work_path.display().to_string());
                continue;
            }
        }

        let mut node_hashes = HashMap::new();
        for (fnode, node_content) in &work_file.nodes {
            node_hashes.insert(fnode.clone(), content_hash(node_content));
        }
        let hashes = WorkHashes {
            version: 1,
            file: content_hash(&work_file.content),
            preamble: content_hash(work_file.preamble.as_deref().unwrap_or("")),
            postamble: content_hash(work_file.postamble.as_deref().unwrap_or("")),
            nodes: node_hashes,
        };
        prepared.push(PreparedWork {
            srctype: srctype.clone(),
            work_path,
            work_snapshot,
            sidecar_path,
            sidecar_snapshot,
            work_content: work_file.content.as_bytes().to_vec(),
            sidecar_content: render_hashes(&hashes)?,
            input_snapshots: work_file.input_snapshots.clone(),
        });
    }

    for item in &prepared {
        for (path, snapshot) in &item.input_snapshots {
            if !snapshot.unchanged(path)? {
                anyhow::bail!(
                    "{} changed while work files were being prepared",
                    path.display()
                );
            }
        }
    }

    let mut applied = Vec::new();
    for item in prepared {
        let result = (|| -> Result<()> {
            applied.push(atomic_replace(
                &item.work_path,
                &item.work_snapshot,
                &item.work_content,
            )?);
            applied.push(atomic_replace(
                &item.sidecar_path,
                &item.sidecar_snapshot,
                &item.sidecar_content,
            )?);
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(rollback_error) = rollback_writes(applied) {
                return Err(anyhow::anyhow!("{error}; additionally {rollback_error}"));
            }
            return Err(error);
        }
        generated.push((item.srctype, item.work_path.display().to_string()));
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
            println!("  {DIM}{}{RST}", escape_terminal(msg));
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
                println!("  {}", escape_terminal(line));
            }
        }
        if !res.stderr.is_empty() {
            for line in res.stderr.lines() {
                eprintln!("  {RED}{}{RST}", escape_terminal(line));
            }
        }
        if res.result {
            println!("{GRN}✓{RST} (exit {})", res.rtcode);
        } else {
            failed += 1;
            println!("{RED}✗{RST} (exit {})", res.rtcode);
        }
        println!();
        if res.interrupted {
            return Ok(res.rtcode);
        }
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

        let work_snapshot = match std::fs::symlink_metadata(&work_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => FileSnapshot::capture(&work_path)?,
        };
        found_any = true;

        println!("{BLD}[{srctype}]{RST} {}", work_path.display());

        let file_content = std::str::from_utf8(
            work_snapshot
                .content()
                .expect("an existing work file has content"),
        )?;
        let extracted = workback::extract_work_content(file_content, &srctype)?;

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
        if !extracted.preamble_present || !extracted.postamble_present {
            eprintln!(
                "  {YLW}warning:{RST} work file must contain exactly one preamble and postamble section"
            );
            eprintln!("  {YLW}aborted:{RST} refusing to sync this work file.");
            had_errors = true;
            continue;
        }

        let sidecar_path = hash_path(&work_path);
        let sidecar_snapshot = FileSnapshot::capture(&sidecar_path)?;
        let Some(stored) = parse_hashes(&sidecar_snapshot)? else {
            eprintln!("  {YLW}warning:{RST} stored work-file baseline is missing or unreadable");
            eprintln!(
                "  {YLW}aborted:{RST} refusing to sync without a baseline. Regenerate the work file and retry."
            );
            had_errors = true;
            continue;
        };

        let extracted_fnodes: HashSet<&str> = extracted
            .nodes
            .iter()
            .map(|(fnode, _, _)| fnode.as_str())
            .collect();
        let expected_fnodes: HashSet<&str> = stored.nodes.keys().map(String::as_str).collect();
        if extracted_fnodes != expected_fnodes {
            eprintln!(
                "  {YLW}warning:{RST} node sections do not match the stored work-file baseline"
            );
            eprintln!("  {YLW}aborted:{RST} refusing to sync a truncated work file.");
            had_errors = true;
            continue;
        }

        let mut new_node_hashes = HashMap::new();
        let mut validation_failed = false;

        let pre = extracted.preamble.as_deref().unwrap_or("");
        let pre_hash = content_hash(pre);
        let pre_changed = stored.preamble != pre_hash;
        let preamble_path = amble_path(&mdcroot, &srctype, "preamble");
        let preamble_snapshot = FileSnapshot::capture(&preamble_path)?;
        let current_pre = snapshot_text(
            &preamble_snapshot,
            crate::config::default_preamble(&srctype),
        )?;
        let current_pre = current_pre.trim_end_matches('\n');
        let pre_source_changed = stored.preamble != content_hash(current_pre);
        if pre_changed && pre_source_changed && pre != current_pre {
            eprintln!(
                "  {YLW}warning:{RST} conflict in preamble: both work file and source changed"
            );
            validation_failed = true;
        }

        let post = extracted.postamble.as_deref().unwrap_or("");
        let post_hash = content_hash(post);
        let post_changed = stored.postamble != post_hash;
        let postamble_path = amble_path(&mdcroot, &srctype, "postamble");
        let postamble_snapshot = FileSnapshot::capture(&postamble_path)?;
        let current_post = snapshot_text(
            &postamble_snapshot,
            crate::config::default_postamble(&srctype),
        )?;
        let current_post = current_post.trim_end_matches('\n');
        let post_source_changed = stored.postamble != content_hash(current_post);
        if post_changed && post_source_changed && post != current_post {
            eprintln!(
                "  {YLW}warning:{RST} conflict in postamble: both work file and source changed"
            );
            validation_failed = true;
        }

        struct PendingNode {
            full_fnode: String,
            title: String,
            abs_path: std::path::PathBuf,
            payload: Option<Vec<u8>>,
            needs_sync: bool,
            original: FileSnapshot,
        }

        let mut pending_nodes = Vec::new();

        // Resolve and load every target before writing any part of this work file.
        for (fnode, extracted_title, content) in &extracted.nodes {
            let hash = content_hash(content);
            new_node_hashes.insert(fnode.clone(), hash.clone());

            match cache.resolve_ref(fnode, None) {
                Ok((full_fnode, title, abs_path)) => {
                    let original = FileSnapshot::capture(&abs_path)?;
                    let mut node = match crate::mdocnode::MdocNode::load(&mdcroot, &abs_path) {
                        Ok(node) => node,
                        Err(error) => {
                            eprintln!(
                                "  {YLW}warning:{RST} cannot load target for fnode {fnode}: {error}"
                            );
                            validation_failed = true;
                            continue;
                        }
                    };
                    if !original.unchanged(&abs_path)? {
                        eprintln!(
                            "  {YLW}warning:{RST} target for fnode {fnode} changed while it was being loaded"
                        );
                        validation_failed = true;
                        continue;
                    }

                    if node.fnode != full_fnode {
                        eprintln!(
                            "  {YLW}warning:{RST} target for fnode {fnode} no longer matches its index entry"
                        );
                        validation_failed = true;
                        continue;
                    }

                    if extracted_title != &node.title {
                        let short = short_fnode(fnode);
                        eprintln!(
                            "  {YLW}warning:{RST} title of {short} was modified\n\
                             {DIM}    original:{RST} {BLD}{}{RST}\n\
                             {DIM}    modified:{RST} {BLD}{extracted_title}{RST}",
                            node.title
                        );
                        validation_failed = true;
                    }

                    let Some(baseline_hash) = stored.nodes.get(fnode) else {
                        eprintln!("  {YLW}warning:{RST} fnode {fnode} has no stored baseline");
                        validation_failed = true;
                        continue;
                    };
                    let current_content = node
                        .blocks
                        .iter()
                        .filter(|b| b.srctype == srctype)
                        .map(|b| b.content.trim_end_matches('\n'))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let current_hash = content_hash(&current_content);
                    let work_changed = baseline_hash != &hash;
                    let source_changed = baseline_hash != &current_hash;

                    if work_changed && source_changed && hash != current_hash {
                        let short = short_fnode(fnode);
                        eprintln!(
                            "  {YLW}warning:{RST} conflict in fnode {short}: both the work section and .mdoc source changed"
                        );
                        validation_failed = true;
                    }

                    if content.lines().any(|line| line.trim() == "@end") {
                        let short = short_fnode(fnode);
                        eprintln!(
                            "  {YLW}warning:{RST} fnode {short} contains a reserved @end line"
                        );
                        validation_failed = true;
                    }

                    let needs_sync = work_changed && hash != current_hash;
                    if needs_sync {
                        let new_content = if content.is_empty() {
                            String::new()
                        } else {
                            format!("{content}\n")
                        };
                        match node
                            .blocks
                            .iter_mut()
                            .find(|block| block.srctype == srctype)
                        {
                            Some(block) => block.content = new_content,
                            None => node.blocks.push(crate::mdocnode::SrcBlock {
                                srctype: srctype.clone(),
                                content: new_content,
                                metadata: std::collections::HashMap::new(),
                            }),
                        }
                    }
                    let payload = if needs_sync {
                        Some(node.render_payload()?.into_bytes())
                    } else {
                        None
                    };
                    pending_nodes.push(PendingNode {
                        full_fnode,
                        title,
                        original,
                        abs_path,
                        payload,
                        needs_sync,
                    });
                }
                Err(error) => {
                    eprintln!("  {YLW}warning:{RST} cannot resolve fnode {fnode}: {error}");
                    validation_failed = true;
                }
            }
        }

        if validation_failed {
            eprintln!(
                "  {YLW}aborted:{RST} refusing to sync this work file; no changes were written."
            );
            had_errors = true;
            continue;
        }

        let new_hashes = WorkHashes {
            version: 1,
            file: content_hash(file_content),
            preamble: pre_hash,
            postamble: post_hash,
            nodes: new_node_hashes,
        };
        let sidecar_content = render_hashes(&new_hashes)?;
        let mut synced_labels = Vec::new();

        let preconditions_unchanged = work_snapshot.unchanged(&work_path)?
            && sidecar_snapshot.unchanged(&sidecar_path)?
            && preamble_snapshot.unchanged(&preamble_path)?
            && postamble_snapshot.unchanged(&postamble_path)?
            && pending_nodes.iter().all(|pending| {
                pending
                    .original
                    .unchanged(&pending.abs_path)
                    .unwrap_or(false)
            });
        if !preconditions_unchanged {
            eprintln!(
                "  {YLW}aborted:{RST} a work, sidecar, amble, or node file changed during validation; no changes were written."
            );
            had_errors = true;
            continue;
        }

        let mut applied = Vec::new();
        let apply_result = (|| -> Result<()> {
            if pre_changed {
                let content = if pre.is_empty() {
                    String::new()
                } else {
                    format!("{pre}\n")
                };
                applied.push(atomic_replace(
                    &preamble_path,
                    &preamble_snapshot,
                    content.as_bytes(),
                )?);
                synced_labels.push("preamble".to_string());
            }
            if post_changed {
                let content = if post.is_empty() {
                    String::new()
                } else {
                    format!("{post}\n")
                };
                applied.push(atomic_replace(
                    &postamble_path,
                    &postamble_snapshot,
                    content.as_bytes(),
                )?);
                synced_labels.push("postamble".to_string());
            }

            for pending in &pending_nodes {
                if !pending.needs_sync {
                    continue;
                }
                applied.push(atomic_replace(
                    &pending.abs_path,
                    &pending.original,
                    pending
                        .payload
                        .as_deref()
                        .expect("changed node has payload"),
                )?);
                let rel_path = crate::workspace::to_rel_path(&mdcroot, &pending.abs_path);
                synced_labels.push(fmt_item(
                    &pending.full_fnode,
                    &pending.title,
                    &rel_path,
                    false,
                ));
            }
            if !work_snapshot.unchanged(&work_path)? {
                anyhow::bail!("work file changed while source updates were being applied");
            }
            applied.push(atomic_replace(
                &sidecar_path,
                &sidecar_snapshot,
                &sidecar_content,
            )?);
            Ok(())
        })();

        if let Err(error) = apply_result {
            eprintln!("  {YLW}aborted:{RST} {error}");
            if let Err(rollback_error) = rollback_writes(applied) {
                return Err(anyhow::anyhow!("{error}; additionally {rollback_error}"));
            }
            had_errors = true;
            continue;
        }

        for label in synced_labels {
            println!("  synced: {label}");
            total_synced += 1;
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
