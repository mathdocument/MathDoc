---
title: Index & Cache
description: SQLite ownership, schemas, refresh paths, derived graph data, and command behavior.
---

`IndCache` owns the SQLite database at `.mdc/index.db`, operational indexing and
materialization transaction boundaries, and the conversion between filesystem state
and indexed graph state. The current schema version is `16`; older managed schemas are
discarded and rebuilt from `.mdoc` files, while newer schemas are rejected without
mutation.

The connection enables WAL mode and foreign keys. It is opened without following
symlinks, and multiply linked database files are rejected. `IndCache` also keeps an open
guard descriptor for the accepted `index.db` inode. Public operations and mutation
boundaries reject a pathname replacement instead of continuing against a detached
SQLite generation. SQLite's `SQLITE_FCNTL_HAS_MOVED` check also verifies that its own
connection did not open a different inode during a swap-and-restore race. Pure reads
check the generation before and after querying. The cached `.mdc` directory identity is
part of the same check, and cache opening revalidates the mutation lock around bootstrap.

## Internal boundaries

- `schema.rs` owns table DDL, indexes, views, and `PRAGMA user_version` handling.
- `queries.rs` contains pure database reads and no materialization or invalidation.
- `derived.rs` owns topological depth, weak components, graph epochs, SCC/cycle cache
  maintenance, and incremental in-degree maintenance.
- `refresh.rs` owns file-backed row, edge, and issue upserts/deletes, including the full
  in-degree rebuild during strong refresh. Helpers borrow a connection and do not own
  transactions.
- `discovery.rs` enumerates workspace `.mdoc` files and compares `(mtime_ns, size)`
  metadata.

Operational multi-step mutations belong in a transaction created and committed by
`src/indcache/mod.rs`. Schema initialization and rebuild are the exception:
`schema.rs::apply_schema()` owns its DDL transaction. Database generation checks bracket
commits, and cache-owned file writes validate both the workspace mutation lock and the
database generation before reporting success or attempting recovery. If the file and
index commit but only the final lock-generation check fails, MathDoc reports uncertainty
without rolling the file back and creating file/index disagreement.

Reference resolution and pure queries do not delete cached rows when path validation
fails; they ignore invalid candidates. Discovery, full or targeted refresh, explicit
path upsert, and `reconcile_fnode_paths()` own stale-row deletion and derived-state
updates. Read-facing facades may transactionally materialize derived caches.

## Database objects

| Object | Key columns | Purpose |
| --- | --- | --- |
| `mdoc_files` | `path`, `mtime_ns`, `size` | Change detection and stale path cleanup |
| `mdocs` | `path`, `fnode`, `title`, `title_lc`, `topo_depth` | Search, resolution, persisted depth |
| `mdoc_edges` | `src_path`, `src_fnode`, `dst_fnode`, `ord` | Ordered dependency edges |
| `mdoc_issues` | `path`, `kind`, `ref_fnode`, `error` | Invalid, duplicate, and missing diagnostics |
| `mdoc_in_degree` | `fnode`, `in_degree` | Precomputed valid-source in-degree |
| `mdoc_weak_component` | `fnode`, `component_size` | Weak component size per node |
| `mdoc_index_state` | epoch, dirty flags, bootstrap, digest | Global materialization state |
| `mdoc_scc_result` | `graph_epoch`, `cycles_json` | Representative cycles for one epoch |

`mdoc_valid_edges` is a schema-owned view. It includes `mdoc_edges` whose source path
has no `invalid` or `duplicate` issue. A missing target does not suppress its source
edge. Source/destination edge indexes and the path-leading issue primary key support the
view predicates.

`IndCache::search(query, limit)` is the canonical ranked search returning
`NodeSummary`. Dependency candidate search reuses its patterns, ranking, and projection
before applying dependency-specific filters. `all_node_summaries()` is the unbounded
non-search query used by the full graph API.

## Fast discovery

`discover_workspace_changes()` enumerates `.mdoc` files and compares cached
`(mtime_ns, size)` values. Adds and updates trigger incremental topological-depth
refresh. Any deletion triggers full depth backfill because ancestor depths may decrease
without one safe starting node.

