mod manifest;
mod mirror;
mod transaction;

use anyhow::{bail, Context, Result};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::builtin_srctypes;
use crate::mdocnode::MdocNode;
use crate::workspace::{
    ensure_regular_directory_tree, iter_mdoc_files, FileSnapshot, FileSnapshotBatch,
    ReadFileSnapshot,
};

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

struct ScannedSyncSource {
    path: PathBuf,
    relative: PathBuf,
    source_id: String,
    snapshot: FileSnapshot,
    node: Result<MdocNode>,
}

const SYNC_SOURCE_BATCH: usize = 2048;

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
    let _profile = crate::profile::scope("workdraft::sync");
    let mdcroot = mutation_lock.root()?.to_path_buf();
    let manifest_path = mdcroot.join(".mdc").join(MANIFEST_NAME);
    let (manifest_snapshot, loaded_manifest) = {
        let _phase = crate::profile::scope("workdraft::load_manifest");
        let snapshot = FileSnapshot::capture(&manifest_path)?;
        let loaded = parse_manifest(&snapshot, &manifest_path)?;
        (snapshot, loaded)
    };
    let LoadedManifest {
        manifest: mut new_manifest,
        legacy_sources,
    } = loaded_manifest;

    let source_files = {
        let _phase = crate::profile::scope("workdraft::enumerate_mdocs");
        let mut files = iter_mdoc_files(&mdcroot).collect::<Result<Vec<_>>>()?;
        files.sort();
        files
    };
    let reconcile_profile = crate::profile::scope("workdraft::reconcile_mdocs");
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
    let mut prepared_output_parents = HashSet::new();
    let mut write_snapshots = FileSnapshotBatch::new(&mdcroot)?;
    let mut source_files = source_files.into_iter();

    loop {
        let source_batch: Vec<_> = source_files.by_ref().take(SYNC_SOURCE_BATCH).collect();
        if source_batch.is_empty() {
            break;
        }
        let scanned_sources = {
            let _phase = crate::profile::scope("workdraft::read_mdocs_parallel");
            scan_sources_parallel(&mdcroot, &source_batch)?
        };
        let mirror_paths = {
            let _phase = crate::profile::scope("workdraft::prepare_output_paths");
            let mut paths = Vec::new();
            for source in &scanned_sources {
                if source.node.is_err() {
                    continue;
                }
                for srctype in builtin_srctypes() {
                    let raw_path = output_path(&mdcroot, &source.relative, srctype);
                    let raw_parent = raw_path
                        .parent()
                        .expect("source mirror always has a parent directory");
                    if prepared_output_parents.insert(raw_parent.to_path_buf()) {
                        ensure_regular_directory_tree(&mdcroot, raw_parent)?;
                    }
                    paths.push(raw_path);
                }
            }
            paths
        };
        let mirror_snapshots = {
            let _phase = crate::profile::scope("workdraft::read_mirrors_parallel");
            read_files_parallel(&mdcroot, &mirror_paths)?
        };
        let mut mirror_paths = mirror_paths.into_iter();
        let mut mirror_snapshots = mirror_snapshots.into_iter();

        let classify_profile = crate::profile::scope("workdraft::classify_reconciliation");
        for source in scanned_sources {
            let ScannedSyncSource {
                path: source_path,
                relative,
                source_id,
                snapshot,
                node,
            } = source;
            current_source_ids.insert(source_id.clone());
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
                let raw_path = mirror_paths
                    .next()
                    .expect("every valid mdoc has five prepared mirror paths");
                let raw_snapshot = mirror_snapshots
                    .next()
                    .expect("every valid mdoc has five mirror snapshots");
                if let Some(identity) = raw_snapshot.as_ref().map(ReadFileSnapshot::identity) {
                    desired_outputs.insert(identity.clone(), raw_path.clone());
                }
                let raw_content = raw_snapshot.as_ref().map(ReadFileSnapshot::content);

                if let Some(baseline) = source_baseline.blocks.get_mut(srctype) {
                    match reconcile(baseline, mdoc_content, mdoc_present, raw_content) {
                        Reconciliation::Unchanged => {}
                        Reconciliation::MdocChanged => {
                            if raw_content != Some(mdoc_content) {
                                writes.push(prepare_mirror_write(
                                    &mut write_snapshots,
                                    raw_path,
                                    raw_content,
                                    mdoc_content.to_vec(),
                                )?);
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
                                writes.push(prepare_mirror_write(
                                    &mut write_snapshots,
                                    raw_path,
                                    raw_content,
                                    normalized_content,
                                )?);
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
                        writes.push(prepare_mirror_write(
                            &mut write_snapshots,
                            raw_path,
                            raw_content,
                            mdoc_content.to_vec(),
                        )?);
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
        debug_assert!(mirror_paths.next().is_none());
        debug_assert!(mirror_snapshots.next().is_none());
        drop(classify_profile);
    }
    write_snapshots.finish()?;
    drop(reconcile_profile);

    let orphan_profile = crate::profile::scope("workdraft::reconcile_orphans");
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
    drop(orphan_profile);

    let source_write_count = writes.len();
    let removed = removals.len();
    {
        let _phase = crate::profile::scope("workdraft::apply_changes");
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
    }
    Ok(SyncReport {
        valid_mdocs,
        updated: source_write_count,
        removed,
        dirty,
        conflicts,
        warnings,
    })
}

fn prepare_mirror_write(
    snapshots: &mut FileSnapshotBatch,
    path: PathBuf,
    observed_content: Option<&[u8]>,
    content: Vec<u8>,
) -> Result<PreparedWrite> {
    let snapshot = snapshots.capture(&path)?;
    if snapshot.content() != observed_content {
        bail!(
            "{} changed during source block reconciliation",
            path.display()
        );
    }
    Ok(PreparedWrite {
        path,
        snapshot,
        content,
    })
}

fn scan_sources_parallel(root: &Path, paths: &[PathBuf]) -> Result<Vec<ScannedSyncSource>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallel_worker_count(paths.len());
    let chunk_size = paths.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in paths.chunks(chunk_size) {
            workers.push(scope.spawn(move || -> Result<Vec<ScannedSyncSource>> {
                let mut snapshots = FileSnapshotBatch::new(root)?;
                let mut result = Vec::with_capacity(chunk.len());
                for source_path in chunk {
                    let relative = source_path
                        .strip_prefix(root)
                        .with_context(|| format!("relativizing {}", source_path.display()))?
                        .to_path_buf();
                    validate_source_relative(&relative)?;
                    let source_id = encode_source_path(&relative);
                    let snapshot = snapshots.capture(source_path)?;
                    let node = match snapshot.content() {
                        Some(content) => MdocNode::load_bytes(source_path, content),
                        None => Err(anyhow::anyhow!(
                            "mdoc file disappeared: {}",
                            source_path.display()
                        )),
                    };
                    result.push(ScannedSyncSource {
                        path: source_path.clone(),
                        relative,
                        source_id,
                        snapshot,
                        node,
                    });
                }
                snapshots.finish()?;
                Ok(result)
            }));
        }
        join_snapshot_workers(workers, paths.len(), "mdoc snapshot worker panicked")
    })
}

pub(super) fn read_files_parallel(
    root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<Option<ReadFileSnapshot>>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallel_worker_count(paths.len());
    let chunk_size = paths.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in paths.chunks(chunk_size) {
            workers.push(
                scope.spawn(move || -> Result<Vec<Option<ReadFileSnapshot>>> {
                    let mut snapshots = FileSnapshotBatch::new(root)?;
                    let mut result = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        result.push(snapshots.capture_read(path)?);
                    }
                    snapshots.finish()?;
                    Ok(result)
                }),
            );
        }
        join_snapshot_workers(workers, paths.len(), "file snapshot worker panicked")
    })
}

