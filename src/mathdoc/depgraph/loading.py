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
    cache.bootstrap_if_needed()

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
        duplicate_paths = cache.duplicate_fnode_paths(node.fnode)
        if len(duplicate_paths) > 1:
            raise ValueError(
                _duplicate_fnode_error(
                    mdcroot=cache.root,
                    fnode=node.fnode,
                    paths=duplicate_paths,
                )
            )

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
                if depth != -1 and node_depth >= depth and dep_fnode not in seen:
                    continue
                dep_node = self.state.nodes_by_fnode.get(dep_fnode)
                if dep_node is None:
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
        loaded_nodes: list[MdocNode] = []

        for file_path in self._iter_mdoc_files():
            self.state.scanned_file_count += 1
            try:
                node = self._load_node_from_path(file_path)
            except ValueError as exc:
                head = self.cache.read_mdoc_head(file_path)
                head_fnode = None if head is None else head[0]
                issue = make_invalid_issue(
                    mdcroot=self.mdcroot,
                    path=file_path,
                    error=str(exc),
                    fnode=head_fnode or "<unknown>",
                )
                record_invalid_issue(self.state, issue)
                continue
            loaded_nodes.append(node)

        nodes_by_fnode: dict[str, list[MdocNode]] = {}
        for node in loaded_nodes:
            nodes_by_fnode.setdefault(node.fnode, []).append(node)

        for fnode, grouped_nodes in sorted(nodes_by_fnode.items()):
            if len(grouped_nodes) == 1:
                self.state.nodes_by_fnode[fnode] = grouped_nodes[0]
                continue
            self._record_duplicate_fnode(
                fnode=fnode,
                paths=[node.path for node in grouped_nodes],
            )

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
            try:
                path = self._resolve_fnode_path(
                    fnode,
                    tolerate_missing=tolerate_missing,
                )
            except ValueError:
                duplicate_paths = self.cache.duplicate_fnode_paths(fnode)
                if tolerate_invalid and len(duplicate_paths) > 1:
                    self._record_duplicate_fnode(fnode=fnode, paths=duplicate_paths)
                    return None
                raise
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
            duplicate_paths = self.cache.duplicate_fnode_paths(node.fnode)
            if len(duplicate_paths) > 1:
                if tolerate_invalid:
                    self._record_duplicate_fnode(
                        fnode=node.fnode,
                        paths=duplicate_paths,
                    )
                    return None
                raise ValueError(
                    _duplicate_fnode_error(
                        mdcroot=self.mdcroot,
                        fnode=node.fnode,
                        paths=duplicate_paths,
                    )
                )

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

    def _record_duplicate_fnode(self, *, fnode: str, paths: list[Path]) -> None:
        sorted_paths = sorted(path.resolve() for path in paths)
        error = _duplicate_fnode_error(
            mdcroot=self.mdcroot,
            fnode=fnode,
            paths=sorted_paths,
        )
        for path in sorted_paths:
            issue = make_invalid_issue(
                mdcroot=self.mdcroot,
                path=path,
                error=error,
                fnode=fnode,
            )
            record_invalid_issue(self.state, issue)
        self.state.broken_issues[fnode] = make_invalid_issue(
            mdcroot=self.mdcroot,
            path=sorted_paths[0],
            error=error,
            fnode=fnode,
        )

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


def _duplicate_fnode_error(
    *,
    mdcroot: Path,
    fnode: str,
    paths: list[Path],
) -> str:
    rel_paths = ", ".join(to_rel_path(mdcroot, path) for path in paths)
    return f"duplicate fnode '{fnode}' found in: {rel_paths}"