Same-metadata external edits are intentionally deferred to strong refresh paths. File
time is stored as `secs * 1_000_000_000 + subsec_nanos`.

Receipt-backed formal status is revalidated on cache open, strong refresh, focused node
upsert, and explicit formal-status refresh. No-change metadata discovery remains a graph
fast path; callers that monitor compiler artifacts directly must request formal refresh.

## Strong refresh

`refresh_all()` descriptor-relatively rereads and reparses every discovered document.
A deterministic `index_digest` covers sorted paths, recovered identity, title,
dependency order, and parse status, but not source block bodies.

If the digest matches, existing node, edge, issue, in-degree, epoch, and derived rows are
retained. If it differs, refresh constructs invalid, duplicate, and missing issues in
memory, replaces base rows with multi-value inserts, rebuilds in-degree in one grouped
query, and eagerly rebuilds topological depths and weak components.

Graph changes invalidate the epoch-keyed SCC result; `graph_check_report()` rebuilds it
lazily. Incremental path upserts and deletes clear `index_digest`, forcing the next
strong refresh to reconcile the complete graph.

The first `IndCache::open()` after database creation or schema rebuild performs a full
bootstrap. Node creation and dependency writes also run `refresh_all()` under the
workspace mutation lock before final duplicate, snapshot, and cycle checks.

Bulk SQL uses `CHUNK_SIZE = 500` for `IN (...)` lists. Full refresh replacement uses
200-row multi-value inserts to remain within SQLite variable limits, including the
four-column edge and issue tables.

## Derived graph data

Leaves have topological depth `0`; each acyclic parent has
`1 + max(depth(dependency))`. Nodes in cycles, and acyclic nodes that transitively
depend on them, may retain partial Kahn accumulation. Cycle reporting is authoritative
in those regions.

Bulk paths call `derived::refresh_all_derived_data()`, loading the graph once, running
core Kahn-style depth and weak-component algorithms, and persisting both results.

Incremental paths call `refresh_topo_depth_upward_from(fnode)`. They recompute the node
and all reverse-reachable ancestors in dependency-first order. An encountered cycle
falls back to full depth backfill. Changing one node's `@dep` list without changing its
fnode therefore rewrites only that node's edges, recomputes its depth, and propagates
upward.

Weak-component dirtiness is independent of topological depth. A graph change increments
`graph_epoch` and sets `weak_component_dirty = 1`. `global_root_items()` ensures weak
components transactionally before a pure query joins persisted depth and component
size. `graph_check_report()` similarly ensures the SCC cache for the current epoch.

## Refresh behavior by command

| Command | Discovery | Content refresh |
| --- | --- | --- |
| `init` | None | None |
| `new` | None | Full refresh, then cache-owned create/upsert under lock |
| `edit` | Workspace discovery | Source path upsert after successful editor exit |
| `sync` | None | Full refresh before all-source reconciliation |
| `search` | Workspace discovery | Reparse new or metadata-changed paths only |
| `dep add` | Discovery, then source upsert | Full refresh under lock before write |
| `dep rm` | Discovery, then source upsert | Full refresh under lock before write |
| `dep show` | Workspace discovery | Reachable refresh from source |
| `dep leaf` | Workspace discovery | Reachable refresh from source |
| `dep refs` | Workspace discovery | Target path upsert |
| `graph check` | None | Full refresh |
| `graph roots` | Workspace discovery | Persisted depth; weak graph only if dirty |
| `graph tui` | Workspace discovery | DepGraph APIs and post-operation discovery |
| `metric ior` | None | Full refresh |
| `work` | Workspace discovery | Workspace-wide mirror reconciliation |
| `back` | Discovery before write | Full refresh when mdocs changed |
| `serve` | Discovery in read handlers | Guarded cache replacement after edits |

CLI-managed writes call `upsert_path()` directly or use a `DepGraph` mutation that does
so. External additions, deletions, and renames are found by discovery. Commands needing
fresh dependency content use reachable or full refresh rather than relying solely on
directory metadata.
