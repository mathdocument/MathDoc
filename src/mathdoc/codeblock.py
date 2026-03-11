from dataclasses import dataclass, field
from typing import Any


@dataclass(slots=True)
class CodeBlock:
    """A typed content block in a knowledge card."""

    codetype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)

    def compile(self) -> Any:
        """
        TODO: Compile the block and display results
        """
        raise NotImplementedError(
            f"Compilation not implemented for codetype '{self.codetype}'.")
