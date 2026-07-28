mod manifest;
mod mirror;
mod transaction;

use anyhow::{bail, Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::config::builtin_srctypes;
use crate::mdocnode::MdocNode;
use crate::workspace::{iter_mdoc_files, FileSnapshot};

use manifest::{
    decode_source_path, encode_source_path, parse_manifest, BlockBaseline, LoadedManifest,
    MANIFEST_NAME,
};
use mirror::{
    back_state, existing_output_path, output_path, prepare_output_path, validate_source_relative,
    MirrorState,
};
use transaction::{
    apply_changes, PreparedManifest, PreparedRemoval, PreparedRename, PreparedWrite,
};

pub(crate) struct Issue {
    pub path: PathBuf,
    pub srctype: Option<String>,
    pub message: String,
}

pub(crate) struct SyncReport {
    pub valid_mdocs: usize,
    pub updated: usize,
    pub removed: usize,
    pub dirty: Vec<Issue>,
    pub conflicts: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

#[derive(Default)]
pub(crate) struct BackReport {
    pub updated_blocks: usize,
    pub updated_mdocs: usize,
    pub conflicts: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

enum Reconciliation<'a> {
    Unchanged,
    MdocChanged,
    MirrorChanged(MirrorState<'a>),
    Converged(MirrorState<'a>),
    Conflict,
}

fn reconcile<'a>(
    baseline: &BlockBaseline,
    mdoc_content: &[u8],
    mdoc_present: bool,
    raw_content: Option<&'a [u8]>,
) -> Reconciliation<'a> {
    let mdoc_changed = !baseline.matches_state(mdoc_content, mdoc_present);
    let raw_changed = !baseline.matches_raw(raw_content);
    match (mdoc_changed, raw_changed) {
        (false, false) => Reconciliation::Unchanged,
        (true, false) => Reconciliation::MdocChanged,
        (false, true) => Reconciliation::MirrorChanged(back_state(raw_content, baseline.present)),
        (true, true) => {
            let raw_state = back_state(raw_content, baseline.present);
            if raw_state.content.as_ref() == mdoc_content && raw_state.present == mdoc_present {
                Reconciliation::Converged(raw_state)
            } else {
                Reconciliation::Conflict
            }
        }
    }
}

pub(crate) fn sync(mutation_lock: &crate::workspace::WorkspaceMutationLock) -> Result<SyncReport> {
    let mdcroot = mutation_lock.root()?.to_path_buf();
    let manifest_path = mdcroot.join(".mdc").join(MANIFEST_NAME);
    let manifest_snapshot = FileSnapshot::capture(&manifest_path)?;
    let LoadedManifest {
        manifest: mut new_manifest,
        legacy_sources,
    } = parse_manifest(&manifest_snapshot, &manifest_path)?;

    let mut source_files = iter_mdoc_files(&mdcroot).collect::<Result<Vec<_>>>()?;
    source_files.sort();
    let mut current_source_ids = BTreeSet::new();
    let mut writes = Vec::new();
    let mut removals = Vec::new();
    let mut renames = Vec::new();
    let mut inputs = Vec::new();
    let mut dirty = Vec::new();
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let mut desired_outputs = HashMap::new();
    let mut valid_mdocs = 0;
    let mut had_invalid_mdoc = false;

    for source_path in source_files {
        let relative = source_path
            .strip_prefix(&mdcroot)
            .with_context(|| format!("relativizing {}", source_path.display()))?
            .to_path_buf();
        validate_source_relative(&relative)?;
        let source_id = encode_source_path(&relative);
        current_source_ids.insert(source_id.clone());
        let snapshot = FileSnapshot::capture(&source_path)?;
        let node = match snapshot.content() {
            Some(content) => MdocNode::load_bytes(&source_path, content),
            None => Err(anyhow::anyhow!(
                "mdoc file disappeared: {}",
                source_path.display()
            )),
        };
        let node = match node {
            Ok(node) => node,
            Err(error) => {
                had_invalid_mdoc = true;
                inputs.push((source_path, snapshot));
                warnings.push(issue(&relative, None, error.to_string()));
                continue;
            }
        };
        valid_mdocs += 1;

        let source_baseline = new_manifest.sources.entry(source_id).or_default();
        for srctype in builtin_srctypes() {
            let (mdoc_content, mdoc_present) = block_state(&node, srctype);
            let raw_path = prepare_output_path(&mdcroot, &relative, srctype)?;
            let raw_snapshot = FileSnapshot::capture(&raw_path)?;
            if let Some(identity) = raw_snapshot.identity() {
                desired_outputs.insert(identity.clone(), raw_path.clone());
            }
            let raw_content = raw_snapshot.content();

            if let Some(baseline) = source_baseline.blocks.get_mut(srctype) {
                match reconcile(baseline, mdoc_content, mdoc_present, raw_content) {
                    Reconciliation::Unchanged => {}
                    Reconciliation::MdocChanged => {
                        if raw_content != Some(mdoc_content) {
                            writes.push(PreparedWrite {
                                path: raw_path,
                                snapshot: raw_snapshot,
                                content: mdoc_content.to_vec(),
                            });
                        }
                        baseline.update(mdoc_content, mdoc_present);
                    }
                    Reconciliation::MirrorChanged(_) => dirty.push(issue(
                        &relative,
                        Some(srctype),
                        "source mirror has uncommitted changes; run `mdc back`",
                    )),
                    Reconciliation::Converged(raw_state) => {
                        if raw_content != Some(raw_state.content.as_ref()) {
                            let normalized_content = raw_state.content.into_owned();
                            writes.push(PreparedWrite {
                                path: raw_path,
                                snapshot: raw_snapshot,
                                content: normalized_content,
                            });
                        }
                        baseline.update(mdoc_content, mdoc_present);
                    }
                    Reconciliation::Conflict => conflicts.push(issue(
                        &relative,
                        Some(srctype),
                        "mdoc block and source mirror both changed",
                    )),
                }
            } else {
                if raw_content.is_none() {
                    writes.push(PreparedWrite {
                        path: raw_path,
                        snapshot: raw_snapshot,
                        content: mdoc_content.to_vec(),
                    });
                } else if raw_content != Some(mdoc_content) {
                    conflicts.push(issue(
                        &relative,
                        Some(srctype),
                        "source mirror has no baseline and differs from the mdoc block",
                    ));
                }
                source_baseline.blocks.insert(
                    srctype.to_string(),
                    BlockBaseline::new(mdoc_content, mdoc_present),
                );
            }
        }
        inputs.push((source_path, snapshot));
    }

    let orphaned_sources: Vec<_> = new_manifest
        .sources
        .extract_if(.., |source_id, _| !current_source_ids.contains(source_id))
        .collect();
    for (source_id, mut source_baseline) in orphaned_sources {
        let source = decode_source_path(&source_id)?;
        if had_invalid_mdoc {
            conflicts.push(issue(
                &source,
                None,
                "raw cleanup was deferred because an invalid mdoc may be its renamed source",
            ));
            new_manifest.sources.insert(source_id, source_baseline);
            continue;
        }
        for srctype in builtin_srctypes() {
            let Some(baseline) = source_baseline.blocks.get(srctype) else {
                continue;
            };
            let Some((path, type_root)) = existing_output_path(&mdcroot, &source, srctype)? else {
                source_baseline.blocks.remove(srctype);
                continue;
            };
            let snapshot = FileSnapshot::capture(&path)?;
            if let Some(desired_path) = snapshot
                .identity()
                .and_then(|identity| desired_outputs.get(identity))
            {
                if path != *desired_path && baseline.matches_raw(snapshot.content()) {
                    renames.push(PreparedRename {
                        from: path,
                        to: desired_path.clone(),
                        snapshot,
                    });
                }
                source_baseline.blocks.remove(srctype);
                continue;
            }
            if baseline.matches_raw(snapshot.content()) || matches!(snapshot, FileSnapshot::Missing)
            {
                if !matches!(snapshot, FileSnapshot::Missing) {
                    removals.push(PreparedRemoval {
                        path,
                        type_root,
                        snapshot,
                    });
                }
                source_baseline.blocks.remove(srctype);
            } else {
                conflicts.push(issue(
                    &source,
                    Some(srctype),
                    "source mdoc was removed but its mirror has uncommitted changes",
                ));
            }
        }
        if !source_baseline.blocks.is_empty() {
            new_manifest.sources.insert(source_id, source_baseline);
        }
    }

    let mut unresolved_legacy_orphans = false;
    for source_id in legacy_sources.difference(&current_source_ids) {
        if had_invalid_mdoc {
            unresolved_legacy_orphans = true;
            continue;
        }
        let source = decode_source_path(source_id)?;
        for srctype in builtin_srctypes() {
            let Some((path, _)) = existing_output_path(&mdcroot, &source, srctype)? else {
                continue;
            };
            let snapshot = FileSnapshot::capture(&path)?;
            if !matches!(snapshot, FileSnapshot::Missing) {
                unresolved_legacy_orphans = true;
            }
        }
    }
    if unresolved_legacy_orphans {
        bail!(
            "cannot upgrade source-blocks.json while orphaned v1 mirrors are unresolved; restore the mdoc or remove the orphaned mirrors explicitly"
        );
    }

    let source_write_count = writes.len();
    let removed = removals.len();
    apply_changes(
        &mdcroot,
        PreparedManifest {
            path: &manifest_path,
            snapshot: &manifest_snapshot,
            content: &new_manifest,
        },
        &inputs,
        writes,
        removals,
        renames,
    )?;
    Ok(SyncReport {
        valid_mdocs,
        updated: source_write_count,
        removed,
        dirty,
        conflicts,
        warnings,
    })
}

pub(crate) fn back(mutation_lock: &crate::workspace::WorkspaceMutationLock) -> Result<BackReport> {
    let mdcroot = mutation_lock.root()?.to_path_buf();
    let manifest_path = mdcroot.join(".mdc").join(MANIFEST_NAME);
    let manifest_snapshot = FileSnapshot::capture(&manifest_path)?;
    let loaded = parse_manifest(&manifest_snapshot, &manifest_path)?;
    if !loaded.legacy_sources.is_empty() {
        bail!("source block manifest must be upgraded with `mdc sync` before `mdc back`");
    }
    let mut manifest = loaded.manifest;
    if manifest.sources.is_empty() {
        return Ok(BackReport::default());
    }

    let mut node_writes = Vec::new();
    let mut raw_writes = Vec::new();
    let mut inputs = Vec::new();
    let mut updated_blocks = 0;
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();

    for (source_id, source_baseline) in &mut manifest.sources {
        let relative = decode_source_path(source_id)?;
        let source_path = mdcroot.join(&relative);
        let source_snapshot = FileSnapshot::capture(&source_path)?;
        if matches!(source_snapshot, FileSnapshot::Missing) {
            conflicts.push(issue(
                &relative,
                None,
                "source mdoc is missing; run `mdc sync` to reconcile its mirrors",
            ));
            continue;
        }
        let mut node = match MdocNode::load_bytes(
            &source_path,
            source_snapshot
                .content()
                .expect("missing source snapshot handled above"),
        ) {
            Ok(node) => node,
            Err(error) => {
                let mut raw_dirty = false;
                for (srctype, baseline) in &source_baseline.blocks {
                    let raw_path = existing_output_path(&mdcroot, &relative, srctype)?
                        .map(|(path, _)| path)
                        .unwrap_or_else(|| output_path(&mdcroot, &relative, srctype));
                    let raw_snapshot = FileSnapshot::capture(&raw_path)?;
                    if !baseline.matches_raw(raw_snapshot.content()) {
                        raw_dirty = true;
                        break;
                    }
                }
                if raw_dirty {
                    conflicts.push(issue(
                        &relative,
                        None,
                        format!("cannot import dirty mirrors into invalid mdoc: {error}"),
                    ));
                } else {
                    warnings.push(issue(&relative, None, error.to_string()));
                }
                inputs.push((source_path, source_snapshot));
                continue;
            }
        };

        let mut node_changed = false;
        for (srctype, baseline) in &mut source_baseline.blocks {
            let (mdoc_content, mdoc_present) = block_state(&node, srctype);
            let raw_path = existing_output_path(&mdcroot, &relative, srctype)?
                .map(|(path, _)| path)
                .unwrap_or_else(|| output_path(&mdcroot, &relative, srctype));
            let raw_snapshot = FileSnapshot::capture(&raw_path)?;
            let raw_content = raw_snapshot.content();
            let raw_state = match reconcile(baseline, mdoc_content, mdoc_present, raw_content) {
                Reconciliation::Unchanged => continue,
                Reconciliation::MdocChanged => {
                    conflicts.push(issue(
                        &relative,
                        Some(srctype),
                        "mdoc block has uncommitted changes; run `mdc sync`",
                    ));
                    continue;
                }
                Reconciliation::MirrorChanged(raw_state) | Reconciliation::Converged(raw_state) => {
                    raw_state
                }
                Reconciliation::Conflict => {
                    conflicts.push(issue(
                        &relative,
                        Some(srctype),
                        "mdoc block and source mirror both changed",
                    ));
                    continue;
                }
            };

            let content = raw_state.content.as_ref();
            let present = raw_state.present;
            let import = content != mdoc_content || present != mdoc_present;
            if import {
                if present {
                    let content = std::str::from_utf8(content)
                        .context("source mirror is not valid UTF-8")?
                        .to_string();
                    node.upsert_source_block(srctype, content)?;
                } else {
                    node.remove_source_block(srctype);
                }
                node_changed = true;
                updated_blocks += 1;
            }
            if !baseline.matches_state(content, present) {
                baseline.update(content, present);
            }
            if raw_content != Some(content) {
                let raw_missing = raw_content.is_none();
                let normalized_content = raw_state.content.into_owned();
                let path = if raw_missing {
                    prepare_output_path(&mdcroot, &relative, srctype)?
                } else {
                    raw_path
                };
                raw_writes.push(PreparedWrite {
                    path,
                    snapshot: raw_snapshot,
                    content: normalized_content,
                });
            }
        }
        if node_changed {
            node_writes.push(PreparedWrite {
                path: source_path,
                snapshot: source_snapshot,
                content: node.render()?.into_bytes(),
            });
        } else {
            inputs.push((source_path, source_snapshot));
        }
    }

    let updated_mdocs = node_writes.len();
    node_writes.append(&mut raw_writes);
    apply_changes(
        &mdcroot,
        PreparedManifest {
            path: &manifest_path,
            snapshot: &manifest_snapshot,
            content: &manifest,
        },
        &inputs,
        node_writes,
        Vec::new(),
        Vec::new(),
    )?;
    Ok(BackReport {
        updated_blocks,
        updated_mdocs,
        conflicts,
        warnings,
    })
}

pub(crate) fn targets(mdcroot: &Path, source_path: &Path) -> Result<Vec<(String, PathBuf)>> {
    let source = source_path
        .strip_prefix(mdcroot)
        .with_context(|| format!("relativizing {}", source_path.display()))?;
    validate_source_relative(source)?;
    let node = MdocNode::load(source_path)?;
    let mut targets = Vec::new();
    for srctype in builtin_srctypes() {
        let path = output_path(mdcroot, source, srctype);
        let snapshot = FileSnapshot::capture(&path)?;
        let block_present = node.source_block(srctype).is_some();
        let raw_nonempty = snapshot
            .content()
            .is_some_and(|content| !content.is_empty());
        if block_present || raw_nonempty {
            if matches!(snapshot, FileSnapshot::Missing) {
                bail!("source mirror is missing: {}", path.display());
            }
            targets.push((srctype.to_string(), path));
        }
    }
    Ok(targets)
}

fn block_state<'a>(node: &'a MdocNode, srctype: &str) -> (&'a [u8], bool) {
    match node.source_block(srctype) {
        Some(block) => (block.content.as_bytes(), true),
        None => (&[], false),
    }
}

fn issue(path: &Path, srctype: Option<&str>, message: impl Into<String>) -> Issue {
    Issue {
        path: path.to_path_buf(),
        srctype: srctype.map(str::to_string),
        message: message.into(),
    }
}
