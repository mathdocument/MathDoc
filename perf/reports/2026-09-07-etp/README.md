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
