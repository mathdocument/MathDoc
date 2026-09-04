---
title: Index & Cache
description: SQLite ownership, schemas, refresh paths, derived graph data, and command behavior.
---

`WorkspaceStore` owns the SQLite database at `.mdc/index.db`, operational indexing and
materialization boundaries, and recoverable file/index mutation sessions. `.mdoc` files
are the workspace source of truth; SQLite is derived and can be rebuilt. `IndCache`
remains a compatibility alias. The current schema version is `24`; compatible schemas
starting at version `17` are migrated transactionally in place, older managed schemas
are rebuilt from `.mdoc` files, and newer schemas are rejected without mutation.
The version 19 migration removes stored missing diagnostics and rebuilds valid in-degree
before enabling their derived view. Version 21 interns dependency endpoint strings and
rewrites edges to compact integer references without recomputing graph-derived rows.
The migration releases the old table and index pages to SQLite's freelist, so they are
immediately reusable even though the database file does not shrink until `VACUUM`.
Version 22 adds rebuildable source-reconciliation observations used by `sync` and `back`.
Version 23 moves the durable, generic `index_dirty` marker from
`mdoc_workdraft_state` into `mdoc_index_state`.
Version 24 removes obsolete graph epochs, weak-component and SCC tables, the index
digest and document count, and the FTS selectivity vocabulary. Base index rows survive
the migration; the removed values were rebuildable cache state.

MathDoc bundles SQLite and requires engine version 3.51.3 or newer so concurrent WAL
readers cannot encounter older WAL-reset defects. The connection enables WAL mode and
foreign keys. It is opened without following
symlinks, and multiply linked database files are rejected. `WorkspaceStore` also keeps an open
guard descriptor for the accepted `index.db` inode. Public operations and mutation
boundaries reject a pathname replacement instead of continuing against a detached
SQLite generation. SQLite's `SQLITE_FCNTL_HAS_MOVED` check also verifies that its own
connection did not open a different inode during a swap-and-restore race. Pure reads
check the generation before and after querying. The cached `.mdc` directory identity is
part of the same check, and cache opening revalidates the mutation lock around bootstrap.

## Backend choice

The index is a transactional, update-heavy graph cache rather than an analytical data
warehouse. SQLite WAL permits separate CLI processes to read while one guarded writer
commits incremental changes. DuckDB's in-process concurrency model requires all writers
to live in one process, which would make `mdc serve` and independent CLI processes require
a new database-owner daemon and IPC protocol. Its columnar scans also do not replace the
point-update, ordered-adjacency, and prefix-resolution indexes used here. DuckDB may be
appropriate for exported offline analytics, but it is not the primary workspace index.

Large-workspace scaling therefore keeps the embedded transactional engine and simple
data paths: compact derived diagnostics, stable integer identities, indexed substring
search, covering adjacency indexes, and direct graph algorithms.

## Internal boundaries

- `schema.rs` owns table DDL, indexes, views, and `PRAGMA user_version` handling.
- `queries.rs` contains pure database reads, including direct weak-component and cycle
  calculation, with no materialization or invalidation.
- `derived.rs` owns complete topological-depth recomputation and targeted in-degree
  maintenance.
- `refresh.rs` owns file-backed row, edge, and issue upserts/deletes, including the full
  in-degree rebuild during strong refresh. Helpers borrow a connection and do not own
  transactions.
- `discovery.rs` enumerates workspace `.mdoc` files and compares `(mtime_ns, size)`
  metadata.

Operational SQLite refresh and upsert transactions are owned by `WorkspaceStore`;
`schema.rs::apply_schema()` separately owns its DDL transaction. Direct `.mdoc`
create/replace operations and `back` imports run through `MutationSession`. Before its
first `.mdoc` write, the session persists `index_dirty = 1`; commit validates the
mutation-lock capability and clears it, while abort rebuilds the index from current
`.mdoc` files. `WorkspaceStore::open()` performs the same recovery when it finds a marker
left by an interrupted process. If the file and index commit but only the final
lock-generation check fails, MathDoc reports uncertainty without rolling the file back
and creating file/index disagreement.

