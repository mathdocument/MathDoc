# Backend performance baseline

The benchmark exercises the production Rust backend against a deterministic
10,000-node / 19,993-edge workspace. It reports medians for a full index refresh,
unchanged-workspace discovery, a 200-result search, a full graph read, and a graph
consistency check. Fixture creation and correctness checks are outside timed work;
filesystem and SQLite caches are warm.

```sh
cargo bench --locked --bench backend
```

The default command compares a new run with `baseline.json`. Run it on the same
machine when checking a local change. Use `--output perf/latest.json` to measure
without comparison, and use `--record` only when intentionally accepting a new
baseline:

```sh
cargo bench --locked --bench backend -- --output perf/latest.json
cargo bench --locked --bench backend -- --record
```

`MDC_PERF_SAMPLES` controls the sample count and defaults to five. Runtime timings
vary across machines, so pull requests measure base and head revisions on the same
GitHub runner and enforce the tolerances in `budgets.json`. Changes to the fixture,
budgets, or committed baseline should be reviewed with the performance change that
requires them. Lower values are better for every metric.

## Application workflow benchmark

```sh
cargo bench --locked --bench workflows -- --output perf/workflows-latest.json
```

This separate benchmark uses 1,000 initial nodes and 25 mutations, comparing sequential
and batched creation and dependency edits. It also measures 16 concurrent graph/search
requests alongside a title write through the real Axum router, including response body
serialization and cache contention. It excludes TCP/browser costs. Reports contain three
raw samples, medians, and the median of per-run request p95 values, all in milliseconds.
Each scenario validates its resulting graph or response before accepting the measurement.

Use `--compare <report.json>` only on the same machine. The workflow comparison allows
20% relative growth or 30 ms absolute noise, whichever is greater. CI compares base and
head when both contain the benchmark; its first introduction records a report without a
comparison. Existing backend query budgets and baselines remain independent.

## Concurrent API benchmark

```sh
cargo bench --locked --bench api -- --output perf/api-latest.json
cargo bench --locked --bench api -- --output perf/api-latest.json --compare /path/to/base.json
```

This benchmark catches lock contention that single-operation benchmarks and mocked
frontend requests cannot observe. It synchronizes each request burst with a barrier:

- 1,000 independent nodes: eight searches and eight full graph reads.
- The same readers plus one revision-checked title write.
- 10,000 nodes / 19,993 edges: sixteen full graph reads.

Requests run through the real Axum router and include response serialization, body
consumption, and JSON decoding, without TCP or browser costs. Fixture construction,
route warm-up, and graph/write correctness checks are outside timed work. The report
retains five samples per scenario (`MDC_PERF_SAMPLES` overrides this, minimum three).
Each sample is the burst's nearest-rank p95, equal to its slowest request with these
16/17-request bursts. Reported metrics are medians of those samples, not long-running
production p95 estimates.

`api-budgets.json` enforces relative growth or absolute noise, whichever is greater.
Comparison rejects changed environments, fixtures, schemas, or metric sets. CI runs
the current API harness on both base and head, including on first introduction, so
a newly added scenario can expose a regression in the same pull request. Production
source in the base checkout is untouched. Raw reports are uploaded with other backend
performance results.
