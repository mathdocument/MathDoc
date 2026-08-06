use anyhow::Result;
use std::io::Write;

use crate::compiler::{CompilerRegistry, CompilerReq, CompilerRes};
use crate::config::Config;
use crate::core::escape_terminal;
use crate::indcache::IndCache;

use super::{cwd, print_workdraft_issues, require_mdcroot, BLD, DIM, GRN, RED, RST};

pub(super) fn cmd_work(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
    let (targets, sync_conflicted, source_fnode, formal_languages, manifest_snapshot) = {
        let mutation_lock = work_lock.acquire_mutation_lock()?;
        let mut cache = IndCache::open_refreshed_under_mutation_lock(&mutation_lock)?;
        let (source_fnode, _, source_path) = cache.resolve_ref(&source, Some(&cwd()))?;
        let sync = crate::workdraft::sync(&mutation_lock)?;
        print_workdraft_issues(&sync.warnings);
        print_workdraft_issues(&sync.dirty);
        print_workdraft_issues(&sync.conflicts);
        let sync_conflicted = !sync.conflicts.is_empty();
        let targets = if sync_conflicted {
            Vec::new()
        } else {
            cache.invalidate_formal_attestations(
                &mutation_lock,
                &source_fnode,
                &crate::formal::status::FORMAL_LANGUAGES.map(str::to_string),
            )?;
            crate::workdraft::targets(&mdcroot, &source_path)?
        };
        let node = crate::mdocnode::MdocNode::load(&source_path)?;
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
            targets,
            sync_conflicted,
            source_fnode,
            formal_languages,
            manifest_snapshot,
        )
    };

    if sync_conflicted {
        return Ok(1);
    }
    if targets.is_empty() {
        println!("No source blocks found for this mdoc");
        return Ok(0);
    }

    let config = Config::load(&mdcroot)?;
    let registry = CompilerRegistry::default_registry();
    let total = targets.len();
    let mut failure_codes = Vec::new();
    let mut interrupted_code = None;
    let mut formal_outcomes = formal_languages
        .iter()
        .map(|language| (language.clone(), false, None))
        .collect::<Vec<_>>();
    for (index, (srctype, path)) in targets.iter().enumerate() {
        work_lock.require_current()?;
        println!(
            "[{}/{}] {BLD}{srctype}{RST}  {}",
            index + 1,
            total,
            escape_terminal(&path.to_string_lossy())
        );
        let _ = std::io::stdout().flush();

        fn compile_progress(message: &str) {
            println!("  {DIM}{}{RST}", escape_terminal(message));
        }

        let req = CompilerReq {
            mdcroot: mdcroot.clone(),
            source: path.clone(),
            config: config.src_config(srctype),
            progress: Some(Box::new(compile_progress)),
        };
        let (result, formal_receipt) = match registry.resolve(srctype) {
            Some(compiler) => compiler.compile_with_receipt(&req),
            None => (
                CompilerRes::err(format!("unknown srctype: {srctype}")),
                None,
            ),
        };
        print_compile_result(&result);
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
        if result.interrupted {
            interrupted_code = Some(result.rtcode);
            break;
        }
    }
    work_lock.require_current()?;

    let mutation_lock = work_lock.acquire_mutation_lock()?;
    let mut cache = IndCache::open_refreshed_under_mutation_lock(&mutation_lock)?;
    let (_, _, source_path) = cache.resolve_ref(&source_fnode, Some(&mdcroot))?;
    cache.upsert_path(&source_path)?;
    let attestation_errors = cache.publish_formal_attestations(
        &mutation_lock,
        &manifest_snapshot,
        &source_fnode,
        &formal_outcomes,
    )?;
    for (language, error) in &attestation_errors {
        eprintln!(
            "  {RED}{}{RST}",
            escape_terminal(&format!("{language} attestation failed: {error}"))
        );
    }
    if !attestation_errors.is_empty() {
        failure_codes.push(1);
    }
    work_lock.require_current()?;
    Ok(interrupted_code.unwrap_or_else(|| aggregate_compile_exit(&failure_codes)))
}

pub(super) fn cmd_back() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let report = {
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
        let mutation_lock = work_lock.acquire_mutation_lock()?;
        let mut cache = IndCache::open_under_mutation_lock(&mutation_lock)?;
        cache.discover_workspace_changes()?;
        let report = crate::workdraft::back(&mutation_lock)?;
        if report.updated_mdocs != 0 {
            cache.refresh_all()?;
        } else {
            cache.refresh_formal_statuses()?;
        }
        work_lock.require_current()?;
        report
    };

    print_workdraft_issues(&report.warnings);
    print_workdraft_issues(&report.conflicts);
    println!(
        "synced {BLD}{}{RST} source block{} into {} mdoc{}",
        report.updated_blocks,
        if report.updated_blocks == 1 { "" } else { "s" },
        report.updated_mdocs,
        if report.updated_mdocs == 1 { "" } else { "s" },
    );
    Ok(if report.conflicts.is_empty() { 0 } else { 1 })
}

fn print_compile_result(result: &CompilerRes) {
    if !result.stdout.is_empty() {
        for line in result.stdout.lines() {
            println!("  {}", escape_terminal(line));
        }
    }
    if !result.stderr.is_empty() {
        for line in result.stderr.lines() {
            eprintln!("  {RED}{}{RST}", escape_terminal(line));
        }
    }
    if result.is_success() {
        println!("{GRN}✓{RST} (exit {})", result.rtcode);
    } else {
        println!("{RED}✗{RST} (exit {})", result.rtcode);
    }
    println!();
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