Reference resolution and pure queries do not delete cached rows when path validation
fails; they ignore invalid candidates. Discovery, full or targeted refresh, explicit
path upsert, and `reconcile_fnode_paths()` own stale-row deletion and derived-state
updates. Read-facing graph queries calculate non-persisted derived results directly.

## Database objects

| Object | Key columns | Purpose |
| --- | --- | --- |
| `mdoc_files` | `path`, `mtime_ns`, `size` | Change detection and stale path cleanup |
| `mdocs` | integer `id`, unique `path`, `fnode`, generated `fnode_lc`, title, depth | Search, resolution, persisted depth |
| `mdoc_search` | FTS5 trigram index over fnode and normalized title | Indexed substring search |
| `mdoc_symbols` | integer `id`, unique `fnode` | Interned dependency endpoint strings |
| `mdoc_edges` | `src_path`, `src_symbol_id`, `dst_symbol_id`, `ord` | Compact ordered dependency edges |
| `mdoc_issues` | `path`, `kind`, `ref_fnode`, `error` | Stored invalid and duplicate diagnostics |
| `mdoc_missing_issues` | valid in-degree and claimant indexes | Derived missing-target diagnostics |
| `mdoc_in_degree` | `fnode`, `in_degree` | Precomputed valid-source in-degree |
| `mdoc_index_state` | `bootstrapped`, `index_dirty` | Bootstrap and durable mutation-recovery state |
| `mdoc_workdraft_state` | manifest digest and clean-result counts | Observation-cache generation |
| `mdoc_workdraft_observations` | encoded source path, source type, file generation | Rebuildable sync/back acceleration |

`mdoc_valid_edges` is a schema-owned view. It resolves interned endpoint IDs back to text
and includes `mdoc_edges` whose source path has no `invalid` or `duplicate` issue. A
missing target does not suppress its source edge. Integer source/destination covering
indexes support pair lookups and reverse traversal without scanning a high-degree source,
while the path-leading issue primary key supports the view predicate. Replaced and deleted
paths prune only their now-unreferenced symbols, avoiding a workspace-wide cleanup scan.
`mdoc_missing_issues` derives missing targets
from the maintained valid-edge in-degree aggregate when no complete identity claimant
exists in `mdocs`. Invalid or duplicate complete claimants remain present-but-invalid
rather than also being missing.

The workdraft observation cache is not the synchronization baseline;
`.mdc/source-blocks.json` remains authoritative. Observations are stored only after a
conflict-free reconciliation and are accepted only when that manifest's SHA-256 digest
and complete source inventory still match. Each mdoc and each manifest-present mirror
stores device/inode, size, mtime, ctime, mode, uid, and gid; absent mirrors are implied by
the matching manifest generation instead of occupying database rows. Safe descriptor-relative
`fstatat` batches still check all five candidate mirror paths without rereading file bodies,
so a newly created mirror invalidates the implied absence. A mismatch identifies the affected
sources, which are then reread, reconciled, and validated byte-for-byte. Missing cache state,
legacy or dense observations, source-set changes, or malformed observations fall back to full
reconciliation.

`WorkspaceStore::search(query, limit)` is the canonical ranked search returning
`NodeSummary`. Dependency candidate search reuses its patterns, ranking, and projection
before applying dependency-specific filters. Terms of at least three characters use the
FTS5 trigram index as a candidate prefilter, followed by the original literal matching
and ranking predicates. The index omits position and column-size detail; queries use all
distinct trigrams for short terms and the first, middle, and last trigrams for longer
terms without relying on FTS phrase semantics. Terms of one or two characters use the
linear fallback. Explicit integer document IDs
keep external-content FTS references stable across `VACUUM`. `all_node_summaries()` is
the unbounded non-search query used by the full graph API.

## Fast discovery

`discover_workspace_changes()` enumerates `.mdoc` files and compares cached
`(mtime_ns, size)` values. It upserts changed paths and removes stale paths. If any of
those operations changes graph semantics, the transaction recomputes every topological
depth.

