use super::{
    process_error_result, require_tool, run_process, CompilerReq, CompilerRes, CompilerWorkspace,
    SrcCompiler,
};

pub(super) struct CompilerPython;

impl SrcCompiler for CompilerPython {
    fn srctype(&self) -> &str {
        "python"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = req.timeout_sec();
        let python = match require_tool("python3").or_else(|_| require_tool("python")) {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        let workspace = match CompilerWorkspace::open(req, "python") {
            Ok(workspace) => workspace,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (lib_root, relative) = match workspace.lib_source(req) {
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
                stdout,
                stderr,
                rtcode,
                interrupted: false,
            },
            Err(e) => process_error_result(e, 127),
        }
    }
}