pub(super) fn capture_files_parallel(root: &Path, paths: &[PathBuf]) -> Result<Vec<FileSnapshot>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallel_worker_count(paths.len());
    let chunk_size = paths.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in paths.chunks(chunk_size) {
            workers.push(scope.spawn(move || -> Result<Vec<FileSnapshot>> {
                let mut snapshots = FileSnapshotBatch::new(root)?;
                let mut result = Vec::with_capacity(chunk.len());
                for path in chunk {
                    result.push(snapshots.capture(path)?);
                }
                snapshots.finish()?;
                Ok(result)
            }));
        }
        join_snapshot_workers(workers, paths.len(), "file snapshot worker panicked")
    })
}

fn parallel_worker_count(item_count: usize) -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(8)
        .min(item_count)
}

fn join_snapshot_workers<T>(
    workers: Vec<std::thread::ScopedJoinHandle<'_, Result<Vec<T>>>>,
    item_count: usize,
    panic_message: &'static str,
) -> Result<Vec<T>> {
    let mut result = Vec::with_capacity(item_count);
    let mut first_error = None;
    for worker in workers {
        match worker.join() {
            Ok(Ok(items)) => result.extend(items),
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(_) => {
                if first_error.is_none() {
                    first_error = Some(anyhow::anyhow!(panic_message));
                }
            }
        }
    }
    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(result)
    }
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