Same-metadata external edits are intentionally deferred to strong refresh paths. File
time is stored as `secs * 1_000_000_000 + subsec_nanos`.

Receipt-backed formal status is revalidated on cache open, strong refresh, focused node
upsert, and explicit formal-status refresh. No-change metadata discovery remains a graph
fast path; callers that monitor compiler artifacts directly must request formal refresh.
Cache open strongly rereads only nodes named by usable work attestations. A formal-status
query strongly reconciles a requested node without an attestation, so unattested block
presence cannot remain stale after a same-metadata external edit without requiring a full
workspace scan on every cache open.

## Strong refresh

`refresh_all()` descriptor-relatively rereads and reparses every discovered document.
Every call rebuilds edge, symbol, issue, in-degree, and topological-depth data. Base node
rows are upserted only when their identity or title changed, preserving stable row IDs and
FTS entries for unchanged nodes. There is no digest or document-count shortcut.

Focused and reachable refreshes retain strong byte reads. A path whose parsed identity,
title, ordered dependencies, parse error, file state, and blocking status are unchanged
can preserve its search, edge, and issue rows. Any graph-semantic change still triggers
a complete topological-depth recomputation before commit.

The first `WorkspaceStore::open()` after database creation or schema rebuild performs a full
bootstrap. Node creation and dependency writes also run `refresh_all()` under the
workspace mutation lock before final duplicate, snapshot, and cycle checks.

Bulk SQL uses `CHUNK_SIZE = 500` for `IN (...)` lists. Full refresh replacement uses
200-row multi-value inserts to remain within SQLite variable limits. Edge batches first
intern their distinct source and target strings, resolve the integer IDs, and then write
the four-column edge rows. Missing diagnostics are queried from their view instead of
being rewritten when a popular target appears or disappears.

## Derived graph data

Leaves have topological depth `0`; each acyclic parent has
`1 + max(depth(dependency))`. Nodes in cycles, and acyclic nodes that transitively
depend on them, may retain partial Kahn accumulation. Cycle reporting is authoritative
in those regions.

Every graph-changing path calls `derived::backfill_all_topo_depths()`. It loads the graph
once, runs the core Kahn-style algorithm, resets stored depths, and persists the complete
result.

Weak-component sizes are calculated directly by `global_root_items()`. Representative
cycles are calculated directly by `graph_check_report()`. Neither result has a persisted
cache, dirty flag, or graph epoch.

## Refresh behavior by command

| Command | Discovery | Content refresh |
| --- | --- | --- |
| `init` | None | None |
| `new` | None | Full refresh, then store-owned create/upsert under lock |
| `edit` | Workspace discovery | Source path upsert after successful editor exit |
| `sync` | None | Full refresh before all-source reconciliation |
| `search` | Workspace discovery | Reparse new or metadata-changed paths only |
| `dep add` | Discovery, then source upsert | Full refresh under lock before write |
| `dep rm` | Discovery, then source upsert | Full refresh under lock before write |
| `dep show` | Workspace discovery | Reachable refresh from source |
| `dep leaf` | Workspace discovery | Reachable refresh from source |
| `dep refs` | Workspace discovery | Target path upsert |
| `graph check` | None | Full refresh before the report |
| `graph roots` | Workspace discovery | Persisted depth and direct weak-component calculation |
| `graph tui` | Workspace discovery | DepGraph APIs and post-operation discovery |
| `metric ior` | None | Full refresh |
| `work` | Workspace discovery | Workspace-wide mirror reconciliation |
| `back` | Discovery before write | Full refresh when mdocs changed |
| `serve` | Every applicable read handler | Focused node upsert for `NodeView`; full refresh only through `workspace/refresh` |

CLI-managed writes call `upsert_path()` directly or use a `DepGraph` mutation that does
so. External additions, deletions, and renames are found by discovery. Commands needing
fresh dependency content use reachable or full refresh rather than relying solely on
directory metadata.
