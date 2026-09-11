---
title: Graph and compilation caches
---

Graph requests perform a database commit lookup and use the resident validated
projection. They do not scan source files or reconcile a workspace. An external
database commit triggers a full snapshot reload, including source blocks.

Lean keys include the environment, stable module identity, Lean source and direct dependency keys recursively. Editing text, titles, Rocq or LaTeX leaves these keys unchanged. A Lean edit updates keys only for affected referrers; their unchanged source hashing state is reused. Node creation and dependency changes recompute graph metadata but update Lean keys only for changed nodes and affected referrers. Project configuration changes and full imports rebuild all keys. Compilation inputs share immutable node and project snapshots. An editor tracks which node versions it has materialized, so selecting unchanged nodes does not reread generated source files.

Checks are cached once per node in memory; certified results and Lake artifacts persist under the service cache directory. Startup scans saved certificates once; lookups require the current complete input key. Graph status reads use the in-memory results. One hot CLI file keeps a live worker; evicted files retain Lake artifacts. One service owns each branch cache. CLI checks currently serialize per branch; browser sessions have eight slots, with two hot document workers per browser tab. Separate database branches/services provide independent work environments.

Lake artifacts are shared by CLI and browser sessions within one branch cache.
Different branches have separate caches; forking a branch does not copy or share
its parent's `.olean` files. See [Storage](../../concepts/workspaces/) for paths
and safe cleanup.
