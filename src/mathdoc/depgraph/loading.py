from collections import deque
from pathlib import Path

from .issues import clear_broken_issue
from .issues import make_invalid_issue
from .issues import mark_missing
from .issues import record_invalid_issue
from .state import GraphState
from ..indcache import IndCache
from ..mdocnode import MdocNode
from ..utils import find_nested_mdcroot, iter_workspace_mdoc_files, to_rel_path


def create_root_node(
    *,
    mdcroot: Path,
    file_path: str = ".",
    title: str = "Untitled",
    fnode: str | None = None,
) -> tuple[MdocNode, str]:
    root_path = Path(mdcroot).resolve()
    node = MdocNode.create_at_path(
        mdcroot=root_path,
        path=root_path,
        title=title,
    )
    if fnode is not None:
        node.fnode = fnode
    node.path = _resolve_new_node_path(
        mdcroot=root_path,
        raw_target=file_path,
        fnode=node.fnode,
    )
    node.save()
    return node, to_rel_path(root_path, node.path)


def _resolve_new_node_path(
    *,
    mdcroot: Path,
    raw_target: str,
    fnode: str,
) -> Path:
    root_path = Path(mdcroot).resolve()
    target_text = raw_target.strip()

    if target_text in ("", "."):
        return root_path / f"{fnode}.mdoc"

    target_rel = Path(target_text)
    if target_rel.is_absolute():
        raise ValueError("target path must be relative to the mdoc root")

    target_path = (root_path / target_rel).resolve()
    try:
        target_path.relative_to(root_path)
    except ValueError as exc:
        raise ValueError(f"target path must be under mdoc root {root_path}") from exc

    final_path = target_path.with_name(target_path.name + ".mdoc")
    nested_root = find_nested_mdcroot(root_path, final_path.parent)
    if nested_root is not None:
        raise ValueError(f"target path is inside nested mdoc root: {nested_root}")
    if final_path.exists():
        raise FileExistsError(f"mdoc file already exists: {final_path}")

    return final_path


def load_root_node_from_ref(
    *,
    cache: IndCache,
    ref: str,
    cwd: Path | None = None,
) -> tuple[MdocNode, str]:
    base_cwd = (cwd or Path.cwd()).resolve()

    for attempt in range(2):
        try:
            _, _, src_path = cache.resolve_ref(ref, cwd=base_cwd)
        except ValueError:
            if attempt == 0:
                cache.refresh_all()
                continue
            raise

        node = MdocNode(mdcroot=cache.root, path=src_path, title="")
        try:
            node.load()
        except FileNotFoundError:
            if attempt == 0:
                cache.refresh_all()
                continue
            raise

        return node, to_rel_path(cache.root, src_path)

    raise ValueError(f"failed to resolve mdoc reference: {ref}")


