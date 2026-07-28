use super::{
    cfg_positive_int, lib_source, process_error_result, require_tool, run_process, CompilerReq,
    CompilerRes, SrcCompiler,
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
        let timeout_sec =
            match cfg_positive_int(&req.compcfg, "timeout_sec", "src.latex.timeout_sec") {
                Ok(v) => v,
                Err(e) => return CompilerRes::err(e.to_string()),
            };

        let latexmk = match require_tool("latexmk") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        if let Err(e) = require_tool("xelatex") {
            return CompilerRes::err_code(e.to_string(), 127);
        }

        let (lib_root, relative) = match lib_source(req, "latex") {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let workspace_root = lib_root.parent().expect("Lib has a workspace parent");
        if let Err(error) = ensure_workspace(workspace_root, &relative) {
            return CompilerRes::err(error.to_string());
        }
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
                        result: false,
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

fn ensure_workspace(root: &Path, relative: &Path) -> Result<()> {
    let main_path = root.join(MAIN_FILE);
    let main_snapshot = crate::workspace::FileSnapshot::capture(&main_path)?;
    if main_snapshot.content().is_none() {
        main_snapshot.replace(&main_path, DEFAULT_MAIN.as_bytes())?;
    }

    let input = relative
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("LaTeX source path is not valid UTF-8"))?;
    if input.contains(['\n', '\r', '"', '{', '}', '%']) {
        bail!("LaTeX source path cannot be represented safely in Lib.tex: {input:?}");
    }
    let driver = format!("\\input{{\"Lib/{input}\"}}\n");
    let driver_path = root.join(DRIVER_FILE);
    let driver_snapshot = crate::workspace::FileSnapshot::capture(&driver_path)?;
    if driver_snapshot.content() != Some(driver.as_bytes()) {
        driver_snapshot.replace(&driver_path, driver.as_bytes())?;
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

    #[test]
    fn workspace_preserves_main_and_updates_driver() {
        let dir = tempfile::tempdir().unwrap();
        let custom_main = "\\documentclass{book}\n";
        std::fs::write(dir.path().join(MAIN_FILE), custom_main).unwrap();

        ensure_workspace(dir.path(), Path::new("notes/theorem.tex")).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join(MAIN_FILE)).unwrap(),
            custom_main
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(DRIVER_FILE)).unwrap(),
            "\\input{\"Lib/notes/theorem.tex\"}\n"
        );
    }

    #[test]
    fn workspace_creates_default_main_once() {
        let dir = tempfile::tempdir().unwrap();

        ensure_workspace(dir.path(), Path::new("node.tex")).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join(MAIN_FILE)).unwrap(),
            DEFAULT_MAIN
        );
    }
}
