---
title: Dependency Graph
description: Model prerequisites, traverse knowledge, and diagnose graph integrity.
---

Every `@dep` entry creates a directed edge from a node to one of its immediate
prerequisites. This orientation makes a theorem point toward the definitions and
results required to understand or prove it.

## Direct and transitive views

Use the graph from either direction:

```bash
# What does this theorem require?
mdc dep show notes/theorem

# Which nodes depend on this lemma?
mdc dep refs notes/background

# What terminal prerequisites are reachable?
mdc dep leaf notes/theorem
```

`dep show` and `dep refs` default to one hop. Pass `-d -1` for unlimited traversal or
a positive depth to set an exact hop limit.

## Roots and leaves

A root has no incoming valid edge: no other indexed node currently depends on it.
`mdc graph roots` orders roots by descending topological depth, then weak-component
size, path, and fnode. This tends to surface deep entry points before isolated notes.

A reachable leaf has no further outgoing dependency. `mdc dep leaf` is useful for
finding the foundational material under a result.

## Integrity rules

Dependency additions require valid, uniquely identified targets and reject:

- ambiguous or missing targets
- invalid duplicate identities
- any edge that would create a cycle

Self-dependencies are prevented: `mdc dep add --target` and the Web API reject them,
while the batch `DepGraph` API reports them in `skipped_self` without writing an edge.
An already-present direct dependency is an idempotent success in the CLI. Dependency
removal checks the current direct dependency tokens before resolving live nodes, so a
dangling dependency can still be removed by its exact token or an unambiguous prefix.

Direct file edits bypass those checks. Run:

```bash
mdc graph check
```

The report includes discovered document count, valid-source edge count, structurally
invalid files, duplicate fnodes, missing targets, and one representative cycle from
each cyclic strongly connected component.

An edge from an invalid or duplicate source is excluded from valid graph reads. An edge
to a missing target still contributes to the valid source's out-degree and receives a
missing-target diagnostic.

## Interactive graph views

`mdc graph tui` provides keyboard-driven navigation, search, source preview, editor
launch, and dependency mutation in the terminal. `mdc serve` adds a three-column
browser and a deterministic depth-layered full graph while preserving the same index
and mutation semantics.
