# ETP index performance: 2026-09-07

ETP has 47,435 nodes and 368,017 edges. Its complete `mdc graph check`
strongly rereads files and refreshes the index before checking the graph. The
existing small backend benchmark measures these phases separately, so its graph
check timing is not the full CLI latency.

Measurements use the original release binary saved before rebuilding and each
optimized release, sequentially on the same machine. Each variant runs three
`mdc --prof graph check` commands, each followed by
`mdc --prof search equation -n 20`. All graph checks exit successfully with the
same counts and no issues; every search returns 20 results. The OS cache is not
explicitly cleared. Results are local observations, not production percentiles.
JSON files contain every sample and inclusive profiling scopes.

## Part 1: page cache

SQLite connections now have a 128 MiB page-cache budget, allocated on demand.
WAL, transaction synchronization and foreign-key checking are unchanged.

| Median complete command | Original | Page cache |
| --- | ---: | ---: |
| `graph check` | 8.519 s | 5.291 s |
| Search, 20 results | 207.924 ms | 204.332 ms |

The complete graph check improves by 37.9%. Unchanged indexes are still fully
rewritten in this first change; reducing that work is a separate step. The
memory ceiling applies per connection and should be revisited if concurrently
open workspaces create memory pressure.

Validation: 224 library tests, 63 index integration tests and 33 dependency graph
tests passed. Release build, formatting and diff checks passed.

## Part 2: preserve unchanged graph rows

Strong refresh still reads and parses every document. It compares ordered edges
by source path, replaces only changed adjacency, and retains stable symbol IDs.
Unchanged diagnostics are preserved. Degree and depth are still recomputed for
recovery, but their persistence updates only differing values.

The complete ETP check takes 4.048, 2.667 and 2.235 s (median 2.667 s), compared
with 5.291 s after part 1 and 8.519 s originally. Median row reconciliation is
305 ms, down from 3.348 s after part 1; the refresh commit is 1.390 ms. Search
remains approximately 204 ms. These timings retain full file reads and the same
47,435-node / 368,017-edge clean graph.

Validation: 224 library, 64 index, 33 dependency graph and 47 Web API tests passed.
The new regression check records actual graph-table writes: no-op and title-only
refreshes produce none. It also checks same-metadata dependency reordering,
untouched adjacency, stable symbol IDs, removal of unused symbols, and repair of
incorrect degree/depth values. Release build and diff checks passed.
All five existing backend performance budgets pass against the recorded
2026-09-05 baseline on the same CPU/compiler environment; full refresh is
193.840 ms versus 252.103 ms on that 10,000-node fixture.
