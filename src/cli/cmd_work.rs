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
    let (targets, sync_conflicted, source_fnode) = {
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&mdcroot)?;
        let mut cache = IndCache::open_under_mutation_lock(&mutation_lock)?;
        cache.discover_workspace_changes()?;
        let (source_fnode, _, source_path) = cache.resolve_ref(&source, Some(&cwd()))?;
        let sync = crate::workdraft::sync(&mutation_lock)?;
        print_workdraft_issues(&sync.warnings);
        print_workdraft_issues(&sync.dirty);
        print_workdraft_issues(&sync.conflicts);
        let sync_conflicted = !sync.conflicts.is_empty();
        if sync_conflicted {
            (Vec::new(), true, source_fnode)
        } else {
            (
                crate::workdraft::targets(&mdcroot, &source_path)?,
                false,
                source_fnode,
            )
        }
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
    for (index, (srctype, path)) in targets.iter().enumerate() {
        work_lock.require_current()?;
        println!(
            "[{}/{}] {BLD}{srctype}{RST}  {}",
            index + 1,
            total,
            path.display()
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
        let result = match registry.resolve(srctype) {
            Some(compiler) => compiler.compile(&req),
            None => CompilerRes::err(format!("unknown srctype: {srctype}")),
        };
        print_compile_result(&result);
        if !result.is_success() {
            failure_codes.push(result.rtcode);
        }
        if result.interrupted {
            interrupted_code = Some(result.rtcode);
            break;
        }
    }
    work_lock.require_current()?;

    let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&mdcroot)?;
    let mut cache = IndCache::open_under_mutation_lock(&mutation_lock)?;
    cache.discover_workspace_changes()?;
    let (_, _, source_path) = cache.resolve_ref(&source_fnode, Some(&mdcroot))?;
    cache.upsert_path(&source_path)?;
    work_lock.require_current()?;
    Ok(interrupted_code.unwrap_or_else(|| aggregate_compile_exit(&failure_codes)))
}

pub(super) fn cmd_back() -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let report = {
        let _work_lock = crate::workspace::WorkspaceWorkLock::acquire(&mdcroot)?;
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&mdcroot)?;
        let mut cache = IndCache::open_under_mutation_lock(&mutation_lock)?;
        cache.discover_workspace_changes()?;
        let report = crate::workdraft::back(&mutation_lock)?;
        if report.updated_mdocs != 0 {
            cache.refresh_all()?;
        }
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
