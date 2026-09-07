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

## Part 3: cheaper path sorting

Workspace scans now compare normalized OS strings directly instead of repeatedly
parsing path components during sorting. Both strong refresh and fast discovery
use this ordering. Discovery still enumerates every file and checks its metadata;
external edits, including edits that preserve directory timestamps, remain covered.
Profiling now separates file-state loading, directory enumeration, sorting and stat.

The saved part 2 release was rerun alongside the final release to account for
timing variation. Each column below is the median of three complete CLI runs.

| ETP metric | Part 2 recheck | Final |
| --- | ---: | ---: |
| `graph check` | 3.431 s | 3.465 s |
| Search, 20 results | 217.572 ms | 175.667 ms |
| Discovery inside search | 154.108 ms | 113.850 ms |

Search improves by 19.3% and discovery by 26.1% in this comparison. Graph check
is essentially unchanged by part 3; its final samples are 3.992, 3.465 and 3.283 s.
The final median is 59.3% below the original 8.519 s, but the earlier part 2 median
of 2.667 s and its later 3.431 s recheck show that total latency varies between
runs. These observations do not establish a fixed speedup for every environment.
All ETP graph checks still report the same counts and no issues.

Validation: all 478 Rust tests pass, including the graph-write regression and
external-file-change cases. Documentation checking reports no errors or warnings.
Release build, formatting and diff checks pass. All five backend and all three
API performance budgets pass against the 2026-09-05 reports. On the backend
fixture, no-op discovery is 16.972 ms versus 23.759 ms originally, and full
refresh is 163.007 ms versus 252.103 ms. API mixed-burst p95 increases from
30.369 to 36.673 ms, within its 45.369 ms budget; the other two API metrics improve.

`discovery.json` and `refresh-recheck.json` contain the final ETP comparison;
`backend-final.json` and `api-final.json` contain the existing benchmark results.
The ETP JSON `base_revision` identifies the commit before each local optimization;
the saved part 2 binary used for its recheck is exactly that revision.

Reproduce the automated performance checks with:

```sh
cargo bench --offline --locked --bench backend -- --output /tmp/backend-final.json --compare perf/reports/2026-09-05-architecture/backend-final.json
cargo bench --offline --locked --bench api -- --output /tmp/api-final.json --compare perf/reports/2026-09-05-architecture/api-final.json
```

For ETP, build with `cargo build --release --offline --locked`, then run that
release binary with `--prof graph check` and `--prof search equation -n 20` from
the workspace directory three times in sequence. These commands refresh the
derived index; they do not modify source documents.
