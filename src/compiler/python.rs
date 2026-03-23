use super::{
    cfg_positive_int, is_timeout_error, require_tool, run_process, source_path, CompilerReq,
    CompilerRes, SrcCompiler,
};

pub(super) struct CompilerPython;

impl SrcCompiler for CompilerPython {
    fn srctype(&self) -> &str {
        "python"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec =
            match cfg_positive_int(&req.compcfg, "timeout_sec", "src.python.timeout_sec") {
                Ok(v) => v,
                Err(e) => return CompilerRes::err(e.to_string()),
            };
        let python = match require_tool("python3").or_else(|_| require_tool("python")) {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        let src = source_path(&req.mdcroot, "python");
        let src_str = src.to_string_lossy();
        match run_process(&[&python, &src_str], "python", timeout_sec, None) {
            Ok((rtcode, stdout, stderr)) => CompilerRes {
                result: rtcode == 0,
                stdout,
                stderr,
                rtcode,
            },
            Err(e) if is_timeout_error(&e) => CompilerRes::err_code(e.to_string(), 124),
            Err(e) => CompilerRes::err_code(e.to_string(), 127),
        }
    }
}
