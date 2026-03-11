from dataclasses import dataclass, field
from email.policy import default
from pathlib import Path
from typing import Any
from uuid import uuid4


@dataclass(slots=True)
class CodeBlock:
    """A typed content block in a knowledge card."""

    codetype: str
    content: str
    metadata: dict[str, str] = field(default_factory=dict)


@dataclass(slots=True)
class FileNode:
    """
    Core unit for a knowledge card file.
    - `fnode`: globally unique id for this card.
    - `title`: human readable title.
    - `path`: file path of the card.
    - `blocks`: one or more code/text blocks (natural language, LaTeX, C++, etc).
    - `depens`: other FileNode ids this node depends on.
    """

    path: Path
    fnode: str = field(default_factory=str)
    title: str = field(default_factory=str)
    depens: list[str] = field(default_factory=list)
    blocks: list[CodeBlock] = field(default_factory=list)

    # @classmethod
    # def create(cls, path: Path) -> "FileNode":
    #     """Create a new node with an auto-generated unique id."""
    #     return cls(fnode=str(uuid4()), path=path)

    def add_block(self, codetype: str, content: str, *, metadata: dict[str, str]) -> CodeBlock:
        """Append a new content block and return it."""
        block = CodeBlock(
            codetype=codetype,
            content=content,
            metadata=metadata,
        )
        self.blocks.append(block)
        return block

    def rmv_block(self, index: int) -> None:
        """Remove a block by index."""
        del self.blocks[index]

    def add_dependency(self, dep_fnode: str) -> None:
        """Register a dependency by FileNode id."""
        if dep_fnode not in self.depens:
            self.depens.append(dep_fnode)

    def rmv_dependency(self, dep_fnode: str) -> None:
        """Unregister a dependency by FileNode id."""
        if dep_fnode in self.depens:
            self.depens.remove(dep_fnode)

    def to_dict(self) -> dict[str, Any]:
        """
        Convert this node into a serializable dict.

        TODO: include strict schema/versioning when file format is finalized.
        """
        return {
            "fnode": self.fnode,
            "title": self.title,
            "path": str(self.path),
            "depens": list(self.depens),
            "blocks": [
                {
                    "codetype": block.codetype,
                    "content": block.content,
                    "metadata": block.metadata,
                }
                for block in self.blocks
            ]
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FileNode":
        """
        Build an FileNode from dict data.

        TODO: add strict input validation and migration for old schema.
        """
        raw_path = data.get("path")
        if not raw_path:
            raise ValueError("FileNode.from_dict requires 'path'.")

        raw_id = data.get("fnode")
        node = cls(
            fnode=str(raw_id) if raw_id else str(uuid4()),
            title=str(data.get("title", "")),
            path=Path(str(raw_path)),
        )

        for block in data.get("blocks", []):
            node.add_block(
                codetype=str(block.get("codetype", "")),
                content=str(block.get("content", "")),
                metadata=dict(block.get("metadata", {})),
            )

        for dep in data.get("depens", []):
            node.add_dependency(str(dep))

        return node

    def load(self) -> None:
        """
        Load card content from file.
        """
        if not self.path.exists():
            raise FileNotFoundError(f"mdoc file not found: {self.path}")

        lines = self.path.read_text(encoding="utf-8").splitlines()
        fnode: str = ""
        title: str = ""
        depens: list[str] = []
        blocks: list[CodeBlock] = []

        index = 0
        while index < len(lines):
            line = lines[index].strip()
            index += 1

            if not line:
                continue
            # fnode and title must exist and be unique
            if line.startswith("@fnode:"):
                if fnode:
                    raise ValueError(f"Duplicate '@fnode' in {self.path}")
                fnode = line.split(":", 1)[1].strip()
                if not fnode:
                    raise ValueError(
                        f"'@fnode' must be non-empty in {self.path}")
                continue
            if line.startswith("@title:"):
                if title:
                    raise ValueError(f"Duplicate '@title' in {self.path}")
                title = line.split(":", 1)[1].strip()
                if not title:
                    raise ValueError(
                        f"'@title' must be non-empty in {self.path}")
                continue
            # depen is optional but must be unique and non-empty if exists
            if line.startswith("@dep:"):
                if depens:
                    raise ValueError(f"Duplicate '@dep' in {self.path}")
                while index < len(lines):
                    candidate = lines[index].strip()
                    index += 1
                    if candidate == "@end":
                        break
                    if not candidate:
                        raise ValueError(
                            f"Empty dependency entry in {self.path}")
                    dep = candidate.split(":", 1)[0].strip()
                    if not dep:
                        raise ValueError(
                            f"Invalid dependency format in {self.path}: '{candidate}'")
                    depens.append(dep)
                else:
                    raise ValueError(
                        f"Missing '@end' for '@dep' block in {self.path}")
                if not depens:
                    raise ValueError(
                        f"'@dep' block must be non-empty in {self.path}")
                continue
            # src is optional and can have multiple blocks
            if line.startswith("@src:"):
                codetype = line.split(":", 1)[1].strip()
                content_lines: list[str] = []

                while index < len(lines):
                    candidate = lines[index]
                    index += 1
                    if candidate.strip() == "@end":
                        break
                    content_lines.append(candidate)
                else:
                    raise ValueError(
                        f"Missing '@end' for block '{codetype}' in {self.path}")

                blocks.append(
                    CodeBlock(
                        codetype=codetype,
                        content="\n".join(content_lines).rstrip("\n"),
                    )
                )

        if not fnode:
            raise ValueError(
                f"'@fnode' must exist and be non-empty in {self.path}")
        if not title:
            raise ValueError(
                f"'@title' must exist and be non-empty in {self.path}")

        self.fnode = fnode
        self.title = title
        self.depens = depens
        self.blocks = blocks

    def save(self) -> None:
        """
        Save card content to file.
        """
        if not self.fnode:
            self.fnode = str(uuid4())

        self.path.parent.mkdir(parents=True, exist_ok=True)

        output_lines: list[str] = [
            f"@fnode: {self.fnode}",
            f"@title: {self.title}",
            "",
        ]

        if self.depens:
            output_lines.append("@dep:")
            output_lines.extend(self.depens)
            output_lines.append("@end")
            output_lines.append("")

        for block in self.blocks:
            output_lines.append(f"@src: {block.codetype}")
            if block.content:
                output_lines.extend(block.content.splitlines())
            output_lines.append("@end")
            output_lines.append("")

        payload = "\n".join(output_lines).rstrip() + "\n"
        self.path.write_text(payload, encoding="utf-8")
