use anyhow::{bail, Result};
use std::path::Path;

use super::{
    cfg_positive_int, emit_progress, process_error_result, require_tool, run_process, CompilerReq,
    CompilerRes, ProgressCallback, SrcCompiler,
};

pub(super) struct CompilerLean;

const MODULE_NAME: &str = "MdcWork";
const SETUP_MARKER: &str = ".mdc-setup-v1";
const SETUP_MARKER_CONTENT: &[u8] = b"lake-init-v1\n";

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
            return process_error_result(e, 1);
        }

        emit_progress(
            &req.progress,
            &format!("building with `lake build +{MODULE_NAME}`"),
        );
        let module = format!("+{MODULE_NAME}");
        match run_process(
            &lake,
            ["--quiet", "--no-ansi", "build", module.as_str()],
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
                    interrupted: false,
                }
            }
            Err(e) => process_error_result(e, 1),
        }
    }
}

// ── Workspace setup ──────────────────────────────────────────────────────────

fn ensure_workspace(
    root: &Path,
    lake_path: &Path,
    timeout_sec: u64,
    progress: &Option<ProgressCallback>,
) -> Result<()> {
    std::fs::create_dir_all(root)?;
    if setup_complete(root)? {
        return Ok(());
    }

    if has_lakefile(root) && validate_workspace(root, lake_path, timeout_sec).is_ok() {
        crate::workspace::atomic_create_if_missing(&root.join(SETUP_MARKER), SETUP_MARKER_CONTENT)?;
        return Ok(());
    }

    emit_progress(
        progress,
        "initializing Lean workspace with `lake init mdc_work`",
    );
    let staging = tempfile::Builder::new()
        .prefix(".mdc-lean-init-")
        .tempdir_in(root.parent().unwrap_or(root))?;
    let (rtcode, stdout, stderr) = run_process(
        lake_path,
        ["init", "mdc_work"],
        "lake init",
        timeout_sec,
        Some(staging.path()),
    )?;
    if rtcode != 0 {
        bail!("lake init failed:\n{}", combine_output(&stdout, &stderr));
    }

    install_setup_file(staging.path(), root, "lakefile.toml")?;
    install_setup_file(staging.path(), root, "lakefile.lean")?;
    install_setup_file(staging.path(), root, "lean-toolchain")?;
    validate_workspace(root, lake_path, timeout_sec)?;
    crate::workspace::atomic_create_if_missing(&root.join(SETUP_MARKER), SETUP_MARKER_CONTENT)?;
    Ok(())
}

fn setup_complete(root: &Path) -> Result<bool> {
    let marker = root.join(SETUP_MARKER);
    match std::fs::symlink_metadata(&marker) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            bail!("invalid Lean setup marker: {}", marker.display())
        }
        Ok(_) => {
            if std::fs::read(&marker)? != SETUP_MARKER_CONTENT {
                bail!("invalid Lean setup marker: {}", marker.display());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn has_lakefile(root: &Path) -> bool {
    root.join("lakefile.toml").is_file() || root.join("lakefile.lean").is_file()
}

fn validate_workspace(root: &Path, lake_path: &Path, timeout_sec: u64) -> Result<()> {
    let (rtcode, stdout, stderr) = run_process(
        lake_path,
        ["env", "lean", "--version"],
        "lake env lean --version",
        timeout_sec,
        Some(root),
    )?;
    if rtcode != 0 {
        bail!(
            "Lean workspace validation failed:\n{}",
            combine_output(&stdout, &stderr)
        );
    }
    Ok(())
}

fn install_setup_file(staging: &Path, root: &Path, name: &str) -> Result<()> {
    let generated_path = staging.join(name);
    let Ok(generated) = std::fs::read(&generated_path) else {
        return Ok(());
    };
    let target = root.join(name);
    let snapshot = crate::workspace::FileSnapshot::capture(&target)?;
    match snapshot.content() {
        None => {
            crate::workspace::atomic_replace(&target, &snapshot, &generated)?;
        }
        Some(existing) if existing != generated && generated.starts_with(existing) => {
            crate::workspace::atomic_replace(&target, &snapshot, &generated)?;
        }
        Some(_) => {}
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn interrupted_setup_repairs_partial_lakefile_and_marks_completion() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("lean");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("lakefile.toml"), "name = \"mdc_work\"\n").unwrap();

        let lake = dir.path().join("fake-lake");
        std::fs::write(
            &lake,
            r#"#!/bin/sh
if [ "$1" = "env" ]; then
  grep -q 'version = "0.1.0"' lakefile.toml
  exit $?
fi
if [ "$1" = "init" ]; then
  printf x >> "$(dirname "$0")/init-count"
  printf 'name = "mdc_work"\nversion = "0.1.0"\n' > lakefile.toml
  printf 'leanprover/lean4:stable\n' > lean-toolchain
  exit 0
fi
exit 1
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&lake, permissions).unwrap();

        ensure_workspace(&root, &lake, 5, &None).unwrap();
        assert!(std::fs::read_to_string(root.join("lakefile.toml"))
            .unwrap()
            .contains("version = \"0.1.0\""));
        assert_eq!(
            std::fs::read(root.join(SETUP_MARKER)).unwrap(),
            SETUP_MARKER_CONTENT
        );

        ensure_workspace(&root, &lake, 5, &None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("init-count")).unwrap(),
            "x"
        );
    }
}
