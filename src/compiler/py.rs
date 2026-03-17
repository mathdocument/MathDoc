use super::{
    cfg_positive_int, is_timeout_error, require_tool, run_process, CompilerReq, CompilerRes,
    SrcCompiler,
};

pub(super) struct CompilerPy;

impl SrcCompiler for CompilerPy {
    fn srctype(&self) -> &str {
        "py"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = match cfg_positive_int(&req.compcfg, "timeout_sec", "src.py.timeout_sec")
        {
            Ok(v) => v,
            Err(e) => return CompilerRes::err(e.to_string()),
        };
        let python = match require_tool("python3").or_else(|_| require_tool("python")) {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        match run_process(&[&python, "-c", &req.content], "python", timeout_sec, None) {
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
