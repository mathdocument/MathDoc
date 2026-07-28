use super::{
    cfg_positive_int, lib_source, process_error_result, require_tool, run_process, CompilerReq,
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
        let (lib_root, relative) = match lib_source(req, "python") {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        match run_process(
            &python,
            [std::path::Path::new("-B"), relative.as_path()],
            "python",
            timeout_sec,
            Some(&lib_root),
        ) {
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
