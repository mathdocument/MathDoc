use anyhow::{bail, Result};
use std::path::{Component, Path};

use super::{
    process_error_result, require_tool, run_process, CompilerReq, CompilerRes, CompilerWorkspace,
    ProgressCallback, SrcCompiler,
};

pub(super) struct CompilerLean;

const DRIVER_MODULE: &str = "Lib";
const DRIVER_FILE: &str = "Lib.lean";

impl SrcCompiler for CompilerLean {
    fn srctype(&self) -> &str {
        "lean"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = match req.timeout_sec() {
            Ok(timeout) => timeout,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let setup_timeout_sec = match req.setup_timeout_sec() {
            Ok(timeout) => timeout,
            Err(error) => return CompilerRes::err(error.to_string()),
        };

        let lake = match require_tool("lake") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let workspace = match CompilerWorkspace::open(req, "lean") {
            Ok(workspace) => workspace,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (_, relative) = match workspace.lib_source(req) {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if let Err(error) = ensure_workspace(&workspace, &lake, setup_timeout_sec, &req.progress) {
            return process_error_result(error, 1);
        }
        let module = match module_name_from_relative(&relative) {
            Ok(module) => module,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if let Err(error) = write_driver(&workspace, &module) {
            return CompilerRes::err(error.to_string());
        }

        req.emit_progress(&format!("building `{DRIVER_MODULE}` importing `{module}`"));
        match run_process(
            &lake,
            ["--quiet", "--no-ansi", "build", "+Lib"],
            "lake build +Lib",
            timeout_sec,
            Some(workspace.root()),
        ) {
            Ok((rtcode, stdout, stderr)) => {
                let (out, err) = classify_build_output(&stdout, &stderr, rtcode == 0);
                CompilerRes {
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

fn ensure_workspace(
    workspace: &CompilerWorkspace,
    lake_path: &Path,
    timeout_sec: u64,
    progress: &Option<ProgressCallback>,
) -> Result<()> {
    let root = workspace.root();
    let toml_path = root.join("lakefile.toml");
    let lean_path = root.join("lakefile.lean");
    let toml_snapshot = workspace.snapshot(&toml_path)?;
    let lean_snapshot = workspace.snapshot(&lean_path)?;
    if toml_snapshot.content().is_some() && lean_snapshot.content().is_some() {
        bail!(
            "both lakefile.toml and lakefile.lean exist in {}",
            root.display()
        );
    }
    if lean_snapshot.content().is_some() {
        bail!(
            "{} must use a standard lakefile.toml with a `Lib` lean library",
            root.display()
        );
    }
    if let Some(content) = toml_snapshot.content() {
        validate_lakefile(content)?;
    }

    let needs_lakefile = toml_snapshot.content().is_none();
    let toolchain_path = root.join("lean-toolchain");
    let toolchain_snapshot = workspace.snapshot(&toolchain_path)?;
    let needs_toolchain = toolchain_snapshot.content().is_none();
    if !needs_lakefile && !needs_toolchain {
        return Ok(());
    }

    if let Some(progress) = progress {
        progress("initializing Lean library workspace with `lake init Lib lib`");
    }
    let staging = tempfile::Builder::new()
        .prefix("mdc-lean-init-")
        .tempdir()?;
    let (rtcode, stdout, stderr) = run_process(
        lake_path,
        ["init", "Lib", "lib"],
        "lake init Lib lib",
        timeout_sec,
        Some(staging.path()),
    )?;
    if rtcode != 0 {
        bail!("lake init failed:\n{}", combine_output(&stdout, &stderr));
    }

    let mut setup_changes = Vec::new();
    let setup_result = (|| -> Result<()> {
        if needs_lakefile {
            if let Some(change) = install_setup_file(staging.path(), workspace, "lakefile.toml")? {
                setup_changes.push(change);
            }
        }
        if needs_toolchain {
            if let Some(change) = install_setup_file(staging.path(), workspace, "lean-toolchain")? {
                setup_changes.push(change);
            }
        }
        let content = workspace
            .snapshot(&toml_path)?
            .content()
            .ok_or_else(|| anyhow::anyhow!("lake init did not generate lakefile.toml"))?
            .to_vec();
        validate_lakefile(&content)?;
        validate_workspace(root, lake_path, timeout_sec)
    })();
    match setup_result {
        Ok(()) => Ok(()),
        Err(error) => match rollback_setup_changes(setup_changes) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "{error}; additionally failed to restore the previous Lake configuration: {rollback_error}"
            )),
        },
    }
}

fn validate_lakefile(content: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(content)?;
    let parsed = text
        .parse::<toml::Value>()
        .map_err(|error| anyhow::anyhow!("invalid lakefile.toml: {error}"))?;
    let library = parsed
        .get("lean_lib")
        .and_then(toml::Value::as_array)
        .and_then(|libraries| {
            libraries.iter().find(|library| {
                library.get("name").and_then(toml::Value::as_str) == Some(DRIVER_MODULE)
            })
        });
    let Some(library) = library else {
        bail!("lakefile.toml must declare `[[lean_lib]] name = \"Lib\"`");
    };
    if library
        .get("srcDir")
        .and_then(toml::Value::as_str)
        .is_some_and(|source| source != ".")
    {
        bail!("the `Lib` lean library must use the workspace root as its srcDir");
    }
    if parsed
        .get("buildDir")
        .and_then(toml::Value::as_str)
        .is_some_and(|build| build != ".lake/build")
    {
        bail!("lakefile.toml must use `.lake/build` as its buildDir");
    }
    Ok(())
}

fn module_name_from_relative(relative: &Path) -> Result<String> {
    let source = relative.with_extension("");
    let mut components = vec![DRIVER_MODULE.to_string()];
    for component in source.components() {
        let Component::Normal(name) = component else {
            bail!("invalid Lean module path {}", relative.display());
        };
        let name = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Lean module path is not valid UTF-8"))?;
        if name.contains(['«', '»', '\n', '\r']) {
            bail!("Lean module path component cannot be quoted safely: {name:?}");
        }
        components.push(format!("«{name}»"));
    }
    if components.len() == 1 {
        bail!("empty Lean module path");
    }
    Ok(components.join("."))
}

fn write_driver(workspace: &CompilerWorkspace, module: &str) -> Result<()> {
    let path = workspace.root().join(DRIVER_FILE);
    let snapshot = workspace.snapshot(&path)?;
    let content = format!("import {module}\n");
    if snapshot.content() != Some(content.as_bytes()) {
        workspace.replace_generated(&path, &snapshot, content.as_bytes())?;
    }
    Ok(())
}

fn rollback_setup_changes(changes: Vec<crate::workspace::AppliedWrite>) -> Result<()> {
    let mut errors = Vec::new();
    for change in changes.into_iter().rev() {
        if let Err(error) = change.rollback() {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("; "))
    }
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

fn install_setup_file(
    staging: &Path,
    workspace: &CompilerWorkspace,
    name: &str,
) -> Result<Option<crate::workspace::AppliedWrite>> {
    let generated_path = staging.join(name);
    let generated = std::fs::read(&generated_path)
        .map_err(|error| anyhow::anyhow!("reading {}: {error}", generated_path.display()))?;
    let target = workspace.root().join(name);
    let snapshot = workspace.snapshot(&target)?;
    if snapshot.content().is_none() {
        return workspace
            .replace_generated(&target, &snapshot, &generated)
            .map(Some);
    }
    Ok(None)
}

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

    fn test_workspace(directory: &tempfile::TempDir) -> CompilerWorkspace {
        let mdcroot = directory.path().canonicalize().unwrap();
        let root = mdcroot.join(".mdc/lean");
        std::fs::create_dir_all(&root).unwrap();
        CompilerWorkspace {
            mdcroot,
            root,
            srctype: "lean".to_string(),
        }
    }

    #[test]
    fn module_path_uses_lib_prefix_and_quoted_components() {
        assert_eq!(
            module_name_from_relative(Path::new("EGA/1-1.1.2.lean")).unwrap(),
            "Lib.«EGA».«1-1.1.2»"
        );
    }

    #[test]
    fn workspace_setup_uses_standard_library_template_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root();
        let lake = executable(
            dir.path(),
            r#"#!/bin/sh
if [ "$1" = "env" ]; then
  grep -q 'name = "Lib"' lakefile.toml
  exit $?
fi
if [ "$1" = "init" ] && [ "$2" = "Lib" ] && [ "$3" = "lib" ]; then
  printf x >> "$(dirname "$0")/init-count"
  printf 'name = "Lib"\nversion = "0.1.0"\ndefaultTargets = ["Lib"]\n\n[[lean_lib]]\nname = "Lib"\n' > lakefile.toml
  printf 'leanprover/lean4:stable\n' > lean-toolchain
  exit 0
fi
exit 1
"#,
        );

        ensure_workspace(&workspace, &lake, 5, &None).unwrap();
        assert!(std::fs::read_to_string(root.join("lakefile.toml"))
            .unwrap()
            .contains("name = \"Lib\""));
        assert!(root.join("lean-toolchain").is_file());

        ensure_workspace(&workspace, &lake, 5, &None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("init-count")).unwrap(),
            "x"
        );
    }

    #[test]
    fn conventional_toml_preserves_user_configuration() {
        let dir = tempfile::TempDir::new().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root();
        std::fs::write(root.join("lean-toolchain"), "leanprover/lean4:stable\n").unwrap();
        let custom = "# keep this comment\nname = \"custom\"\n\n[[require]]\nname = \"mathlib\"\nscope = \"leanprover-community\"\n\n[[lean_lib]]\nname = \"Lib\"\n";
        std::fs::write(root.join("lakefile.toml"), custom).unwrap();

        ensure_workspace(&workspace, Path::new("missing-lake"), 5, &None).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("lakefile.toml")).unwrap(),
            custom
        );
    }

    #[test]
    fn non_lib_configuration_is_preserved_and_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root();
        let custom = "name = \"custom\"\n\n[[lean_lib]]\nname = \"Other\"\n";
        std::fs::write(root.join("lakefile.toml"), custom).unwrap();

        let error = ensure_workspace(&workspace, Path::new("missing-lake"), 5, &None).unwrap_err();

        assert!(error.to_string().contains("name = \"Lib\""));
        assert_eq!(
            std::fs::read_to_string(root.join("lakefile.toml")).unwrap(),
            custom
        );
    }

    #[test]
    fn custom_source_and_build_layouts_are_rejected() {
        for (setting, expected) in [
            ("srcDir = \"src\"", "srcDir"),
            (
                "buildDir = \"custom-build\"\n\n[[lean_lib]]\nname = \"Lib\"",
                "buildDir",
            ),
        ] {
            let content = if setting.starts_with("buildDir") {
                format!("name = \"custom\"\n{setting}\n")
            } else {
                format!("name = \"custom\"\n\n[[lean_lib]]\nname = \"Lib\"\n{setting}\n")
            };

            let error = validate_lakefile(content.as_bytes()).unwrap_err();

            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn failed_validation_rolls_back_generated_setup() {
        let dir = tempfile::TempDir::new().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root();
        let lake = executable(
            dir.path(),
            r#"#!/bin/sh
if [ "$1" = "init" ]; then
  printf 'name = "Lib"\n\n[[lean_lib]]\nname = "Lib"\n' > lakefile.toml
  printf 'leanprover/lean4:stable\n' > lean-toolchain
  exit 0
fi
exit 1
"#,
        );

        assert!(ensure_workspace(&workspace, &lake, 5, &None).is_err());
        assert!(!root.join("lakefile.toml").exists());
        assert!(!root.join("lean-toolchain").exists());
    }

    fn executable(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("fake-lake");
        std::fs::write(&path, content).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