class GraphLoader:
    def __init__(
        self,
        *,
        mdcroot: Path,
        cache: IndCache,
        state: GraphState,
    ) -> None:
        self.mdcroot = Path(mdcroot).resolve()
        self.cache = cache
        self.state = state

    def ensure_ready(self) -> None:
        mdc_dir = self.mdcroot / ".mdc"
        if not mdc_dir.is_dir():
            raise ValueError(f"invalid mdoc root (missing .mdc): {mdc_dir}")
        self.cache.bootstrap_if_needed()

    def ensure_node_loaded(self, fnode: str) -> MdocNode:
        node = self.state.nodes_by_fnode.get(fnode)
        if node is not None:
            return node
        self.ensure_ready()
        node = self.load_node(
            fnode,
            tolerate_missing=False,
            tolerate_invalid=False,
        )
        if node is None:
            raise ValueError(f"no mdoc matched reference: {fnode}")
        self.state.nodes_by_fnode[fnode] = node
        self.state.dep_graph.setdefault(fnode, [])
        return node

    def expand_from_root(self, *, root_fnode: str, depth: int) -> None:
        self.ensure_ready()
        root = self.ensure_node_loaded(root_fnode)

        seen: set[str] = {root.fnode}
        queue: deque[tuple[MdocNode, int]] = deque([(root, 0)])

        while queue:
            node, node_depth = queue.popleft()
            self.state.dep_graph[node.fnode] = []

            for dep_fnode in self._dedupe_keep_order(node.depens):
                dep_node = self.state.nodes_by_fnode.get(dep_fnode)
                if dep_node is None:
                    if depth != -1 and node_depth >= depth:
                        continue
                    dep_node = self.load_node(
                        dep_fnode,
                        tolerate_missing=True,
                        tolerate_invalid=True,
                    )
                    if dep_node is not None:
                        self.state.nodes_by_fnode[dep_fnode] = dep_node

                self.state.dep_graph[node.fnode].append(dep_fnode)
                self.state.dep_graph.setdefault(dep_fnode, [])
                if dep_node is None:
                    continue

                if dep_fnode in seen:
                    continue
                seen.add(dep_fnode)
                queue.append((dep_node, node_depth + 1))

        for fnode in self.state.nodes_by_fnode:
            self.state.dep_graph.setdefault(fnode, [])

    def scan_all(self) -> None:
        self.ensure_ready()
        self.state.reset_graph()

        for file_path in self._iter_mdoc_files():
            self.state.scanned_file_count += 1
            try:
                node = self._load_node_from_path(file_path)
            except ValueError as exc:
                head_fnode, _ = self._read_mdoc_head(file_path)
                issue = make_invalid_issue(
                    mdcroot=self.mdcroot,
                    path=file_path,
                    error=str(exc),
                    fnode=head_fnode or "<unknown>",
                )
                record_invalid_issue(self.state, issue)
                continue
            self.state.nodes_by_fnode[node.fnode] = node

        for node in list(self.state.nodes_by_fnode.values()):
            self.state.dep_graph[node.fnode] = []
            for dep_fnode in self._dedupe_keep_order(node.depens):
                if (
                    dep_fnode not in self.state.nodes_by_fnode
                    and dep_fnode not in self.state.invalid_fnodes
                ):
                    dep_node = self.load_node(
                        dep_fnode,
                        tolerate_missing=True,
                        tolerate_invalid=True,
                    )
                    if dep_node is not None:
                        self.state.nodes_by_fnode[dep_fnode] = dep_node
                self.state.dep_graph[node.fnode].append(dep_fnode)
                self.state.dep_graph.setdefault(dep_fnode, [])

        for fnode in self.state.nodes_by_fnode:
            self.state.dep_graph.setdefault(fnode, [])

    def load_node(
        self,
        fnode: str,
        *,
        tolerate_missing: bool,
        tolerate_invalid: bool,
    ) -> MdocNode | None:
        for attempt in range(2):
            path = self._resolve_fnode_path(
                fnode,
                tolerate_missing=tolerate_missing,
            )
            if path is None:
                if attempt == 0:
                    self.cache.refresh_all()
                    continue
                mark_missing(self.state, fnode)
                return None

            try:
                node = self._load_node_from_path(path)
            except FileNotFoundError:
                if attempt == 0:
                    self.cache.refresh_all()
                    continue
                if tolerate_missing:
                    mark_missing(self.state, fnode)
                    return None
                raise
            except ValueError as exc:
                if tolerate_invalid:
                    issue = make_invalid_issue(
                        mdcroot=self.mdcroot,
                        path=path,
                        error=str(exc),
                        fnode=fnode,
                    )
                    record_invalid_issue(self.state, issue)
                    return None
                raise

            clear_broken_issue(self.state, fnode)
            return node

        if tolerate_missing:
            mark_missing(self.state, fnode)
            return None
        raise ValueError(f"no mdoc matched reference: {fnode}")

    def _resolve_fnode_path(
        self,
        fnode: str,
        *,
        tolerate_missing: bool,
    ) -> Path | None:
        try:
            _, _, path = self.cache.resolve_ref(fnode, cwd=self.cache.root)
            return path
        except ValueError as exc:
            if tolerate_missing and str(exc).startswith("no mdoc matched reference:"):
                return None
            raise

    def _load_node_from_path(self, path: Path) -> MdocNode:
        node = MdocNode(mdcroot=self.mdcroot, path=path, title="")
        node.load()
        return node

    def _iter_mdoc_files(self) -> list[Path]:
        return list(iter_workspace_mdoc_files(self.mdcroot))

    @staticmethod
    def _read_mdoc_head(path: Path) -> tuple[str | None, str | None]:
        fnode = ""
        title = ""
        try:
            with path.open("r", encoding="utf-8") as handle:
                for raw_line in handle:
                    line = raw_line.strip()
                    lower = line.lower()
                    if lower.startswith("@fnode:"):
                        fnode = line.split(":", 1)[1].strip()
                    elif lower.startswith("@title:"):
                        title = line.split(":", 1)[1].strip()
                    if fnode and title:
                        break
        except OSError:
            return None, None
        return (fnode or None, title or None)

    @staticmethod
    def _dedupe_keep_order(items: list[str]) -> list[str]:
        out: list[str] = []
        seen: set[str] = set()
        for item in items:
            if item in seen:
                continue
            seen.add(item)
            out.append(item)
        return out
