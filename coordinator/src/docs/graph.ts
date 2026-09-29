// Graph algorithms (src/core/algorithms.rs) and the branch snapshot (Snapshot in
// src/store.rs). The snapshot is a disposable projection of one TerminusDB version.
import { DocError, byteOrder, moduleFile, parseUuid } from "./names.js";
import {
  type LatexProject,
  type LeanProject,
  type Node,
  latexProjectKey,
  validateModules,
  validateNode,
} from "./model.js";

type Graph = Map<string, string[]>;

/** Leaves are 0; each acyclic parent is one more than its deepest dependency. */
export function topoDepths(graph: Graph): Map<string, number> {
  const reverse = new Map<string, string[]>();
  const remaining = new Map<string, number>();
  const depths = new Map<string, number>();
  for (const id of graph.keys()) {
    reverse.set(id, []);
    depths.set(id, 0);
  }
  for (const [source, targets] of graph) {
    let count = 0;
    for (const target of targets)
      if (graph.has(target)) {
        reverse.get(target)!.push(source);
        count++;
      }
    remaining.set(source, count);
  }
  const queue = [...remaining].filter(([, c]) => c === 0).map(([id]) => id);
  for (let i = 0; i < queue.length; i++) {
    const id = queue[i];
    const depth = depths.get(id)!;
    for (const parent of reverse.get(id) ?? []) {
      depths.set(parent, Math.max(depths.get(parent)!, depth + 1));
      const count = Math.max(0, remaining.get(parent)! - 1);
      remaining.set(parent, count);
      if (count === 0) queue.push(parent);
    }
  }
  return depths;
}

/** Undirected component size of each member; edges leaving `members` are ignored. */
export function weakComponentSizes(
  graph: Graph,
  members: Set<string>,
): Map<string, number> {
  const adjacency = new Map<string, Set<string>>();
  for (const m of members) adjacency.set(m, new Set());
  for (const [source, targets] of graph) {
    if (!members.has(source)) continue;
    for (const target of targets) {
      if (!members.has(target)) continue;
      adjacency.get(source)!.add(target);
      adjacency.get(target)!.add(source);
    }
  }
  const sizes = new Map<string, number>();
  const seen = new Set<string>();
  for (const start of members) {
    if (seen.has(start)) continue;
    const component: string[] = [];
    const queue = [start];
    for (let i = 0; i < queue.length; i++) {
      const node = queue[i];
      if (seen.has(node)) continue;
      seen.add(node);
      component.push(node);
      for (const n of adjacency.get(node) ?? [])
        if (!seen.has(n)) queue.push(n);
    }
    for (const node of component) sizes.set(node, component.length);
  }
  return sizes;
}

/** True when some strongly connected component has more than one node. */
export function hasCycle(graph: Graph): boolean {
  const state = new Map<string, 1 | 2>();
  for (const start of graph.keys()) {
    if (state.has(start)) continue;
    const stack: [string, number][] = [[start, 0]];
    state.set(start, 1);
    while (stack.length) {
      const top = stack[stack.length - 1];
      const targets = graph.get(top[0]) ?? [];
      if (top[1] < targets.length) {
        const next = targets[top[1]++];
        if (!graph.has(next)) continue;
        const s = state.get(next);
        if (s === 1) return true;
        if (s === undefined) {
          state.set(next, 1);
          stack.push([next, 0]);
        }
      } else {
        state.set(top[0], 2);
        stack.pop();
      }
    }
  }
  return false;
}

export class Snapshot {
  nodes = new Map<string, Node>();
  depths = new Map<string, number>();
  referrers = new Map<string, string[]>();
  private sorted: string[] | null = null;
  latexProjectKey: string;

  constructor(
    public version: string,
    nodes: Node[],
    public project: LeanProject,
    public latexProject: LatexProject,
  ) {
    for (const n of nodes) this.nodes.set(n.fnode, n);
    this.latexProjectKey = latexProjectKey(latexProject);
    this.recomputeGraph();
  }

