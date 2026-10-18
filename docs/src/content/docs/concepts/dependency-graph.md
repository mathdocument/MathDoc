---
title: Dependency graph
---

Each node declares its direct dependencies by UUID; TerminusDB stores them as
typed links. Every write is validated before it commits: missing targets,
duplicate edges and cycles (including self links) are rejected. Adding an edge
that already exists changes nothing. Deleting a node also removes, in the same
commit, every edge that points to it.

A node's **depth** is 0 when it has no dependencies and otherwise one more than
the depth of its deepest dependency. **Roots** are nodes that no other node
depends on; `mdc graph roots` lists them deepest first with their depth and the
size of their weakly connected component, and the editor opens a branch at its
deepest root. `metric ior` reports in- and out-degree and
`ln((in + 1) / (out + 1))`. See [Graph, search and
metrics](../../reference/graph-and-metrics/) and [Dependency
commands](../../reference/dependency-commands/).

The API keeps an in-memory projection of each loaded branch with depths and
reverse edges. Each request looks up the branch's current commit; the projection
is updated in place after the service's own writes and reloaded completely after
a commit made elsewhere (another process, or a writeback by the worker).
Graph queries never read or compile source.

Edges carry meaning for proofs: when a branch is converted for LeanGround, its
Lean dependencies decide what is registered and what becomes a premise. Direct
theorem dependencies are premises; definition dependencies (and theirs) form the
definition closure; nodes without a Lean block are informal and are not
followed. Lean `import` lines are ignored. See
[Proofs with LeanGround](../../reference/proofs/#node-conversion).

LaTeX previews use the same edges: a LaTeX block can reference labels of its
node's direct dependencies only ([LaTeX](../latex/#references)).
