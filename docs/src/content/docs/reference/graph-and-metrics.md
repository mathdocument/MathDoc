---
title: Graph, search and metrics
---

```sh
mdc search theorem -n 20 -p myproject/main
mdc graph check -p myproject/main -m
mdc graph roots -p myproject/main
mdc graph full -p myproject/main
mdc metric ior 'Theorem' -p myproject/main
```

`search QUERY` matches titles and UUIDs case-insensitively. It returns node
summaries, with `-n/--max-results` in 0–200 (default 200). It does not search block
contents or fuzzy-match names, and it does not paginate beyond that limit.

Graph operations read the service's validated projection. `check` returns node
and edge counts and missing/invalid/cycle lists; it does not compile Lean.
`full` returns node summaries, each with its cached `lean` status, and index-pair
edges for visualization. It does not compile Lean to obtain colors. `roots`
returns unreferenced nodes with topological depth and weak component size.

`metric ior` is a single-node operation. Its result includes UUID, in-degree,
out-degree and `ior = ln((in_degree + 1) / (out_degree + 1))`. Reverse queries use
the resident reverse adjacency map. Every request checks the database revision;
external commits can trigger a projection reload. There is no file scan.

For repeatable graph benchmarks and their scope, see
[Performance measurements](../../development/performance/).
