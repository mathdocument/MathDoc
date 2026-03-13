from .base import BlockCompiler
from .comp_natl import CompilerNatl
from .comp_py import CompilerPy
from .comp_latex import CompilerLatex


class CompilerRegistry:
    def __init__(self, compilers: list[BlockCompiler]) -> None:
        self._compilers = {
            compiler.srctype.casefold(): compiler for compiler in compilers
        }

    def resolve(self, srctype: str) -> BlockCompiler | None:
        return self._compilers.get(srctype.casefold())


COMPILER_REGISTRY = CompilerRegistry(
    [
        CompilerNatl(),
        CompilerPy(),
        CompilerLatex(),
    ]
)
