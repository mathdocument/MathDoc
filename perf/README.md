# Backend performance

The current benchmark exercises the real TerminusDB-backed service on a synthetic
47,435-node / 368,017-edge graph. It reports one-time import and startup separately,
then seven graph-check samples and their median, including the database revision
lookup and API response decoding. It excludes CLI startup, HTTP client transport
and browser rendering.

```sh
python3 scripts/test-local.py test_service_scale
```

The test creates a separate database and prints its name. To reuse that same
fixture without importing it again, set `MDC_SCALE_DATABASE` to the printed name.
The local database and private `.env` are required. CI runs the same target with
`--release`; compare results only with the same build mode and machine.

The benchmark validates node/edge counts and acyclicity. It reports timing without
enforcing a machine-independent latency threshold. Browser measurements remain in
[`web/perf`](../web/perf/README.md).

The filesystem/SQLite benchmark targets, preparation script and active budgets
were retired with the old backend. [`reports`](reports) preserves their historical
measurements, including the earlier ETP investigation. Commands inside those
reports refer to their original Git revisions.
