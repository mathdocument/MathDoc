use super::{CompilerReq, CompilerRes, SrcCompiler};

pub(super) struct CompilerNatl;

impl SrcCompiler for CompilerNatl {
    fn srctype(&self) -> &str {
        "natl"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        CompilerRes::ok(req.content.trim_end_matches('\n'))
    }
}
