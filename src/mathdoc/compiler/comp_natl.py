from .base import SrcCompiler, CompilerReq, CompilerRes


class CompilerNatl(SrcCompiler):
    @property
    def srctype(self) -> str:
        return "natl"

    def compile(self, req: CompilerReq) -> CompilerRes:
        return CompilerRes(
            result=True,
            stdout=req.content.rstrip("\n"),
            stderr="",
            rtcode=0,
        )