  /** BTreeMap iteration order of the legacy snapshot. */
  ids(): string[] {
    return (this.sorted ??= [...this.nodes.keys()].sort(byteOrder));
  }
  values(): Node[] {
    return this.ids().map((id) => this.nodes.get(id)!);
  }
  graph(): Graph {
    return new Map(this.ids().map((id) => [id, this.nodes.get(id)!.depens]));
  }

  recomputeGraph() {
    this.sorted = null;
    this.depths = topoDepths(this.graph());
    this.referrers = new Map(this.ids().map((id) => [id, []]));
    for (const node of this.values())
      for (const dep of node.depens) {
        if (!this.referrers.has(dep)) this.referrers.set(dep, []);
        this.referrers.get(dep)!.push(node.fnode);
      }
  }

  resolve(reference: string): Node {
    const id = parseUuid(reference);
    if (id !== null) {
      const node = this.nodes.get(id);
      if (!node) throw new DocError("node not found", 404);
      return node;
    }
    const matches = this.values().filter((n) => n.title === reference);
    if (matches.length === 0)
      throw new DocError(
        "node not found; use an exact name or complete UUID",
        404,
      );
    if (matches.length > 1)
      throw new DocError("name is ambiguous; use the complete UUID");
    return matches[0];
  }

  validateChanges(changes: Node[]) {
    validateModules(this.project, changes);
    const ids = new Set<string>();
    for (const node of changes) {
      validateNode(node);
      if (ids.has(node.fnode))
        throw new DocError("duplicate UUID in transaction");
      ids.add(node.fnode);
    }
    const same = (a: string[], b: string[]) =>
      a.length === b.length && a.every((x, i) => x === b[i]);
    if (
      changes.every((n) => {
        const old = this.nodes.get(n.fnode);
        return (
          old !== undefined &&
          same(old.depens, n.depens) &&
          old.module === n.module
        );
      })
    )
      return;
    const graph = this.graph();
    const modules = new Map<string, string>();
    for (const n of this.values())
      if (!ids.has(n.fnode)) modules.set(moduleFile(n.module, "lean"), n.fnode);
    for (const node of changes) {
      const file = moduleFile(node.module, "lean");
      const owner = modules.get(file);
      modules.set(file, node.fnode);
      if (owner !== undefined && owner !== node.fnode)
        throw new DocError(
          "Lean module file is already assigned to another node",
        );
      graph.set(node.fnode, node.depens);
    }
    for (const node of changes)
      for (const dep of node.depens)
        if (!graph.has(dep))
          throw new DocError(`dependency node does not exist: ${dep}`);
    if (hasCycle(graph)) throw new DocError("dependency cycle rejected");
  }

  apply(changes: Node[], version: string) {
    const same = (a: string[], b: string[]) =>
      a.length === b.length && a.every((x, i) => x === b[i]);
    const graphChanged = changes.some((n) => {
      const old = this.nodes.get(n.fnode);
      return (
        old === undefined ||
        !same(old.depens, n.depens) ||
        old.module !== n.module
      );
    });
    for (const node of changes) this.nodes.set(node.fnode, node);
    this.version = version;
    if (graphChanged) this.recomputeGraph();
  }

  remove(id: string, version: string) {
    for (const parent of this.referrers.get(id) ?? []) {
      const node = this.nodes.get(parent)!;
      this.nodes.set(parent, {
        ...node,
        depens: node.depens.filter((d) => d !== id),
      });
    }
    this.nodes.delete(id);
    this.version = version;
    this.recomputeGraph();
  }

  setLatexProject(project: LatexProject, version: string) {
    this.latexProject = project;
    this.latexProjectKey = latexProjectKey(project);
    this.version = version;
  }

  report() {
    // All commits are validated before publication; this reads the validated projection.
    let edges = 0;
    for (const n of this.nodes.values()) edges += n.depens.length;
    return {
      nodes: this.nodes.size,
      edges,
    };
  }
}
