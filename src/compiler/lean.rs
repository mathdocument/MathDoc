//! Lean 4 / Mathlib compiler.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::{
    cfg_positive_int, cfg_str_opt, emit_progress, is_timeout_error, require_tool, run_process,
    CompilerReq, CompilerRes, SrcCompiler,
};

pub struct CompilerLean;

const MANAGED_MARKER: &str = "# Managed by MathDoc";
const MODULE_NAME: &str = "MathDocCheck";
const CACHE_STAMP: &str = ".mathdoc-cache.stamp";

// ── Workspace layout ──────────────────────────────────────────────────────────

struct LeanRelease {
    toolchain: String,
    mathlib_rev: String,
}

struct LeanWorkspace {
    root: PathBuf,
    lakefile_path: PathBuf,
    toolchain_path: PathBuf,
    module_path: PathBuf,
    manifest_path: PathBuf,
    mathlib_sentinel: PathBuf,
    cache_stamp_path: PathBuf,
}

impl LeanWorkspace {
    fn new(root: PathBuf) -> Self {
        LeanWorkspace {
            lakefile_path: root.join("lakefile.toml"),
            toolchain_path: root.join("lean-toolchain"),
            module_path: root.join(format!("{MODULE_NAME}.lean")),
            manifest_path: root.join("lake-manifest.json"),
            mathlib_sentinel: root.join(".lake/packages/mathlib/Mathlib.lean"),
            cache_stamp_path: root.join(CACHE_STAMP),
            root,
        }
    }
}

