---
title: Dependency graph
---

Each node declares its direct dependencies by UUID; TerminusDB stores them as
typed links. Every write is validated before it commits: missing targets and
cycles (including self links) are rejected. Adding an edge that already exists
changes nothing.

The API keeps an in-memory projection of each loaded branch with topological
depths and reverse edges. Each request looks up the branch's current commit; the
projection is updated in place after the service's own writes and reloaded
completely after a commit made elsewhere (another process, or a writeback by the
worker). Graph queries never read or compile source.

Edges carry meaning for proofs: when a Lean node is submitted, its Lean
dependencies decide what is registered and what becomes a premise. Direct theorem
dependencies are premises; definition dependencies (and theirs) form the
definition closure; nodes without a Lean block are informal and are not followed.
Lean `import` lines are ignored. See
[Proofs with LeanGround](../../reference/proofs/#node-conversion).
