# ETP database-service baseline: 2026-09-09

The migrated ETP graph has **47,435 nodes and 368,017 edges**. Ordinary CLI graph
checks take **11.7 ms median**, with no workspace refresh. Structural writes still
take about **600 ms**, and large graph reads can delay concurrent small reads.

## Data and measurement

- Release binary: `bd88be40bc128cac1ce921007c6c9addf92fc4f3`, globally installed `mdc` 0.5.0.
- Apple M3 Pro, 18 GiB RAM, macOS 26.6.2; TerminusDB in the existing local Colima/Docker instance.
- Database image: `terminusdb/terminusdb-server@sha256:385faf298ad77aaf2d4d6df5e84a4cbe3596d01dab2e3b991af905639ae56388`.
- Import: 39.68 s for a 181.29 MB JSON bundle. All UUIDs, module identities,
  names, dependency sets, block contents/metadata and project settings matched
  the subsequent full database export. No missing dependencies or cycles.
- Three blocks per node: text 8.53 MB, LaTeX 80.76 MB, Lean 54.53 MB. The Lean
  blocks contain 1,191,301 lines in total, averaging 25.1 lines per node. This is
  the actual ETP dataset, not a synthetic 500-line-per-node workload.
- New service process startup: **5.05 s**, including loading/validating the full
  database snapshot and computing graph/Lean input indexes. OS and database
  caches were not flushed. Service RSS after the benchmark: **457.2 MiB**.
- Twenty sequential samples per operation; one untimed warmup for each HTTP
  operation. HTTP timings include direct loopback transport and JSON decoding.
  CLI timings include a fresh process, HTTP requests and output decoding.
- Read/write measurements used an isolated native database branch of ETP. Its
  service, branch and temporary cache were removed afterwards; the main branch
  revision remained unchanged. The database now exposes only `main`.
- **No Lean check, build, editor session, or graph-wide compilation was requested.**
  These measurements do not cover browser rendering or compilation latency.

The Python sampler disables proxy discovery for direct local HTTP. An initial
calibration found that macOS proxy discovery added roughly 80 ms per request;
that preliminary run is excluded from these results. Percentiles describe these
local samples, not a production latency guarantee.

## Reads

All values are milliseconds. CLI entries without a number were not sampled.

| Operation | HTTP median | HTTP p95 | CLI median | Response / scope |
| --- | ---: | ---: | ---: | --- |
| Graph check | 5.9 | 6.2 | 11.7 | Validated projection, all 47,435 nodes |
| Roots | 265.1 | 295.6 | 306.1 | 39,800 roots, 6.31 MB |
| Full graph | 167.4 | 177.0 | 269.6 | 47,435 nodes + 368,017 edges, 10.73 MB |
| Search, common term | 5.6 | 7.3 | 10.8 | 20 results |
| Search, absent term | 8.7 | 9.1 | — | Scans all names, zero results |
| Resolve UUID | 5.1 | 5.3 | — | Complete UUID |
| Resolve name | 6.1 | 6.5 | — | Unique exact name |
| View, highest out-degree | 6.2 | 7.1 | 15.3 | 43 dependencies, 19 KB |
| View, highest in-degree | 34.1 | 36.6 | — | 7,625 referrers, 2.34 MB |
| Direct dependencies | 5.3 | 5.5 | — | 43 results |
| Transitive dependencies | 5.2 | 5.6 | 20.8 | Maximum-depth node, 55 results |
| Direct referrers | 17.5 | 18.4 | — | 7,625 results |
| Transitive referrers | 127.1 | 142.1 | 203.8 | 47,426 results, 5.75 MB |
| Leaves | 5.4 | 6.5 | 20.7 | Maximum-depth node, 2 results |
| Dependency candidates | 11.3 | 12.2 | — | Empty search, 50 results |
| IOR | 5.1 | 5.2 | 21.6 | Cached degrees |
| History | 11.8 | 12.1 | — | 3 commits before benchmark writes |

Graph checks read a projection validated when loading or accepting mutations;
they do not rerun SCC validation on every read. This differs intentionally from
the retired filesystem command, which scanned files before reporting. The
[2026-09-07 report](../2026-09-07-etp/README.md) recorded 8.519 s originally and
3.465 s after filesystem optimizations on the same node/edge counts. Those are
historical observations, not an interleaved A/B run of the current binary.

Eight concurrent clients made 40 mixed requests (check, roots, search, transitive
dependencies and transitive referrers): **543.9 ms median, 733.5 ms p95**, 2.918 s
total. This deliberately includes expensive full-result queries. It is not a
maximum-throughput stress test.

## Writes

Mutations use new text-only benchmark nodes in the full ETP graph. Acyclicity and
optimistic revision rejection are asserted. These are graph/storage timings;
they do not represent editing a heavily referenced Lean definition.

| Operation | Median ms | p95 ms |
| --- | ---: | ---: |
| Create node | 599.4 | 616.8 |
| Rename | 23.9 | 31.3 |
| Save text | 20.3 | 21.6 |
| Add dependency edge | 597.3 | 624.6 |
| Remove dependency edge | 598.5 | 616.6 |
| Reject dependency cycle | 83.8 | 88.3 |
| Reject stale revision | 4.5 | 5.5 |

## Remaining costs

The old per-command file synchronization cost is gone. Reading the implementation
explains the remaining measured differences, although this run did not instrument
individual internal phases:

1. Structural mutations validate the whole graph and recompute depths, reverse
   adjacency and all Lean input keys. This is the main target for reducing the
   roughly 600 ms create/add/remove latency. Rename and text saves already avoid it.
2. Reads share one snapshot mutex and perform a database revision lookup. Roots
   additionally calculate component sizes each time; large read handlers build
   their responses while holding that mutex. This explains why expensive reads
   can queue smaller concurrent requests.
3. Startup and a database revision changed outside this service load all source
   blocks as well as topology. The 5.05 s startup is amortized by a long-lived
   service; it is not paid by ordinary CLI commands. External-change reload was
   not separately timed in this run.
4. Full graph, roots and transitive-referrer responses are several megabytes.
   Payload transfer, JSON work and browser layout remain separate costs from
   database graph traversal.

See [graph-service.json](graph-service.json) for every timing sample, response
size and selected node identity, and [metadata.json](metadata.json) for dataset
and environment details. Reproduction instructions are in
[perf/README.md](../../README.md).
