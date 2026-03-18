# Cache Update Logic

## Database schema (version 8)

| Table                  | Key columns                                                                              | Purpose                                                              |
| ---------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `mdoc_dirs`            | `path`, `mtime_ns`                                                                       | Directory mtime cache for cheap external add/delete/rename detection |
| `mdoc_files`           | `path`, `mtime_sec`, `mtime_ns`, `size`                                                  | File-state cache for change detection and stale-path cleanup         |
| `mdocs`                | `path`, `fnode`, `title`, `title_lc`, `mtime_sec`, `mtime_ns`, `size`, `topo_depth`     | Searchable node cache, reference resolution, and persisted topo depth |
| `mdoc_edges`           | `src_path`, `src_fnode`, `dst_fnode`, `ord`                                              | Persisted dependency edges in source order                           |
| `mdoc_issues`          | `path`, `kind`, `ref_fnode`, `error`                                                     | Structural problems: `invalid`, `duplicate`, `missing`               |
| `mdoc_in_degree`       | `fnode`, `in_degree`                                                                     | Precomputed in-degree for root detection                             |
| `mdoc_weak_component`  | `fnode`, `component_id`, `component_size`                                                | Weak connected components with stable representative and size        |
| `mdoc_index_state`     | `graph_epoch`, `weak_component_dirty`, `bootstrapped`, `topo_depth_backfilled`           | Epoch counter, incremental refresh state, and migration flags        |
| `mdoc_scc_result`      | `graph_epoch`, `cycles_json`                                                             | Cached SCC/cycle result, invalidated by epoch change                 |

## Two refresh mechanisms

**`discover_workspace_changes()`** — incremental scan. Compares cached directory `mtime_ns` values against the filesystem. Only re-stats directories that changed, then upserts/removes affected files. Topo depth is updated incrementally (upward BFS per changed fnode) for add/update operations; if any deletion is detected, falls back to a full `backfill_all_topo_depths` since ancestor depths can decrease. Leaves weak components for lazy recompute (dirty bit). Fast and safe to call on every command.

**`refresh_all()`** — full rescan. Walks the entire workspace tree unconditionally, stats every file, and reconciles the full index. Used only by `mdc sync`.

## Derived data: topo_depth and weak components

`mdocs.topo_depth` and `mdoc_weak_component` are derived summaries of the dependency graph, kept up to date via two strategies:

**Bulk (full-graph) path** — used by `refresh_all` and `refresh_indexed_paths` (called by `mdc sync` and `graph check`). Calls `refresh_all_derived_data`, which loads the graph once and runs both Kahn's algorithm (topo depths) and BFS (weak components) in a single pass, persisting both results.

**Incremental (targeted) path** — used by `upsert_path`, `refresh_reachable_from_path`, and `discover_workspace_changes` (on add/update). After each file upsert, runs `refresh_topo_depth_upward_from` for that fnode (BFS upward through the reverse graph, updating depths only where they change). Weak components are handled lazily via the `weak_component_dirty` flag already set by `bump_graph_epoch` inside `upsert_mdoc_row`.

`discover_workspace_changes` falls back to a full `backfill_all_topo_depths` when any deletion is detected, since ancestor depths can decrease and the BFS shortcut is not valid.

`upsert_path` additionally does a full incremental weak component update (union-find on edge add; BFS split-check on edge remove) and clears `weak_component_dirty = 0` on success.

**Lazy recompute** — `global_root_items` checks `weak_component_dirty`. If set, it performs a full weak-component recompute and clears the flag before reading. If clear, it reads `topo_depth` from `mdocs` and `component_size` from `mdoc_weak_component` directly via a single JOIN — no graph load.

## Per-command cache behavior

