use super::{
    cfg_positive_int, is_timeout_error, require_tool, run_process, source_path, CompilerReq,
    CompilerRes, SrcCompiler,
};

pub(super) struct CompilerLatex;

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

        let tex_dir = req.mdcroot.join(".mdc").join("latex");
        let tex_path = source_path(&req.mdcroot, "latex");
        let pdf_path = tex_dir.join("MdcWork.pdf");

        match run_process(
            &[
                &latexmk,
                "-pdf",
                "-xelatex",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-outdir=.",
                "MdcWork.tex",
            ],
            "latexmk",
            timeout_sec,
            Some(&tex_dir),
        ) {
            Ok((rtcode, stdout, stderr)) => {
                if rtcode != 0 {
                    return CompilerRes {
                        result: false,
                        stdout: String::new(),
                        stderr: summarize_latex_error(&stdout, &stderr),
                        rtcode,
                    };
                }
                if !pdf_path.is_file() {
                    return CompilerRes::err(format!(
                        "latexmk succeeded but pdf not found: {}",
                        pdf_path.display()
                    ));
                }
                CompilerRes::ok(format!(
                    "artifact dir: {}\nartifact tex: {}\nartifact pdf: {}",
                    tex_dir.display(),
                    tex_path.display(),
                    pdf_path.display()
                ))
            }
            Err(e) if is_timeout_error(&e) => CompilerRes::err_code(e.to_string(), 124),
            Err(e) => CompilerRes::err_code(e.to_string(), 127),
        }
    }
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
