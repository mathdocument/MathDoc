use anyhow::{bail, Result};
use std::path::Path;

use super::{
    cfg_positive_int, emit_progress, is_timeout_error, require_tool, run_process, CompilerReq,
    CompilerRes, SrcCompiler,
};

pub(super) struct CompilerLean;

const MODULE_NAME: &str = "MdcWork";

impl SrcCompiler for CompilerLean {
    fn srctype(&self) -> &str {
        "lean"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec =
            match cfg_positive_int(&req.compcfg, "timeout_sec", "src.lean.timeout_sec") {
                Ok(v) => v,
                Err(e) => return CompilerRes::err(e.to_string()),
            };
        let setup_timeout_sec = match cfg_positive_int(
            &req.compcfg,
            "setup_timeout_sec",
            "src.lean.setup_timeout_sec",
        ) {
            Ok(v) => v,
            Err(e) => return CompilerRes::err(e.to_string()),
        };

        let lake = match require_tool("lake") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let ws_root = req.mdcroot.join(".mdc").join("lean");
        if let Err(e) = ensure_workspace(&ws_root, &lake, setup_timeout_sec, &req.progress) {
            return CompilerRes::err(e.to_string());
        }

        emit_progress(
            &req.progress,
            &format!("building with `lake build +{MODULE_NAME}`"),
        );
        match run_process(
            &[
                &lake,
                "--quiet",
                "--no-ansi",
                "build",
                &format!("+{MODULE_NAME}"),
            ],
            &format!("lake build +{MODULE_NAME}"),
            timeout_sec,
            Some(&ws_root),
        ) {
            Ok((rtcode, stdout, stderr)) => {
                let (out, err) = classify_build_output(&stdout, &stderr, rtcode == 0);
                CompilerRes {
                    result: rtcode == 0,
                    stdout: out,
                    stderr: err,
                    rtcode,
                }
            }
            Err(e) if is_timeout_error(&e) => CompilerRes::err_code(e.to_string(), 124),
            Err(e) => CompilerRes::err_code(e.to_string(), 1),
        }
    }
}

// ── Workspace setup ──────────────────────────────────────────────────────────

fn ensure_workspace(
    root: &Path,
    lake_path: &str,
    timeout_sec: u64,
    progress: &Option<Box<dyn Fn(&str)>>,
) -> Result<()> {
    if root.join("lakefile.toml").is_file() || root.join("lakefile.lean").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(root)?;

    emit_progress(
        progress,
        "initializing Lean workspace with `lake init mdc_work`",
    );
    let (rtcode, stdout, stderr) = run_process(
        &[lake_path, "init", "mdc_work"],
        "lake init",
        timeout_sec,
        Some(root),
    )?;
    if rtcode != 0 {
        bail!("lake init failed:\n{}", combine_output(&stdout, &stderr));
    }

    Ok(())
}

// ── Build output processing ──────────────────────────────────────────────────

fn classify_build_output(stdout: &str, stderr: &str, ok: bool) -> (String, String) {
    let lines = clean_output_lines(stdout, stderr);
    if lines.is_empty() {
        return (String::new(), String::new());
    }
    if !ok {
        return (String::new(), lines.join("\n"));
    }
    let mut out_lines = Vec::new();
    let mut err_lines = Vec::new();
    for line in &lines {
        if line.starts_with("warning:") || line.starts_with("error:") {
            err_lines.push(line.as_str());
        } else {
            out_lines.push(line.as_str());
        }
    }
    (
        out_lines.join("\n").trim().to_string(),
        err_lines.join("\n").trim().to_string(),
    )
}

fn combine_output(stdout: &str, stderr: &str) -> String {
    let lines = clean_output_lines(stdout, stderr);
    if lines.is_empty() {
        "no diagnostic output".to_string()
    } else {
        lines.join("\n").trim().to_string()
    }
}

fn clean_output_lines(stdout: &str, stderr: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in [stdout, stderr] {
        for line in raw.replace("\r\n", "\n").replace('\r', "\n").lines() {
            let text = line.trim();
            if text.is_empty() || is_noise_line(text) {
                continue;
            }
            lines.push(text.to_string());
        }
    }
    lines
}

fn is_noise_line(line: &str) -> bool {
    if line.starts_with("warning: failed to query latest release") {
        return true;
    }
    if line == "Build completed successfully (0 jobs)." {
        return true;
    }
    let first_char = line.chars().next().unwrap_or(' ');
    if matches!(first_char, '⚠' | '✔' | '✖' | 'ℹ') {
        let rest = line.trim_start_matches(|c: char| !c.is_ascii_whitespace());
        if rest.trim_start().starts_with('[') {
            return true;
        }
    }
    false
}
