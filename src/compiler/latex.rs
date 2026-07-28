use super::{
    process_error_result, require_tool, run_process, CompilerReq, CompilerRes, CompilerWorkspace,
    SrcCompiler,
};
use anyhow::{bail, Result};
use std::path::Path;

pub(super) struct CompilerLatex;

const DRIVER_FILE: &str = "Lib.tex";
const MAIN_FILE: &str = "Main.tex";
const DEFAULT_MAIN: &str =
    "\\documentclass{article}\n\n\\begin{document}\n\\input{Lib.tex}\n\\end{document}\n";

impl SrcCompiler for CompilerLatex {
    fn srctype(&self) -> &str {
        "latex"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = req.timeout_sec();

        let latexmk = match require_tool("latexmk") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        if let Err(e) = require_tool("xelatex") {
            return CompilerRes::err_code(e.to_string(), 127);
        }

        let workspace = match CompilerWorkspace::open(req, "latex") {
            Ok(workspace) => workspace,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (_, relative) = match workspace.lib_source(req) {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if let Err(error) = ensure_workspace(&workspace, &relative) {
            return CompilerRes::err(error.to_string());
        }
        let workspace_root = workspace.root();
        let pdf_path = workspace_root.join("Main.pdf");
        let args = [
            "-pdf",
            "-xelatex",
            "-interaction=nonstopmode",
            "-halt-on-error",
            MAIN_FILE,
        ];

        match run_process(&latexmk, args, "latexmk", timeout_sec, Some(workspace_root)) {
            Ok((rtcode, stdout, stderr)) => {
                if rtcode != 0 {
                    return CompilerRes {
                        stdout: String::new(),
                        stderr: summarize_latex_error(&stdout, &stderr),
                        rtcode,
                        interrupted: false,
                    };
                }
                if !pdf_path.is_file() {
                    return CompilerRes::err(format!(
                        "latexmk succeeded but pdf not found: {}",
                        pdf_path.display()
                    ));
                }
                CompilerRes::ok(format!(
                    "source tex: {}\nartifact pdf: {}",
                    req.source.display(),
                    pdf_path.display()
                ))
            }
            Err(e) => process_error_result(e, 127),
        }
    }
}

fn ensure_workspace(workspace: &CompilerWorkspace, relative: &Path) -> Result<()> {
    let main_path = workspace.root().join(MAIN_FILE);
    let main_snapshot = workspace.snapshot(&main_path)?;
    if main_snapshot.content().is_none() {
        workspace.replace_generated(&main_path, &main_snapshot, DEFAULT_MAIN.as_bytes())?;
    }

    let input = relative
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("LaTeX source path is not valid UTF-8"))?;
    if input.contains(['\n', '\r', '"', '{', '}', '%']) {
        bail!("LaTeX source path cannot be represented safely in Lib.tex: {input:?}");
    }
    let driver = format!("\\input{{\"Lib/{input}\"}}\n");
    let driver_path = workspace.root().join(DRIVER_FILE);
    let driver_snapshot = workspace.snapshot(&driver_path)?;
    if driver_snapshot.content() != Some(driver.as_bytes()) {
        workspace.replace_generated(&driver_path, &driver_snapshot, driver.as_bytes())?;
    }
    Ok(())
}

fn summarize_latex_error(stdout: &str, stderr: &str) -> String {
    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
    let error_lines: Vec<&str> = combined.lines().filter(|l| l.starts_with("! ")).collect();
    let summary: Vec<&str> = if error_lines.is_empty() {
        let all: Vec<&str> = combined.lines().collect();
        all[all.len().saturating_sub(24)..].to_vec()
    } else {
        let n = error_lines.len();
        error_lines[n.saturating_sub(8)..].to_vec()
    };
    summary.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_workspace(directory: &tempfile::TempDir) -> CompilerWorkspace {
        let mdcroot = directory.path().canonicalize().unwrap();
        let root = mdcroot.join(".mdc/latex");
        std::fs::create_dir_all(&root).unwrap();
        CompilerWorkspace {
            mdcroot,
            root,
            srctype: "latex".to_string(),
        }
    }

    #[test]
    fn workspace_preserves_main_and_updates_driver() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root();
        let custom_main = "\\documentclass{book}\n";
        std::fs::write(root.join(MAIN_FILE), custom_main).unwrap();

        ensure_workspace(&workspace, Path::new("notes/theorem.tex")).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join(MAIN_FILE)).unwrap(),
            custom_main
        );
        assert_eq!(
            std::fs::read_to_string(root.join(DRIVER_FILE)).unwrap(),
            "\\input{\"Lib/notes/theorem.tex\"}\n"
        );
    }

    #[test]
    fn workspace_creates_default_main_once() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = test_workspace(&dir);

        ensure_workspace(&workspace, Path::new("node.tex")).unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.root().join(MAIN_FILE)).unwrap(),
            DEFAULT_MAIN
        );
    }

    #[test]
    fn generated_write_cannot_follow_replaced_workspace_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let workspace = test_workspace(&dir);
        let root = workspace.root().to_path_buf();
        let displaced = workspace.mdcroot.join(".mdc/displaced-latex");
        let hook_root = root.clone();
        let hook_outside = outside.path().to_path_buf();
        let hook_displaced = displaced.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteBeforeDirectoryBinding,
            move || {
                std::fs::rename(hook_root, hook_displaced).unwrap();
                symlink(hook_outside, &root).unwrap();
            },
        );

        let error = ensure_workspace(&workspace, Path::new("node.tex")).unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(!outside.path().join(MAIN_FILE).exists());
        assert!(!displaced.join(MAIN_FILE).exists());
    }
}
