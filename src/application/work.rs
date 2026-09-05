use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::compiler::{CompilerReq, CompilerRes};
use crate::config::Config;
use crate::indcache::WorkspaceStore;
use crate::workdraft::{BackReport, SyncReport};

pub(crate) enum WorkEvent<'a> {
    Reconciled(&'a SyncReport),
    Started {
        index: usize,
        total: usize,
        srctype: &'a str,
        path: &'a Path,
    },
    Progress(&'a str),
    Finished(&'a CompilerRes),
}

pub(crate) struct WorkReport {
    pub sync: SyncReport,
    pub compilations: Vec<CompilerRes>,
    pub attestation_errors: Vec<(String, String)>,
    pub exit_code: i32,
}

pub(crate) fn compile_node(
    mdcroot: PathBuf,
    source: &str,
    cwd: &Path,
    notify: impl Fn(WorkEvent<'_>),
) -> Result<WorkReport> {
    let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
    let (mut cache, targets, sync, source_fnode, formal_languages, manifest_snapshot) = {
        let mutation_lock = work_lock.acquire_mutation_lock()?;
        let mut cache = WorkspaceStore::open_refreshed_under_mutation_lock(&mutation_lock)?;
        let (source_fnode, _, source_path) = cache.resolve_ref(source, Some(cwd))?;
        let sync = crate::workdraft::sync_cached(&work_lock, &mutation_lock, &mut cache)?;
        notify(WorkEvent::Reconciled(&sync));
        let sync_conflicted = !sync.conflicts.is_empty();
        let node = crate::mdocnode::MdocNode::load(&source_path)?;
        let targets = if sync_conflicted {
            Vec::new()
        } else {
            cache.invalidate_formal_attestations(
                &mutation_lock,
                &source_fnode,
                &crate::formal::status::FORMAL_LANGUAGES.map(str::to_string),
            )?;
            crate::workdraft::targets(&mdcroot, &source_path, &node)?
        };
        let formal_languages = targets
            .iter()
            .map(|(srctype, _)| srctype)
            .filter(|srctype| {
                crate::formal::status::FORMAL_LANGUAGES.contains(&srctype.as_str())
                    && node.source_block(srctype).is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        let manifest_snapshot = crate::formal::attestation::snapshot(&mdcroot)?;
        work_lock.require_current()?;
        (
            cache,
            targets,
            sync,
            source_fnode,
            formal_languages,
            manifest_snapshot,
        )
    };

    let mut report = WorkReport {
        exit_code: if sync.conflicts.is_empty() { 0 } else { 1 },
        sync,
        compilations: Vec::new(),
        attestation_errors: Vec::new(),
    };
    if targets.is_empty() {
        return Ok(report);
    }

    let config = Config::load(&mdcroot)?;
    let total = targets.len();
    let mut failure_codes = Vec::new();
    let mut interrupted_code = None;
    let mut formal_outcomes = formal_languages
        .iter()
        .map(|language| (language.clone(), false, None))
        .collect::<Vec<_>>();
    for (index, (srctype, path)) in targets.iter().enumerate() {
        work_lock.require_current()?;
        notify(WorkEvent::Started {
            index: index + 1,
            total,
            srctype,
            path,
        });

        let req = CompilerReq {
            mdcroot: mdcroot.clone(),
            source: path.clone(),
            config: config.src_config(srctype),
            progress: Some(Box::new(|message| notify(WorkEvent::Progress(message)))),
        };
        let (result, formal_receipt) =
            crate::compiler::compile_with_receipt(&work_lock, srctype, &req);

        if let Some((_, succeeded, receipt)) = formal_outcomes
            .iter_mut()
            .find(|(language, _, _)| language == srctype)
        {
            *succeeded = result.is_success();
            *receipt = formal_receipt;
        }
        if !result.is_success() {
            failure_codes.push(result.rtcode);
        }
        let interrupted = result.interrupted;
        if interrupted {
            interrupted_code = Some(result.rtcode);
        }
        notify(WorkEvent::Finished(&result));
        report.compilations.push(result);
        if interrupted {
            break;
        }
    }
    work_lock.require_current()?;

    let mutation_lock = work_lock.acquire_mutation_lock()?;
    cache.validate_mutation_lock(&mutation_lock)?;
    cache.discover_workspace_changes()?;
    let (_, _, source_path) = cache.resolve_ref(&source_fnode, Some(&mdcroot))?;
    cache.upsert_path(&source_path)?;
    let attestation_errors = cache.publish_formal_attestations(
        &work_lock,
        &mutation_lock,
        &manifest_snapshot,
        &source_fnode,
        &formal_outcomes,
    )?;
    if !attestation_errors.is_empty() {
        failure_codes.push(1);
    }
    work_lock.require_current()?;
    report.attestation_errors = attestation_errors;
    report.exit_code = interrupted_code.unwrap_or_else(|| aggregate_compile_exit(&failure_codes));
    Ok(report)
}

pub(crate) fn import_mirrors(mdcroot: PathBuf) -> Result<BackReport> {
    let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
    let mutation_lock = work_lock.acquire_mutation_lock()?;
    let mut cache = WorkspaceStore::open_under_mutation_lock(&mutation_lock)?;
    cache.discover_workspace_changes()?;
    let report = crate::workdraft::back_cached(&work_lock, &mutation_lock, &mut cache)?;
    cache.refresh_formal_statuses()?;
    work_lock.require_current()?;
    Ok(report)
}

pub(crate) fn export_mirrors(mdcroot: PathBuf) -> Result<(u32, SyncReport)> {
    let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
    let mutation_lock = work_lock.acquire_mutation_lock()?;
    let mut cache = WorkspaceStore::open_refreshed_under_mutation_lock(&mutation_lock)?;
    let total = cache.count()?;
    let report = crate::workdraft::sync_cached(&work_lock, &mutation_lock, &mut cache)?;
    cache.refresh_formal_statuses()?;
    work_lock.require_current()?;
    Ok((total, report))
}

fn aggregate_compile_exit(failure_codes: &[i32]) -> i32 {
    match failure_codes {
        [] => 0,
        [code] if (1..=255).contains(code) => *code,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::aggregate_compile_exit;

    #[test]
    fn compile_exit_aggregation_is_deterministic() {
        assert_eq!(aggregate_compile_exit(&[]), 0);
        assert_eq!(aggregate_compile_exit(&[124]), 124);
        assert_eq!(aggregate_compile_exit(&[127]), 127);
        assert_eq!(aggregate_compile_exit(&[124, 127]), 1);
        assert_eq!(aggregate_compile_exit(&[-1]), 1);
    }
}
