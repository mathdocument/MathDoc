import shlex
from pathlib import Path
from uuid import uuid4
from dataclasses import dataclass, field

from .codeblock import CodeBlock


@dataclass(slots=True)
class MdocNode:
    """
    Core unit for a knowledge card file.
    - `fnode`: globally unique id for this card.
    - `title`: human readable title.
    - `path`: file path of the card.
    - `blocks`: one or more code/text blocks (natural language, LaTeX, C++, etc).
    - `depens`: other MdocNode ids this node depends on.
    """

    path: Path
    title: str
    fnode: str = field(default_factory=lambda: str(uuid4()), init=False)
    depens: list[str] = field(default_factory=list, init=False)
    blocks: list[CodeBlock] = field(default_factory=list, init=False)

    @dataclass(slots=True)
    class EvalBlockResult:
        block: CodeBlock
        result: CodeBlock.CompileResult

    @classmethod
    def create(cls, folder: str = ".", title: str = "Untitled") -> "MdocNode":
        """Create a new node with an auto-generated unique id."""
        folder_path = Path(folder).resolve()
        node = cls(path=folder_path, title=title)
        node.path = folder_path / f"{node.fnode}.mdoc"
        return node

    def add_dependency(self, dep_fnode: str) -> None:
        """Register a dependency by MdocNode id."""
        if dep_fnode not in self.depens:
            self.depens.append(dep_fnode)

    def rmv_dependency(self, dep_fnode: str) -> None:
        """Unregister a dependency by MdocNode id."""
        if dep_fnode in self.depens:
            self.depens.remove(dep_fnode)

    def eval_blocks(self, *, mdoc_root: Path) -> list["MdocNode.EvalBlockResult"]:
        if mdoc_root is None:
            raise ValueError("mdoc_root is required for eval_blocks")
        results: list[MdocNode.EvalBlockResult] = []
        for block in self.blocks:
            result = block.compile(mdoc_root=mdoc_root, fnode=self.fnode)
            results.append(MdocNode.EvalBlockResult(
                block=block, result=result))
        return results

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
        for index, raw_line in enumerate(lines, start=1):
            line = raw_line.strip()

            if not line:
                continue
            # fnode and title must exist and be unique
            if line.startswith("@fnode:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@fnode' after {status} block in {self.path}")
                if fnode:
                    raise ValueError(
                        f"line {index}: Duplicate '@fnode' in {self.path}")
                fnode = line.split(":", 1)[1].strip()
                if not fnode:
                    raise ValueError(
                        f"line {index}: '@fnode' must be non-empty in {self.path}")
                continue
            if line.startswith("@title:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@title' after {status} block in {self.path}")
                if title:
                    raise ValueError(
                        f"line {index}: Duplicate '@title' in {self.path}")
                title = line.split(":", 1)[1].strip()
                if not title:
                    raise ValueError(
                        f"line {index}: '@title' must be non-empty in {self.path}")
                continue

            # depens is optional but must be unique and non-empty if exists
            if line.startswith("@dep:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@dep' after {status} block in {self.path}")
                if depens:
                    raise ValueError(
                        f"line {index}: Duplicate '@dep' in {self.path}")
                status = "@dep"
                continue
            # src is optional and can have multiple blocks
            if line.startswith("@src:"):
                if status:
                    raise ValueError(
                        f"line {index}: unexpected '@src' after {status} block in {self.path}")
                codetype, metadata = self._parse_src_header(line)
                for block in blocks:
                    if codetype == block.codetype:
                        raise ValueError(
                            f"line {index}: Duplicate '@src' codetype '{codetype}' in {self.path}")
                blocks.append(CodeBlock(codetype=codetype,
                              content="", metadata=metadata))
                status = "@src"
                continue

            # get dep content
            if status == "@dep":
                if line == "@end":
                    if not depens:
                        raise ValueError(
                            f"line {index}: '@dep' block must be non-empty in {self.path}")
                    status = ""
                    continue
                dep = line.split(":", 1)[0].strip()
                if not dep:
                    raise ValueError(
                        f"line {index}: Invalid dependency format in {self.path}: '{line}'")
                if dep in depens:
                    raise ValueError(
                        f"line {index}: Duplicate dependency '{dep}' in {self.path}")
                depens.append(dep)
                continue
            # get src content
            if status == "@src":
                if line == "@end":
                    status = ""
                    continue
                blocks[-1].content += raw_line.rstrip() + "\n"
                continue
            raise ValueError(
                f"line {index}: Unrecognized line in {self.path}: '{line}'")

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

        def _quote(value: str) -> str:
            escaped = value.replace("\\", "\\\\").replace('"', '\\"')
            return f"\"{escaped}\""

        meta_tokens = [
            f"{key}={_quote(value)}" for key, value in metadata.items()]
        return f"@src: {codetype} " + " ".join(meta_tokens)
