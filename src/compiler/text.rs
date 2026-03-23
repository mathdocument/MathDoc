use super::{CompilerReq, CompilerRes, SrcCompiler};

pub(super) struct CompilerText;

impl SrcCompiler for CompilerText {
    fn srctype(&self) -> &str {
        "text"
    }

    fn compile(&self, _req: &CompilerReq) -> CompilerRes {
        CompilerRes::ok("")
    }
}
