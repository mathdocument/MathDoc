use super::{
    cfg_positive_int, process_error_result, require_tool, run_process, source_path, CompilerReq,
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
        let work_dir = src
            .parent()
            .expect("compiler source has a parent directory");
        match run_process(&python, [&src], "python", timeout_sec, Some(work_dir)) {
            Ok((rtcode, stdout, stderr)) => CompilerRes {
                result: rtcode == 0,
                stdout,
                stderr,
                rtcode,
                interrupted: false,
            },
            Err(e) => process_error_result(e, 127),
        }
    }
}
