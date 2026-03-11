from dataclasses import dataclass, field
from pathlib import Path
import shlex
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

    @classmethod
    def create(cls, pstr: str = "", title: str = "") -> "FileNode":
        """Create a new node with an auto-generated unique id."""
        fnode = str(uuid4())
        if not pstr:
            pstr = f"data/{fnode}.mdoc"
        return cls(path=Path(pstr), fnode=fnode, title=title)

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

        status = ""
        for index, raw_line in enumerate(lines):
            line = raw_line.strip()

            if not line:
                continue
            # fnode and title must exist and be unique
            if line.startswith("@fnode:"):
                if status:
                    raise ValueError(
                        f"line {index+1}: unexpected '@fnode' after {status} block in {self.path}")
                if fnode:
                    raise ValueError(
                        f"line {index+1}: Duplicate '@fnode' in {self.path}")
                fnode = line.split(":", 1)[1].strip()
                if not fnode:
                    raise ValueError(
                        f"line {index+1}: '@fnode' must be non-empty in {self.path}")
                continue
            if line.startswith("@title:"):
                if status:
                    raise ValueError(
                        f"line {index+1}: unexpected '@title' after {status} block in {self.path}")
                if title:
                    raise ValueError(
                        f"line {index+1}: Duplicate '@title' in {self.path}")
                title = line.split(":", 1)[1].strip()
                if not title:
                    raise ValueError(
                        f"line {index+1}: '@title' must be non-empty in {self.path}")
                continue

            # depens is optional but must be unique and non-empty if exists
            if line.startswith("@dep:"):
                if status:
                    raise ValueError(
                        f"line {index+1}: unexpected '@dep' after {status} block in {self.path}")
                if depens:
                    raise ValueError(
                        f"line {index+1}: Duplicate '@dep' in {self.path}")
                status = "@dep"
                continue
            # src is optional and can have multiple blocks
            if line.startswith("@src:"):
                if status:
                    raise ValueError(
                        f"line {index+1}: unexpected '@src' after {status} block in {self.path}")
                codetype, metadata = self._parse_src_header(line)
                blocks.append(CodeBlock(codetype=codetype,
                              content="", metadata=metadata))
                status = "@src"
                continue

            # get dep content
            if status == "@dep":
                if line == "@end":
                    if not depens:
                        raise ValueError(
                            f"line {index+1}: '@dep' block must be non-empty in {self.path}")
                    status = ""
                    continue
                dep = line.split(":", 1)[0].strip()
                if not dep:
                    raise ValueError(
                        f"line {index+1}: Invalid dependency format in {self.path}: '{line}'")
                depens.append(dep)
                continue
            # get src content
            if status == "@src":
                if line == "@end":
                    status = ""
                    continue
                blocks[-1].content += raw_line + "\n"
                continue
            raise ValueError(
                f"line {index+1}: Unrecognized line in {self.path}: '{line}'")

        if status:
            raise ValueError(
                f"Unclosed block '{status}' in {self.path}")
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
            output_lines.append(self._format_src_header(
                block.codetype, block.metadata))
            if block.content:
                output_lines.extend(block.content.splitlines())
            output_lines.append("@end")
            output_lines.append("")

        payload = "\n".join(output_lines).rstrip() + "\n"
        self.path.write_text(payload, encoding="utf-8")

    @staticmethod
    def _parse_src_header(line: str) -> tuple[str, dict[str, str]]:
        """
        Parse a src header line.

        Example:
        - @src: latex preamble="path"
        - @src: lean version=4.2
        """
        payload = line.split(":", 1)[1].strip()
        if not payload:
            raise ValueError("Missing codetype after '@src:'.")

        tokens = shlex.split(payload)
        if not tokens:
            raise ValueError("Invalid '@src' header.")

        codetype = tokens[0]
        metadata: dict[str, str] = {}
        for token in tokens[1:]:
            if "=" not in token:
                raise ValueError(f"Invalid src metadata token: '{token}'")
            key, value = token.split("=", 1)
            key = key.strip()
            if not key:
                raise ValueError(f"Invalid src metadata token: '{token}'")
            metadata[key] = value

        return codetype, metadata

    @staticmethod
    def _format_src_header(codetype: str, metadata: dict[str, str]) -> str:
        """Format a src header line for saving."""

        if not metadata:
            return f"@src: {codetype}"

        meta_tokens = [f"{key}=\"{value}\"" for key, value in metadata.items()]
        return f"@src: {codetype} " + " ".join(meta_tokens)
