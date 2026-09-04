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
