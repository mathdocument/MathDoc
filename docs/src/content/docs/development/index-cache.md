---
title: Graph and compilation caches
---

Graph requests perform a database commit lookup and use the resident validated
projection. They do not scan source files or reconcile a workspace. An external
database commit triggers a full snapshot reload, including source blocks.

Lean keys include the environment, stable module identity, Lean source and direct dependency keys recursively. Editing text, titles, Rocq or LaTeX leaves these keys unchanged. A Lean edit updates keys only for affected referrers; graph changes recompute graph metadata.

Checks are cached once per node in memory; certified results and Lake artifacts persist under the service cache directory. After restart, matching certificates are loaded on demand by node UUID and complete input key, with no directory scan. One hot CLI file keeps a live worker; evicted files retain Lake artifacts. One service owns each branch cache. CLI checks currently serialize per branch; browser sessions have eight slots, with two hot document workers per browser tab. Separate database branches/services provide independent work environments.

Lake artifacts are shared by CLI and browser sessions within one branch cache.
Different branches have separate caches; forking a branch does not copy or share
its parent's `.olean` files. See [Storage](../../concepts/workspaces/) for paths
and safe cleanup.
