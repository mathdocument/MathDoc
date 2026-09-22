---
title: Graph and compilation caches
---

Graph requests perform a database commit lookup and use the resident validated
projection. They do not scan source files or reconcile a workspace. An external
database commit triggers a full snapshot reload, including source blocks.

Lean keys include the environment, stable module identity, Lean source and direct dependency keys recursively. Editing text, titles, Rocq or LaTeX leaves these keys unchanged. A Lean edit updates keys only for affected referrers; their unchanged source hashing state is reused. Node creation and dependency changes recompute graph metadata but update Lean keys only for changed nodes and affected referrers. Project configuration changes and full imports rebuild all keys. Compilation inputs share immutable node and project snapshots. An editor tracks which node versions it has materialized, so selecting unchanged nodes does not reread generated source files.

Certified checks persist in the database pool by complete recursive input key,
project configuration, platform, compiler identity and certificate schema. Each
branch revalidates native imports against its current graph before reuse. Shared
evidence restores graph statuses in memory on startup and after graph/evidence
changes; opening nodes is unnecessary. URI paths in shared facts use
`file:///project`. Artifact cleanup does not erase proof facts, but `built` always
requires complete native objects. Old `checks-v1` records lack the new identity
and are not promoted; the first check reconstructs evidence using native caches.

One hot CLI file keeps a live worker; evicted files retain Lake artifacts. One
service owns each branch cache. CLI checks serialize per branch; identical cold
inputs across branches also share a cancellable file lock and recheck evidence
after waiting. Different targets may still compile a common cold dependency
twice. Browser sessions remain independent, with eight slots per branch and two
hot document workers per tab. Shared caches do not share unsaved documents or
eliminate per-LSP elaboration.

Lake's native content cache is shared by all branches and browser sessions in a
database, partitioned by platform and project configuration. Workspaces keep
private source, traces and new outputs; they read cached Lean objects directly
instead of copying a build tree. Artifact availability is checked against Lake's
output descriptors, including split module outputs. Deleting a branch preserves
the shared pool. See [Storage](../../concepts/workspaces/) for paths and cleanup.