// ── SrcCompiler impl ──────────────────────────────────────────────────────────

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
        let imports = match read_imports(&req.compcfg) {
            Ok(v) => v,
            Err(e) => return CompilerRes::err(e.to_string()),
        };
        let preamble = match cfg_str_opt(&req.compcfg, "preamble", "src.lean.preamble") {
            Ok(v) => v,
            Err(e) => return CompilerRes::err(e.to_string()),
        };

        let lake = match require_tool("lake") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        let lean = match require_tool("lean") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let release = match detect_lean_release(&lean, timeout_sec.min(10)) {
            Ok(r) => r,
            Err(e) => return CompilerRes::err(e.to_string()),
        };

        let workspace = lean_workspace(&req.mdcroot);
        if let Err(e) = write_workspace_scaffold(&workspace, &release) {
            return CompilerRes::err(format!("failed to prepare Lean workspace: {e}"));
        }
        if let Err(e) = ensure_cached_dependencies(
            &workspace,
            &release,
            &lake,
            setup_timeout_sec,
            &req.progress,
        ) {
            return CompilerRes::err(e.to_string());
        }

        let payload = lean_payload(&req.content, &imports, &preamble);
        if let Err(e) = std::fs::write(&workspace.module_path, &payload) {
            return CompilerRes::err(format!("failed to write Lean module: {e}"));
        }

        emit_progress(
            &req.progress,
            &format!("building Lean module `{MODULE_NAME}` with `lake build +{MODULE_NAME}`"),
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
            Some(&workspace.root),
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

// ── Lean workspace helpers ────────────────────────────────────────────────────

fn lean_workspace(mdcroot: &Path) -> LeanWorkspace {
    LeanWorkspace::new(mdcroot.join(".mdc").join("lean"))
}

fn detect_lean_release(lean_path: &str, timeout_sec: u64) -> Result<LeanRelease> {
    let (_, stdout, stderr) = run_process(&[lean_path, "--version"], "lean", timeout_sec, None)?;
    let output = if stdout.is_empty() { &stderr } else { &stdout };
    let version = parse_lean_version(output)
        .ok_or_else(|| anyhow::anyhow!("failed to detect Lean version from `lean --version`"))?;
    Ok(LeanRelease {
        toolchain: format!("leanprover/lean4:v{version}"),
        mathlib_rev: format!("v{version}"),
    })
}

fn parse_lean_version(output: &str) -> Option<String> {
    let re = regex::Regex::new(r"version\s+([0-9]+\.[0-9]+\.[0-9]+)").ok()?;
    re.captures(output)?.get(1).map(|m| m.as_str().to_string())
}

fn write_workspace_scaffold(workspace: &LeanWorkspace, release: &LeanRelease) -> Result<()> {
    std::fs::create_dir_all(&workspace.root)?;
    std::fs::write(&workspace.lakefile_path, lakefile_text(release))?;
    std::fs::write(
        &workspace.toolchain_path,
        format!("{}\n", release.toolchain),
    )?;
    Ok(())
}

fn ensure_cached_dependencies(
    workspace: &LeanWorkspace,
    release: &LeanRelease,
    lake_path: &str,
    timeout_sec: u64,
    progress: &Option<Box<dyn Fn(&str)>>,
) -> Result<()> {
    if cache_is_ready(workspace, release) {
        return Ok(());
    }

    emit_progress(
        progress,
        &format!(
            "preparing Lean workspace in `.mdc/lean` and resolving Mathlib dependencies (first run may take a while)"
        ),
    );
    emit_progress(
        progress,
        "resolving Mathlib dependencies with `lake update`",
    );
    let (rtcode, stdout, stderr) = run_process(
        &[lake_path, "--quiet", "--no-ansi", "update"],
        "lake update",
        timeout_sec,
        Some(&workspace.root),
    )?;
    if rtcode != 0 {
        bail!(
            "failed to initialize Lean dependencies:\n{}",
            combine_output(&stdout, &stderr)
        );
    }

    emit_progress(
        progress,
        "downloading Mathlib cache with `lake exe cache get`",
    );
    let (rtcode, stdout, stderr) = run_process(
        &[lake_path, "--quiet", "--no-ansi", "exe", "cache", "get"],
        "lake exe cache get",
        timeout_sec,
        Some(&workspace.root),
    )?;
    if rtcode != 0 {
        bail!(
            "failed to download Mathlib cache:\n{}",
            combine_output(&stdout, &stderr)
        );
    }

    std::fs::write(&workspace.cache_stamp_path, cache_signature(release))?;
    Ok(())
}

fn cache_is_ready(workspace: &LeanWorkspace, release: &LeanRelease) -> bool {
    if !workspace.manifest_path.is_file()
        || !workspace.mathlib_sentinel.is_file()
        || !workspace.cache_stamp_path.is_file()
    {
        return false;
    }
    std::fs::read_to_string(&workspace.cache_stamp_path)
        .map(|s| s == cache_signature(release))
        .unwrap_or(false)
}

fn cache_signature(release: &LeanRelease) -> String {
    format!(
        "toolchain={}\nmathlib_rev={}",
        release.toolchain, release.mathlib_rev
    )
}

fn lakefile_text(release: &LeanRelease) -> String {
    format!(
        "{MANAGED_MARKER}\nname = \"mathdoclean\"\nversion = \"0.1.0\"\ndefaultTargets = [\"{MODULE_NAME}\"]\n\n\
         [leanOptions]\npp.unicode.fun = true\nrelaxedAutoImplicit = false\n\
         weak.linter.mathlibStandardSet = true\nmaxSynthPendingDepth = 3\n\n\
         [[require]]\nname = \"mathlib\"\nscope = \"leanprover-community\"\nrev = \"{}\"\n\n\
         [[lean_lib]]\nname = \"{MODULE_NAME}\"\n",
        release.mathlib_rev
    )
}

// ── Lean payload assembly ─────────────────────────────────────────────────────

fn read_imports(compcfg: &HashMap<String, toml::Value>) -> Result<Vec<String>> {
    let raw = compcfg
        .get("imports")
        .cloned()
        .unwrap_or(toml::Value::Array(vec![toml::Value::String(
            "Mathlib".to_string(),
        )]));
    match raw.as_array() {
        None => bail!("config key 'src.lean.imports' must be an array of strings"),
        Some(arr) => {
            let mut imports = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for item in arr {
                match item.as_str() {
                    None => bail!("config key 'src.lean.imports' must be an array of strings"),
                    Some(s) => {
                        let v = s.trim().to_string();
                        if !v.is_empty() && seen.insert(v.clone()) {
                            imports.push(v);
                        }
                    }
                }
            }
            Ok(imports)
        }
    }
}

fn lean_payload(content: &str, imports: &[String], preamble: &str) -> String {
    let (head_imports, body) = extract_leading_imports(content);

    let mut merged: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for imp in imports.iter().chain(head_imports.iter()) {
        if seen.insert(imp.clone()) {
            merged.push(imp.clone());
        }
    }

    let mut parts: Vec<String> = merged.iter().map(|i| format!("import {i}")).collect();
    if !parts.is_empty() {
        parts.push(String::new());
    }
    parts.push("set_option warn.sorry true".to_string());

    let preamble_text = preamble.trim_matches('\n');
    if !preamble_text.is_empty() {
        parts.push(preamble_text.to_string());
    }
    let body_text = body.trim_matches('\n');
    if !body_text.is_empty() {
        parts.push(String::new());
        parts.push(body_text.to_string());
    }

    format!("{}\n", parts.join("\n").trim_end_matches('\n'))
}

fn extract_leading_imports(content: &str) -> (Vec<String>, String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    // Skip leading blank lines
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    let mut imports = Vec::new();
    while i < lines.len() {
        let line = lines[i].trim();
        if !line.starts_with("import ") {
            break;
        }
        let target = line["import ".len()..].trim();
        if !target.is_empty() {
            imports.push(target.to_string());
        }
        i += 1;
    }
    (imports, lines[i..].join("\n"))
}

// ── Build output processing ───────────────────────────────────────────────────

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
            if text.is_empty() {
                continue;
            }
            if is_noise_line(text) {
                continue;
            }
            lines.push(text.to_string());
        }
    }
    lines
}

fn is_noise_line(line: &str) -> bool {
    // Lake progress indicators: ⚠ [1/42] Built Foo
    if line.starts_with("warning: failed to query latest release") {
        return true;
    }
    if line == "Build completed successfully (0 jobs)." {
        return true;
    }
    // Unicode progress chars: ⚠ ✔ ✖ ℹ followed by [N/N] Built
    let first_char = line.chars().next().unwrap_or(' ');
    if matches!(first_char, '⚠' | '✔' | '✖' | 'ℹ') {
        let rest = line.trim_start_matches(|c: char| !c.is_ascii_whitespace());
        if rest.trim_start().starts_with('[') {
            return true;
        }
    }
    false
}
