mod manifest;
mod mirror;
mod transaction;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::config::builtin_srctypes;
use crate::mdocnode::MdocNode;
use crate::workspace::{
    iter_mdoc_files, FileSnapshot, FileSnapshotBatch, FileStatSnapshot, ReadFileSnapshot,
};

use manifest::{
    decode_source_path, encode_source_path, parse_manifest, LoadedManifest, SourceBaseline,
    SourceBlockManifest, MANIFEST_NAME,
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
    pub source_files: usize,
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
    pub index_refreshed: bool,
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
    snapshot: ReadFileSnapshot,
    node: Result<MdocNode>,
}

struct BackScanRequest {
    relative: PathBuf,
    source_path: PathBuf,
    srctypes: Vec<&'static str>,
}

struct ScannedBackSource {
    relative: PathBuf,
    source_path: PathBuf,
    source_snapshot: Option<ReadFileSnapshot>,
    mirrors: Vec<(&'static str, PathBuf, Option<ReadFileSnapshot>)>,
}

struct ObservationSpec {
    source_id: String,
    srctype: String,
    path: PathBuf,
}

struct MirrorChanges<'a> {
    root: &'a Path,
    source: &'a Path,
    snapshots: &'a mut FileSnapshotBatch,
    writes: &'a mut Vec<PreparedWrite>,
    removals: &'a mut Vec<PreparedRemoval>,
}

impl MirrorChanges<'_> {
    fn queue(
        &mut self,
        srctype: &str,
        mut path: PathBuf,
        observed_content: Option<&[u8]>,
        content: &[u8],
        present: bool,
    ) -> Result<()> {
        if present && observed_content != Some(content) {
            if observed_content.is_none() {
                path = prepare_output_path(self.root, self.source, srctype)?;
            }
            let snapshot = self.snapshots.capture(&path)?;
            ensure_observed_content(&path, &snapshot, observed_content)?;
            self.writes.push(PreparedWrite {
                path,
                snapshot,
                content: content.to_vec(),
            });
        } else if !present && observed_content.is_some() {
            self.queue_removal(srctype, path, observed_content, false)?;
        }
        Ok(())
    }

    fn queue_sparse_migration_removal(
        &mut self,
        srctype: &str,
        path: PathBuf,
        observed_content: Option<&[u8]>,
    ) -> Result<()> {
        self.queue_removal(srctype, path, observed_content, true)
    }

    fn queue_removal(
        &mut self,
        srctype: &str,
        path: PathBuf,
        observed_content: Option<&[u8]>,
        recoverable: bool,
    ) -> Result<()> {
        let snapshot = self.snapshots.capture(&path)?;
        ensure_observed_content(&path, &snapshot, observed_content)?;
        self.removals.push(PreparedRemoval {
            path,
            type_root: self.root.join(".mdc").join(srctype).join("Lib"),
            snapshot,
            recoverable,
        });
        Ok(())
    }
}

const SYNC_SOURCE_BATCH: usize = 2048;
const MAX_PARALLEL_WORKERS: usize = 12;

