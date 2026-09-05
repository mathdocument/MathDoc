use anyhow::Result;
use std::io::Write;

use crate::application::work::{self, WorkEvent};
use crate::compiler::CompilerRes;
use crate::core::escape_terminal;

use super::{cwd, print_workdraft_issues, require_mdcroot, BLD, DIM, GRN, RED, RST};

pub(super) fn cmd_work(source: String) -> Result<i32> {
    let report = work::compile_node(require_mdcroot()?, &source, &cwd(), |event| match event {
        WorkEvent::Reconciled(sync) => {
            print_workdraft_issues(&sync.warnings);
            print_workdraft_issues(&sync.dirty);
            print_workdraft_issues(&sync.conflicts);
        }
        WorkEvent::Started {
            index,
            total,
            srctype,
            path,
        } => {
            println!(
                "[{index}/{total}] {BLD}{srctype}{RST}  {}",
                escape_terminal(&path.to_string_lossy())
            );
            let _ = std::io::stdout().flush();
        }
        WorkEvent::Progress(message) => println!("  {DIM}{}{RST}", escape_terminal(message)),
        WorkEvent::Finished(result) => print_compile_result(result),
    })?;
    if report.sync.conflicts.is_empty() && report.compilations.is_empty() {
        println!("No source blocks found for this mdoc");
    }
    for (language, error) in &report.attestation_errors {
        eprintln!(
            "  {RED}{}{RST}",
            escape_terminal(&format!("{language} attestation failed: {error}"))
        );
    }
    Ok(report.exit_code)
}

pub(super) fn cmd_back() -> Result<i32> {
    let report = work::import_mirrors(require_mdcroot()?)?;

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
