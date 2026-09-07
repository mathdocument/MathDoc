---
title: Graph and metrics
---

```sh
mdc graph check
mdc graph roots
mdc graph full
mdc metric ior 'Theorem'
```

These commands query the service's validated graph projection. Graph check returns node and edge counts plus missing/invalid/cycle lists. Full graph returns node summaries and index-pair edges for visualization. Roots are unreferenced nodes; each includes its topological depth and weak component size.

IOR is `ln((in_degree + 1) / (out_degree + 1))`. Reverse edge queries use the projection's reverse adjacency map. No command performs filesystem synchronization.