fn reconcile<'a>(
    baseline: &SourceBaseline,
    srctype: &str,
    mdoc_content: &[u8],
    mdoc_present: bool,
    raw_content: Option<&'a [u8]>,
) -> Reconciliation<'a> {
    let mdoc_changed = !baseline.matches_state(srctype, mdoc_content, mdoc_present);
    let raw_changed = !baseline.matches_raw(srctype, raw_content);
    match (mdoc_changed, raw_changed) {
        (false, false) => Reconciliation::Unchanged,
        (true, false) => Reconciliation::MdocChanged,
        (false, true) => Reconciliation::MirrorChanged(back_state(raw_content)),
        (true, true) => {
            let raw_state = back_state(raw_content);
            if raw_state.content.as_ref() == mdoc_content && raw_state.present == mdoc_present {
                Reconciliation::Converged(raw_state)
            } else {
                Reconciliation::Conflict
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn sync(mutation_lock: &crate::workspace::WorkspaceMutationLock) -> Result<SyncReport> {
    sync_with_cache(mutation_lock, None)
}

pub(crate) fn sync_cached(
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    cache: &mut crate::indcache::IndCache,
) -> Result<SyncReport> {
    sync_with_cache(mutation_lock, Some(cache))
}

fn sync_with_cache(
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    mut cache: Option<&mut crate::indcache::IndCache>,
) -> Result<SyncReport> {
    let _profile = crate::profile::scope("workdraft::sync");
    let mdcroot = mutation_lock.root()?.to_path_buf();
    let manifest_path = mdcroot.join(".mdc").join(MANIFEST_NAME);
    let (manifest_snapshot, loaded_manifest) = {
        let _phase = crate::profile::scope("workdraft::load_manifest");
        let snapshot = FileSnapshot::capture_beneath(&mdcroot, &manifest_path)?;
        let loaded = parse_manifest(&snapshot, &manifest_path)?;
        (snapshot, loaded)
    };
    let LoadedManifest {
        manifest: mut new_manifest,
        legacy_sources,
        needs_sparse_migration,
    } = loaded_manifest;

    let source_files = {
        let _phase = crate::profile::scope("workdraft::enumerate_mdocs");
        let mut files = iter_mdoc_files(&mdcroot).collect::<Result<Vec<_>>>()?;
        files.sort();
        files
    };
    let observation_changes = if legacy_sources.is_empty() && !needs_sparse_migration {
        check_observation_cache(
            cache.as_deref(),
            &mdcroot,
            &manifest_snapshot,
            &new_manifest,
            &source_files,
        )?
    } else {
        None
    };
    if let Some((cached, changed)) = &observation_changes {
        if changed.is_empty() {
            require_fast_path_current(mutation_lock, &mdcroot, &manifest_path, &manifest_snapshot)?;
            return Ok(SyncReport {
                valid_mdocs: cached.valid_mdocs,
                source_files: cached.source_files,
                updated: 0,
                removed: 0,
                dirty: Vec::new(),
                conflicts: Vec::new(),
                warnings: Vec::new(),
            });
        }
    }
    let observation_sources = source_files.clone();
    let incremental_changed = observation_changes
        .as_ref()
        .map(|(_, changed)| changed.clone());
    let source_files = if let Some(changed) = &incremental_changed {
        source_files
            .into_iter()
            .filter(|path| {
                path.strip_prefix(&mdcroot)
                    .ok()
                    .map(encode_source_path)
                    .is_some_and(|source_id| changed.contains(&source_id))
            })
            .collect()
    } else {
        source_files
    };
    let reconcile_profile = crate::profile::scope("workdraft::reconcile_mdocs");
    let mut current_source_ids = if incremental_changed.is_some() {
        observation_sources
            .iter()
            .filter_map(|path| path.strip_prefix(&mdcroot).ok())
            .map(encode_source_path)
            .collect()
    } else {
        BTreeSet::new()
    };
    let mut writes = Vec::new();
    let mut removals = Vec::new();
    let mut renames = Vec::new();
    let mut inputs = Vec::new();
    let mut dirty = Vec::new();
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let mut desired_outputs = HashMap::new();
    let mut expected_source_contents = HashMap::new();
    let mut valid_mdocs = observation_changes
        .as_ref()
        .map_or(0, |(cached, _)| cached.valid_mdocs);
    let mut exported_source_files = observation_changes
        .as_ref()
        .map_or(0, |(cached, _)| cached.source_files);
    let mut had_invalid_mdoc = false;
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
                    paths.push(output_path(&mdcroot, &source.relative, srctype));
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
            if incremental_changed.is_some() {
                expected_source_contents.insert(source_id.clone(), snapshot.content().to_vec());
            }
            if incremental_changed.is_some() {
                valid_mdocs = valid_mdocs.saturating_sub(1);
                exported_source_files = exported_source_files.saturating_sub(
                    new_manifest
                        .sources
                        .get(&source_id)
                        .map_or(0, |baseline| baseline.blocks.len()),
                );
            }
            current_source_ids.insert(source_id.clone());
            let node = match node {
                Ok(node) => node,
                Err(error) => {
                    had_invalid_mdoc = true;
                    inputs.push((source_path, Some(snapshot)));
                    warnings.push(issue(&relative, None, error.to_string()));
                    continue;
                }
            };
            valid_mdocs += 1;

            let source_baseline = new_manifest.sources.entry(source_id).or_default();
            let mut mirror_changes = MirrorChanges {
                root: &mdcroot,
                source: &relative,
                snapshots: &mut write_snapshots,
                writes: &mut writes,
                removals: &mut removals,
            };
            for srctype in builtin_srctypes() {
                let (mdoc_content, mdoc_present) = block_state(&node, srctype);
                exported_source_files += usize::from(mdoc_present);
                let raw_path = mirror_paths
                    .next()
                    .expect("every valid mdoc has five prepared mirror paths");
                let raw_input_path = raw_path.clone();
                let raw_snapshot = mirror_snapshots
                    .next()
                    .expect("every valid mdoc has five mirror snapshots");
                let raw_content = raw_snapshot.as_ref().map(ReadFileSnapshot::content);
                if mdoc_present || raw_content.is_some_and(|content| !content.is_empty()) {
                    if let Some(identity) = raw_snapshot.as_ref().map(ReadFileSnapshot::identity) {
                        desired_outputs.insert(identity.clone(), raw_path.clone());
                    }
                }

                if !source_baseline.is_unknown(srctype) {
                    if needs_sparse_migration
                        && !source_baseline.is_present(srctype)
                        && !mdoc_present
                        && raw_content == Some(&[])
                    {
                        mirror_changes.queue_sparse_migration_removal(
                            srctype,
                            raw_path,
                            raw_content,
                        )?;
                        inputs.push((raw_input_path, raw_snapshot));
                        continue;
                    }
                    match reconcile(
                        source_baseline,
                        srctype,
                        mdoc_content,
                        mdoc_present,
                        raw_content,
                    ) {
                        Reconciliation::Unchanged => {}
                        Reconciliation::MdocChanged => {
                            mirror_changes.queue(
                                srctype,
                                raw_path,
                                raw_content,
                                mdoc_content,
                                mdoc_present,
                            )?;
                            source_baseline.update(srctype, mdoc_content, mdoc_present);
                        }
                        Reconciliation::MirrorChanged(_) => dirty.push(issue(
                            &relative,
                            Some(srctype),
                            "source mirror has uncommitted changes; run `mdc back`",
                        )),
                        Reconciliation::Converged(raw_state) => {
                            mirror_changes.queue(
                                srctype,
                                raw_path,
                                raw_content,
                                raw_state.content.as_ref(),
                                raw_state.present,
                            )?;
                            source_baseline.update(srctype, mdoc_content, mdoc_present);
                        }
                        Reconciliation::Conflict => conflicts.push(issue(
                            &relative,
                            Some(srctype),
                            "mdoc block and source mirror both changed",
                        )),
                    }
                } else {
                    if needs_sparse_migration && !mdoc_present && raw_content == Some(&[]) {
                        mirror_changes.queue_sparse_migration_removal(
                            srctype,
                            raw_path,
                            raw_content,
                        )?;
                        source_baseline.update(srctype, mdoc_content, mdoc_present);
                    } else if raw_content.is_none() {
                        mirror_changes.queue(
                            srctype,
                            raw_path,
                            raw_content,
                            mdoc_content,
                            mdoc_present,
                        )?;
                        source_baseline.update(srctype, mdoc_content, mdoc_present);
                    } else if mdoc_present && raw_content == Some(mdoc_content) {
                        source_baseline.update(srctype, mdoc_content, mdoc_present);
                    } else if raw_content != Some(mdoc_content) || !mdoc_present {
                        conflicts.push(issue(
                            &relative,
                            Some(srctype),
                            "source mirror has no baseline and differs from the mdoc block",
                        ));
                    }
                }
                inputs.push((raw_input_path, raw_snapshot));
            }
            inputs.push((source_path, Some(snapshot)));
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
            if source_baseline.is_unknown(srctype) {
                continue;
            }
            let Some((path, type_root)) = existing_output_path(&mdcroot, &source, srctype)? else {
                source_baseline.forget(srctype);
                continue;
            };
            let snapshot = FileSnapshot::capture_beneath(&mdcroot, &path)?;
            if let Some(desired_path) = snapshot
                .identity()
                .and_then(|identity| desired_outputs.get(identity))
            {
                if path != *desired_path && source_baseline.matches_raw(srctype, snapshot.content())
                {
                    renames.push(PreparedRename {
                        from: path,
                        to: desired_path.clone(),
                        snapshot,
                    });
                }
                source_baseline.forget(srctype);
                continue;
            }
            let clean_sparse_placeholder = needs_sparse_migration
                && !source_baseline.is_present(srctype)
                && snapshot.content() == Some(&[]);
            if source_baseline.matches_raw(srctype, snapshot.content())
                || clean_sparse_placeholder
                || matches!(snapshot, FileSnapshot::Missing)
            {
                if !matches!(snapshot, FileSnapshot::Missing) {
                    removals.push(PreparedRemoval {
                        path,
                        type_root,
                        snapshot,
                        recoverable: clean_sparse_placeholder,
                    });
                }
                source_baseline.forget(srctype);
            } else {
                conflicts.push(issue(
                    &source,
                    Some(srctype),
                    "source mdoc was removed but its mirror has uncommitted changes",
                ));
            }
        }
        if source_baseline.has_established_types() {
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
            let snapshot = FileSnapshot::capture_beneath(&mdcroot, &path)?;
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
    let renamed = renames.len();
    {
        let _phase = crate::profile::scope("workdraft::apply_changes");
        apply_changes(
            &mdcroot,
            || mutation_lock.root().map(|_| ()),
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
    if dirty.is_empty() && conflicts.is_empty() && warnings.is_empty() {
        if let Some(changed_sources) = &incremental_changed {
            update_observation_cache(
                cache.as_deref_mut(),
                &mdcroot,
                &new_manifest,
                changed_sources,
                &expected_source_contents,
                valid_mdocs,
                exported_source_files,
            )?;
        } else if source_write_count == 0 && removed == 0 && renamed == 0 {
            store_observation_cache(
                cache,
                &mdcroot,
                &new_manifest,
                &observation_sources,
                &inputs,
                valid_mdocs,
                exported_source_files,
            )?;
        }
    }
    Ok(SyncReport {
        valid_mdocs,
        source_files: exported_source_files,
        updated: source_write_count,
        removed,
        dirty,
        conflicts,
        warnings,
    })
}

fn ensure_observed_content(
    path: &Path,
    snapshot: &FileSnapshot,
    observed_content: Option<&[u8]>,
) -> Result<()> {
    if snapshot.content() != observed_content {
        bail!(
            "{} changed during source block reconciliation",
            path.display()
        );
    }
    Ok(())
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
                    let snapshot = snapshots.capture_read(source_path)?.ok_or_else(|| {
                        anyhow::anyhow!("mdoc file disappeared: {}", source_path.display())
                    })?;
                    let node = MdocNode::load_bytes(source_path, snapshot.content());
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

pub(super) fn stat_files_parallel(
    root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<Option<FileStatSnapshot>>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallel_worker_count(paths.len());
    let chunk_size = paths.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in paths.chunks(chunk_size) {
            workers.push(
                scope.spawn(move || -> Result<Vec<Option<FileStatSnapshot>>> {
                    let mut snapshots = FileSnapshotBatch::new(root)?;
                    let mut result = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        result.push(snapshots.capture_stat(path)?);
                    }
                    snapshots.finish()?;
                    Ok(result)
                }),
            );
        }
        join_snapshot_workers(workers, paths.len(), "file stat worker panicked")
    })
}

fn parallel_worker_count(item_count: usize) -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(MAX_PARALLEL_WORKERS)
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

#[cfg(test)]
pub(crate) fn back(mutation_lock: &crate::workspace::WorkspaceMutationLock) -> Result<BackReport> {
    back_with_cache(mutation_lock, None)
}

pub(crate) fn back_cached(
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    cache: &mut crate::indcache::IndCache,
) -> Result<BackReport> {
    back_with_cache(mutation_lock, Some(cache))
}

fn back_with_cache(
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    mut cache: Option<&mut crate::indcache::IndCache>,
) -> Result<BackReport> {
    let _profile = crate::profile::scope("workdraft::back");
    let mdcroot = mutation_lock.root()?.to_path_buf();
    let manifest_path = mdcroot.join(".mdc").join(MANIFEST_NAME);
    let (manifest_snapshot, loaded) = {
        let _phase = crate::profile::scope("workdraft::load_manifest");
        let snapshot = FileSnapshot::capture_beneath(&mdcroot, &manifest_path)?;
        let loaded = parse_manifest(&snapshot, &manifest_path)?;
        (snapshot, loaded)
    };
    if loaded.needs_sparse_migration {
        bail!("source block manifest must be upgraded with `mdc sync` before `mdc back`");
    }
    let mut manifest = loaded.manifest;
    if let Some(cache) = cache.as_deref_mut() {
        if cache.workdraft_index_is_dirty()? {
            cache.recover_workdraft_index()?;
        }
    }
    if manifest.sources.is_empty() {
        return Ok(BackReport::default());
    }
    let source_files = {
        let _phase = crate::profile::scope("workdraft::enumerate_back_mdocs");
        let mut files = iter_mdoc_files(&mdcroot).collect::<Result<Vec<_>>>()?;
        files.sort();
        files
    };
    let observation_changes = check_observation_cache(
        cache.as_deref(),
        &mdcroot,
        &manifest_snapshot,
        &manifest,
        &source_files,
    )?;
    if observation_changes
        .as_ref()
        .is_some_and(|(_, changed)| changed.is_empty())
    {
        require_fast_path_current(mutation_lock, &mdcroot, &manifest_path, &manifest_snapshot)?;
        return Ok(BackReport::default());
    }
    let changed_sources = observation_changes.map(|(_, changed)| changed);

    let mut node_writes = Vec::new();
    let mut raw_writes = Vec::new();
    let mut inputs = Vec::new();
    let mut updated_blocks = 0;
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let mut expected_source_contents = HashMap::new();

    let requests = {
        let _phase = crate::profile::scope("workdraft::prepare_back_paths");
        manifest
            .sources
            .iter()
            .filter(|(source_id, _)| {
                changed_sources
                    .as_ref()
                    .is_none_or(|changed| changed.contains(*source_id))
            })
            .map(|(source_id, source_baseline)| {
                let relative = decode_source_path(source_id)?;
                let source_path = crate::workspace::resolve_mdoc_path(&mdcroot, &relative)?;
                let srctypes = builtin_srctypes()
                    .filter(|srctype| !source_baseline.is_unknown(srctype))
                    .collect();
                Ok(BackScanRequest {
                    relative,
                    source_path,
                    srctypes,
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let scanned_sources = {
        let _phase = crate::profile::scope("workdraft::read_back_inputs_parallel");
        scan_back_sources_parallel(&mdcroot, requests)?
    };

    let classify_profile = crate::profile::scope("workdraft::classify_back_reconciliation");
    for scanned in scanned_sources {
        let source_id = encode_source_path(&scanned.relative);
        let source_baseline = manifest
            .sources
            .get_mut(&source_id)
            .expect("scanned back source came from the manifest");
        let ScannedBackSource {
            relative,
            source_path,
            source_snapshot,
            mirrors,
        } = scanned;
        if let Some(snapshot) = &source_snapshot {
            expected_source_contents.insert(source_id.clone(), snapshot.content().to_vec());
        }
        if source_snapshot.is_none() {
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
                .as_ref()
                .map(ReadFileSnapshot::content)
                .expect("missing source snapshot handled above"),
        ) {
            Ok(node) => node,
            Err(error) => {
                let mut raw_dirty = false;
                for (srctype, raw_path, raw_snapshot) in mirrors {
                    if !source_baseline.matches_raw(
                        srctype,
                        raw_snapshot.as_ref().map(ReadFileSnapshot::content),
                    ) {
                        raw_dirty = true;
                        inputs.push((raw_path, raw_snapshot));
                        break;
                    }
                    inputs.push((raw_path, raw_snapshot));
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
        for (srctype, raw_path, raw_snapshot) in mirrors {
            let (mdoc_content, mdoc_present) = block_state(&node, srctype);
            let raw_content = raw_snapshot.as_ref().map(ReadFileSnapshot::content);
            let raw_state = match reconcile(
                source_baseline,
                srctype,
                mdoc_content,
                mdoc_present,
                raw_content,
            ) {
                Reconciliation::Unchanged => {
                    inputs.push((raw_path, raw_snapshot));
                    continue;
                }
                Reconciliation::MdocChanged => {
                    conflicts.push(issue(
                        &relative,
                        Some(srctype),
                        "mdoc block has uncommitted changes; run `mdc sync`",
                    ));
                    inputs.push((raw_path, raw_snapshot));
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
                    inputs.push((raw_path, raw_snapshot));
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
            if !source_baseline.matches_state(srctype, content, present) {
                source_baseline.update(srctype, content, present);
            }
            if present && raw_content != Some(content) {
                let normalized_content = raw_state.content.into_owned();
                raw_writes.push(PreparedWrite {
                    snapshot: hydrate_observed_snapshot(
                        &mdcroot,
                        &raw_path,
                        raw_snapshot.as_ref(),
                    )?,
                    path: raw_path,
                    content: normalized_content,
                });
            } else {
                inputs.push((raw_path, raw_snapshot));
            }
        }
        if node_changed {
            let content = node.render()?.into_bytes();
            expected_source_contents.insert(source_id, content.clone());
            node_writes.push(PreparedWrite {
                snapshot: hydrate_observed_snapshot(
                    &mdcroot,
                    &source_path,
                    source_snapshot.as_ref(),
                )?,
                path: source_path,
                content,
            });
        } else {
            inputs.push((source_path, source_snapshot));
        }
    }
    drop(classify_profile);

    let updated_mdocs = node_writes.len();
    let updated_paths: Vec<_> = node_writes.iter().map(|write| write.path.clone()).collect();
    node_writes.append(&mut raw_writes);
    let write_count = node_writes.len();
    if updated_mdocs != 0 {
        if let Some(cache) = cache.as_deref_mut() {
            cache.set_workdraft_index_dirty(true)?;
        }
    }
    {
        let _phase = crate::profile::scope("workdraft::apply_back_changes");
        apply_changes(
            &mdcroot,
            || mutation_lock.root().map(|_| ()),
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
    }
    let mut index_refreshed = false;
    if updated_mdocs != 0 {
        if let Some(cache) = cache.as_deref_mut() {
            if updated_paths.len() == 1 {
                cache.discover_workspace_changes()?;
                cache.upsert_path(&updated_paths[0])?;
            } else {
                cache.refresh_all()?;
            }
            index_refreshed = true;
        }
    }
    if conflicts.is_empty() && warnings.is_empty() {
        let source_file_count = manifest
            .sources
            .values()
            .map(|source| source.blocks.len())
            .sum();
        if let Some(changed_sources) = &changed_sources {
            update_observation_cache(
                cache.as_deref_mut(),
                &mdcroot,
                &manifest,
                changed_sources,
                &expected_source_contents,
                manifest.sources.len(),
                source_file_count,
            )?;
        } else if write_count == 0 {
            store_observation_cache(
                cache.as_deref_mut(),
                &mdcroot,
                &manifest,
                &source_files,
                &inputs,
                manifest.sources.len(),
                source_file_count,
            )?;
        }
    }
    if index_refreshed {
        if let Some(cache) = cache {
            cache.set_workdraft_index_dirty(false)?;
        }
    }
    Ok(BackReport {
        updated_blocks,
        updated_mdocs,
        index_refreshed,
        conflicts,
        warnings,
    })
}

fn check_observation_cache(
    cache: Option<&crate::indcache::IndCache>,
    root: &Path,
    manifest_snapshot: &FileSnapshot,
    manifest: &SourceBlockManifest,
    source_files: &[PathBuf],
) -> Result<Option<(crate::indcache::WorkdraftObservationCache, BTreeSet<String>)>> {
    let Some(cache) = cache else {
        return Ok(None);
    };
    let _profile = crate::profile::scope("workdraft::check_observation_cache");
    let digest: [u8; 32] = Sha256::digest(manifest_snapshot.content().unwrap_or(&[])).into();
    let Some(cached) = cache.load_workdraft_observations(&digest)? else {
        return Ok(None);
    };
    let Some(specs) = observation_specs(root, manifest, source_files)? else {
        return Ok(None);
    };
    if cached.files.len() != specs.len() {
        return Ok(None);
    }
    if cached.valid_mdocs != manifest.sources.len()
        || cached.source_files
            != manifest
                .sources
                .values()
                .map(|source| source.blocks.len())
                .sum::<usize>()
    {
        return Ok(None);
    }
    let paths: Vec<_> = specs.iter().map(|spec| spec.path.clone()).collect();
    let stats = stat_files_parallel_batched(root, &paths, SYNC_SOURCE_BATCH * 6)?;
    let mut changed = BTreeSet::new();
    for (spec, stat) in specs.iter().zip(stats) {
        if cached
            .files
            .get(&(spec.source_id.clone(), spec.srctype.clone()))
            != Some(&stat)
        {
            changed.insert(spec.source_id.clone());
        }
    }
    Ok(Some((cached, changed)))
}

fn require_fast_path_current(
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    root: &Path,
    manifest_path: &Path,
    manifest_snapshot: &FileSnapshot,
) -> Result<()> {
    if !manifest_snapshot.unchanged_beneath(root, manifest_path)? {
        bail!(
            "{} changed during source block reconciliation",
            manifest_path.display()
        );
    }
    mutation_lock.root()?;
    Ok(())
}

fn store_observation_cache(
    cache: Option<&mut crate::indcache::IndCache>,
    root: &Path,
    manifest: &SourceBlockManifest,
    source_files: &[PathBuf],
    inputs: &[(PathBuf, Option<ReadFileSnapshot>)],
    valid_mdocs: usize,
    source_file_count: usize,
) -> Result<()> {
    let Some(cache) = cache else {
        return Ok(());
    };
    let _profile = crate::profile::scope("workdraft::store_observation_cache");
    let Some(specs) = observation_specs(root, manifest, source_files)? else {
        return Ok(());
    };
    let stats_by_path: HashMap<&Path, Option<FileStatSnapshot>> = inputs
        .iter()
        .map(|(path, snapshot)| {
            (
                path.as_path(),
                snapshot.as_ref().map(ReadFileSnapshot::stat_snapshot),
            )
        })
        .collect();
    let observations = specs
        .into_iter()
        .map(|spec| {
            let stat = stats_by_path
                .get(spec.path.as_path())
                .copied()
                .ok_or_else(|| {
                    anyhow::anyhow!("missing validated observation for {}", spec.path.display())
                })?;
            Ok(crate::indcache::WorkdraftObservation {
                source_id: spec.source_id,
                srctype: spec.srctype,
                stat,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let digest = serialized_manifest_digest(manifest)?;
    cache.store_workdraft_observations(&digest, valid_mdocs, source_file_count, observations)
}

fn update_observation_cache(
    cache: Option<&mut crate::indcache::IndCache>,
    root: &Path,
    manifest: &SourceBlockManifest,
    changed_sources: &BTreeSet<String>,
    expected_source_contents: &HashMap<String, Vec<u8>>,
    valid_mdocs: usize,
    source_file_count: usize,
) -> Result<()> {
    let Some(cache) = cache else {
        return Ok(());
    };
    let _profile = crate::profile::scope("workdraft::update_observation_cache");
    let mut specs = Vec::with_capacity(changed_sources.len() * 6);
    for source_id in changed_sources {
        let relative = decode_source_path(source_id)?;
        let baseline = manifest
            .sources
            .get(source_id)
            .expect("changed observation source is in the manifest");
        if builtin_srctypes().any(|srctype| baseline.is_unknown(srctype)) {
            return Ok(());
        }
        specs.push(ObservationSpec {
            source_id: source_id.clone(),
            srctype: String::new(),
            path: root.join(&relative),
        });
        for srctype in builtin_srctypes() {
            specs.push(ObservationSpec {
                source_id: source_id.clone(),
                srctype: srctype.to_string(),
                path: output_path(root, &relative, srctype),
            });
        }
    }
    let paths: Vec<_> = specs.iter().map(|spec| spec.path.clone()).collect();
    let snapshots = read_files_parallel(root, &paths)?;
    for (spec, snapshot) in specs.iter().zip(&snapshots) {
        if spec.srctype.is_empty() {
            let expected = expected_source_contents
                .get(&spec.source_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("missing expected mdoc content for {}", spec.path.display())
                })?;
            if snapshot.as_ref().map(ReadFileSnapshot::content) != Some(expected.as_slice()) {
                bail!(
                    "{} changed while observations were updated",
                    spec.path.display()
                );
            }
        } else {
            let baseline = manifest
                .sources
                .get(&spec.source_id)
                .expect("observation source is in the manifest");
            if !baseline.matches_raw(
                &spec.srctype,
                snapshot.as_ref().map(ReadFileSnapshot::content),
            ) {
                bail!(
                    "{} changed while observations were updated",
                    spec.path.display()
                );
            }
        }
    }
    let observations = specs
        .into_iter()
        .zip(snapshots)
        .map(|(spec, snapshot)| crate::indcache::WorkdraftObservation {
            source_id: spec.source_id,
            srctype: spec.srctype,
            stat: snapshot.as_ref().map(ReadFileSnapshot::stat_snapshot),
        })
        .collect();
    let digest = serialized_manifest_digest(manifest)?;
    cache.update_workdraft_observations(&digest, valid_mdocs, source_file_count, observations)
}

fn serialized_manifest_digest(manifest: &SourceBlockManifest) -> Result<[u8; 32]> {
    let mut content = serde_json::to_vec_pretty(manifest)?;
    content.push(b'\n');
    Ok(Sha256::digest(content).into())
}

fn observation_specs(
    root: &Path,
    manifest: &SourceBlockManifest,
    source_files: &[PathBuf],
) -> Result<Option<Vec<ObservationSpec>>> {
    if source_files.len() != manifest.sources.len() {
        return Ok(None);
    }
    let mut specs = Vec::with_capacity(source_files.len() * 6);
    for source_path in source_files {
        let relative = source_path
            .strip_prefix(root)
            .with_context(|| format!("relativizing {}", source_path.display()))?;
        validate_source_relative(relative)?;
        let source_id = encode_source_path(relative);
        let Some(baseline) = manifest.sources.get(&source_id) else {
            return Ok(None);
        };
        if builtin_srctypes().any(|srctype| baseline.is_unknown(srctype)) {
            return Ok(None);
        }
        specs.push(ObservationSpec {
            source_id: source_id.clone(),
            srctype: String::new(),
            path: source_path.clone(),
        });
        for srctype in builtin_srctypes() {
            specs.push(ObservationSpec {
                source_id: source_id.clone(),
                srctype: srctype.to_string(),
                path: output_path(root, relative, srctype),
            });
        }
    }
    Ok(Some(specs))
}

fn stat_files_parallel_batched(
    root: &Path,
    paths: &[PathBuf],
    batch_size: usize,
) -> Result<Vec<Option<FileStatSnapshot>>> {
    let mut stats = Vec::with_capacity(paths.len());
    for batch in paths.chunks(batch_size) {
        stats.extend(stat_files_parallel(root, batch)?);
    }
    Ok(stats)
}

fn scan_back_sources_parallel(
    root: &Path,
    requests: Vec<BackScanRequest>,
) -> Result<Vec<ScannedBackSource>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let source_paths: Vec<_> = requests
        .iter()
        .map(|request| request.source_path.clone())
        .collect();
    let source_snapshots = {
        let _phase = crate::profile::scope("workdraft::read_back_mdocs_parallel");
        read_files_parallel_batched(root, &source_paths, SYNC_SOURCE_BATCH)?
    };
    let mirror_paths: Vec<_> = requests
        .iter()
        .zip(&source_snapshots)
        .flat_map(|(request, snapshot)| {
            snapshot.iter().flat_map(|_| {
                request
                    .srctypes
                    .iter()
                    .map(|srctype| output_path(root, &request.relative, srctype))
            })
        })
        .collect();
    let mirror_snapshots = {
        let _phase = crate::profile::scope("workdraft::read_back_mirrors_parallel");
        read_files_parallel_batched(root, &mirror_paths, SYNC_SOURCE_BATCH * 5)?
    };
    let mut mirror_paths = mirror_paths.into_iter();
    let mut mirror_snapshots = mirror_snapshots.into_iter();
    let result = requests
        .into_iter()
        .zip(source_snapshots)
        .map(|(request, source_snapshot)| {
            let mirrors = if source_snapshot.is_some() {
                request
                    .srctypes
                    .iter()
                    .map(|srctype| {
                        (
                            *srctype,
                            mirror_paths.next().expect("prepared mirror path"),
                            mirror_snapshots.next().expect("captured mirror snapshot"),
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            ScannedBackSource {
                relative: request.relative,
                source_path: request.source_path,
                source_snapshot,
                mirrors,
            }
        })
        .collect();
    debug_assert!(mirror_paths.next().is_none());
    debug_assert!(mirror_snapshots.next().is_none());
    Ok(result)
}

fn read_files_parallel_batched(
    root: &Path,
    paths: &[PathBuf],
    batch_size: usize,
) -> Result<Vec<Option<ReadFileSnapshot>>> {
    let mut snapshots = Vec::with_capacity(paths.len());
    for batch in paths.chunks(batch_size) {
        snapshots.extend(read_files_parallel(root, batch)?);
    }
    Ok(snapshots)
}

fn hydrate_observed_snapshot(
    root: &Path,
    path: &Path,
    observed: Option<&ReadFileSnapshot>,
) -> Result<FileSnapshot> {
    let snapshot = FileSnapshot::capture_beneath(root, path)?;
    if !snapshot.matches_read(observed) {
        bail!(
            "{} changed during source block reconciliation",
            path.display()
        );
    }
    Ok(snapshot)
}

fn capture_existing_mirror(
    root: &Path,
    source: &Path,
    srctype: &str,
) -> Result<(PathBuf, FileSnapshot)> {
    match existing_output_path(root, source, srctype)? {
        Some((path, _)) => {
            let snapshot = FileSnapshot::capture_beneath(root, &path)?;
            Ok((path, snapshot))
        }
        None => Ok((output_path(root, source, srctype), FileSnapshot::Missing)),
    }
}

pub(crate) fn targets(
    mdcroot: &Path,
    source_path: &Path,
    node: &MdocNode,
) -> Result<Vec<(String, PathBuf)>> {
    let source = source_path
        .strip_prefix(mdcroot)
        .with_context(|| format!("relativizing {}", source_path.display()))?;
    validate_source_relative(source)?;
    let mut targets = Vec::new();
    for srctype in builtin_srctypes() {
        let (path, snapshot) = capture_existing_mirror(mdcroot, source, srctype)?;
        if node.source_block(srctype).is_some() && matches!(snapshot, FileSnapshot::Missing) {
            bail!("source mirror is missing: {}", path.display());
        }
        if !matches!(snapshot, FileSnapshot::Missing) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_rejects_a_mirror_changed_after_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.upsert_source_block("lean", "original\n".to_string())
            .unwrap();
        std::fs::write(&path, node.render().unwrap()).unwrap();
        let lock = crate::workspace::WorkspaceMutationLock::acquire(&root).unwrap();
        sync(&lock).unwrap();
        let mirror = root.join(".mdc/lean/Lib/node.lean");
        std::fs::write(&mirror, "first edit\n").unwrap();
        let hook_mirror = mirror.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteBeforeDirectoryBinding,
            move || std::fs::write(hook_mirror, "second edit\n").unwrap(),
        );

        let error = match back(&lock) {
            Ok(_) => panic!("expected concurrent mirror edit to be rejected"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("changed while source block writes"));
        assert_eq!(
            MdocNode::load(&path)
                .unwrap()
                .source_block("lean")
                .unwrap()
                .content,
            "original\n"
        );
        assert_eq!(std::fs::read_to_string(mirror).unwrap(), "second edit\n");
    }
}