| Command           | Discovery                    | Content refresh                                                | Notes                                                              |
| ----------------- | ---------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------ |
| `mdc init`        | —                            | —                                                              | Creates `.mdc/`; does not touch the index                          |
| `mdc new`         | —                            | `upsert_path()` on the created file                            | Immediately indexed after creation                                 |
| `mdc sync`        | —                            | `refresh_all()` (full walk + `refresh_all_derived_data`)       | Intentional full-rescan escape hatch                               |
| `mdc search`      | `discover_workspace_changes` | —                                                              | Queries `mdocs`; external changes picked up via discovery          |
| `mdc dep add`     | via `DepGraph::from_ref`     | inside `add/create_and_add_dependency`                         | Index update is part of the DepGraph mutation; cycle-creating deps rejected |
| `mdc dep rm`      | via `DepGraph::from_ref`     | inside `remove_direct_dependencies`                            | Index update is part of the DepGraph mutation                      |
| `mdc dep show`    | `discover_workspace_changes` | `refresh_reachable_from_path()` on source                      | Targeted file re-stat; exits 1 if cycles detected                      |
| `mdc dep leaf`    | `discover_workspace_changes` | `refresh_reachable_from_path()` on source                      | Targeted file re-stat; exits 1 if cycles detected                      |
| `mdc dep refs`    | `discover_workspace_changes` | `upsert_path()` on target                                      | Target refreshed before reverse-edge resolution                    |
| `mdc graph check` | `refresh_workspace_index()`  | re-stats all indexed file content                              | Discovers new/deleted files and re-reads content of known files    |
| `mdc graph roots` | `discover_workspace_changes` | —                                                              | Reads topo_depth + component_size from DB; graph load only if dirty |
| `mdc graph tui`   | `discover_workspace_changes` | inside DepGraph mutation API, then `discover_workspace_changes` | add/rm/create delegate to DepGraph which handles upsert; `refresh_after_op` uses discovery |
| `mdc eval`        | `discover_workspace_changes` | `upsert_path()` + `refresh_reachable_from_path()`              | Preflight: exits 1 on broken deps or cycles; then runs blocks      |

## How file state changes are discovered

- **CLI-managed writes**: `mdc new` calls `upsert_path()` directly after creating the file. `mdc dep add` and `mdc dep rm` route through `add_direct_dependencies`, `remove_direct_dependencies`, and `create_and_add_dependency`, which call `upsert_path()` internally — index updates are part of the DepGraph mutation API, not the caller's responsibility.
- **External adds/deletes/renames**: detected by `discover_workspace_changes()`, which walks cached directories, compares `mtime_ns`, and re-stats only directories that changed. This runs at the start of every non-trivial command.
- **External content edits to known files**: discovered by `refresh_reachable_from_path()` (stat-checks files reachable from a given node) or by `refresh_all()` (`mdc sync`). Commands that only need the index for search/display (e.g. `mdc graph roots`) do not re-stat file content; run `mdc sync` to force a full refresh.

## Design notes

- `IndCache` owns all transaction boundaries. `queries.rs` and `refresh.rs` take `&Connection` and never open transactions themselves.
- Mtime is stored as two columns (`mtime_sec`, `mtime_ns`) where `mtime_ns = secs * 1_000_000_000 + subsec_nanos`. Incremental rescans skip files where both values match.
- `CHUNK_SIZE = 500` is used for chunked SQL `IN (...)` queries and bulk inserts to stay within SQLite's variable limit.
- `topo_depth` is persisted in `mdocs` so `all_topo_depths` is a plain `SELECT` — no graph traversal. Incremental updates walk only the ancestor subgraph of the changed node.
- `component_id` is the lexicographically smallest `fnode` in the component (a stable representative). Union-find merges by size: all members of the smaller component are mass-UPDATE'd to the larger's `component_id`. Split detection on edge removal is O(component size), not O(V+E).
- `mdc sync` is the intentional broad-sweep path. All other commands use targeted or incremental refresh to keep latency low even in large workspaces.
- `add_direct_dependencies` rejects cycle-creating deps at write time via `is_reachable` on the indexed graph. Cycles can still be introduced by direct file edits; `mdc graph check` detects and reports them. `mdc graph check` uses `refresh_workspace_index()` (not just `discover_workspace_changes()`) so it validates actual file content, not only the cached index state.
- `open_db` returns `(Connection, bool)` where the bool signals that `topo_depth` was just added during migration (all-zero defaults). `IndCache::open` runs `backfill_all_topo_depths` in a transaction before serving any reads, so a quiet repo that never changes files still gets correct depths immediately after upgrade.
- `DepGraph::from_ref` calls `cache.upsert_path` on the resolved path before the duplicate-fnode check. This ensures a file found only via filesystem fallback (not yet in the index) is indexed first, so `duplicate_fnode_paths` sees it alongside any existing indexed duplicate.
