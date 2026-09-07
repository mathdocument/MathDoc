---
title: Graph and compilation caches
---

The former filesystem index and workspace freshness logic are retired. Graph requests perform a database commit lookup and use the resident validated projection.

Lean keys include the environment, stable module identity, Lean source and direct dependency keys recursively. Editing text, titles, Rocq or LaTeX leaves these keys unchanged. A Lean edit updates keys only for affected referrers; graph changes recompute graph metadata.

Checks are cached once per node in memory; certified results and Lake artifacts persist under the service cache directory. After restart, matching certificates are loaded on demand by node UUID and complete input key, with no directory scan. Four hot CLI files keep live workers; evicted files retain Lake artifacts. One service owns each branch cache. CLI checks currently serialize per branch; browser sessions have eight slots. Separate database branches/services provide independent work environments.
