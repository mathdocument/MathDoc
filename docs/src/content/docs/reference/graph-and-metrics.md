---
title: Graph, search and metrics
---

```sh
mdc search theorem -n 20 -p myproject/main
mdc graph check -p myproject/main
mdc graph roots -p myproject/main
mdc graph full -p myproject/main
mdc metric ior 'Theorem' -p myproject/main
```

`search QUERY` matches titles and UUIDs case-insensitively and returns node
summaries, at most `-n` (0–200, default 200). It does not search block contents,
does not fuzzy-match and does not paginate.

Graph commands read the branch's in-memory projection. `check` returns node and
edge counts and issue lists. `full` returns node summaries, each with a `lean`
field (`no_code` or `unverified`: whether a Lean block exists), and index-pair
edges for visualization. `roots` returns unreferenced nodes with topological
depth and weak component size. None of them contact LeanGround; certification is
reported per node by `show` and the node view.

`metric ior` returns the node's UUID, in-degree, out-degree and
`ior = ln((in_degree + 1) / (out_degree + 1))`.

Each request checks the branch's current commit and reloads the projection after
commits made elsewhere. For benchmarks see
[Performance measurements](../../development/performance/).
