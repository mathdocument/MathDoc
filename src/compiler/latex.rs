use super::{
    cfg_positive_int, is_timeout_error, require_tool, run_process, CompilerReq, CompilerRes,
    SrcCompiler,
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
        if let Err(e) = std::fs::create_dir_all(&tex_dir) {
            return CompilerRes::err(format!("failed to create latex artifact dir: {e}"));
        }
        let stem = format!(
            "temp-latex-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let tex_path = tex_dir.join(format!("{stem}.tex"));
        let pdf_path = tex_dir.join(format!("{stem}.pdf"));

        let payload = latex_payload(&req.content, &req.preamble, &req.postamble);
        if let Err(e) = std::fs::write(&tex_path, &payload) {
            return CompilerRes::err(format!("failed to write latex source: {e}"));
        }

        let tex_name = tex_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match run_process(
            &[
                &latexmk,
                "-pdf",
                "-xelatex",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-outdir=.",
                &tex_name,
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

fn latex_payload(content: &str, preamble: &str, postamble: &str) -> String {
    if content.contains(r"\documentclass") {
        return content.to_string();
    }
    let preamble_text = preamble.trim_end_matches('\n');
    let body = content.trim_end_matches('\n');
    let postamble_text = postamble.trim_matches('\n');

    let mut parts = vec![preamble_text, body];
    if !postamble_text.is_empty() {
        parts.push(postamble_text);
    }
    format!("{}\n", parts.join("\n"))
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
