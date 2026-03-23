use std::path::Path;

use super::{
    cfg_positive_int, is_timeout_error, require_tool, run_process, CompilerReq, CompilerRes,
    SrcCompiler,
};

pub(super) struct CompilerRocq;

const SOURCE_FILE: &str = "MdcWork.v";

impl SrcCompiler for CompilerRocq {
    fn srctype(&self) -> &str {
        "rocq"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec =
            match cfg_positive_int(&req.compcfg, "timeout_sec", "src.rocq.timeout_sec") {
                Ok(v) => v,
                Err(e) => return CompilerRes::err(e.to_string()),
            };

        let rocq = match require_tool("rocq") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let ws_root = req.mdcroot.join(".mdc").join("rocq");
        if let Err(e) = ensure_workspace(&ws_root) {
            return CompilerRes::err(e.to_string());
        }

        match run_process(
            &[&rocq, "compile", SOURCE_FILE],
            &format!("rocq compile {SOURCE_FILE}"),
            timeout_sec,
            Some(&ws_root),
        ) {
            Ok((rtcode, stdout, stderr)) => CompilerRes {
                result: rtcode == 0,
                stdout: stdout.trim().to_string(),
                stderr: stderr.trim().to_string(),
                rtcode,
            },
            Err(e) if is_timeout_error(&e) => CompilerRes::err_code(e.to_string(), 124),
            Err(e) => CompilerRes::err_code(e.to_string(), 1),
        }
    }
}

fn ensure_workspace(root: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)?;
    let project_path = root.join("_CoqProject");
    if !project_path.is_file() {
        std::fs::write(&project_path, "")?;
    }
    Ok(())
}
