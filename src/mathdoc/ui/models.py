from dataclasses import dataclass, field


@dataclass(slots=True, frozen=True)
class NodeRef:
    fnode: str
    title: str
    rel_path: str
    depth: int | None = None
    broken: bool = False


@dataclass(slots=True, frozen=True)
class IssueView:
    ref: NodeRef
    error: str


@dataclass(slots=True, frozen=True)
class ChainView:
    anchor_label: str
    anchor: NodeRef
    count_label: str
    items: tuple[NodeRef, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class MissingReferrerView:
    target: NodeRef
    referrers: tuple[NodeRef, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class BrokenDependencySummary:
    missing: int = 0
    invalid: int = 0

    @property
    def total(self) -> int:
        return self.missing + self.invalid


@dataclass(slots=True, frozen=True)
class DepAddView:
    source: NodeRef
    added: tuple[NodeRef, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class DepRmView:
    source: NodeRef
    removed: tuple[NodeRef, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class CycleView:
    nodes: tuple[NodeRef, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class GraphCheckView:
    nodes: int
    edges: int
    missing: tuple[IssueView, ...] = field(default_factory=tuple)
    invalid: tuple[IssueView, ...] = field(default_factory=tuple)
    cycles: tuple[CycleView, ...] = field(default_factory=tuple)


@dataclass(slots=True, frozen=True)
class EvalBlockView:
    index: int
    total: int
    srctype: str
    ok: bool
    rtcode: int
    stdout: str
    stderr: str


@dataclass(slots=True, frozen=True)
class EvalReportView:
    blocks: tuple[EvalBlockView, ...] = field(default_factory=tuple)
    failed: int = 0
