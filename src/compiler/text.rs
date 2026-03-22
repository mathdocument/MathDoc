use super::{CompilerReq, CompilerRes, SrcCompiler};

pub(super) struct CompilerText;

impl SrcCompiler for CompilerText {
    fn srctype(&self) -> &str {
        "text"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        CompilerRes::ok(req.content.trim_end_matches('\n'))
    }
}
