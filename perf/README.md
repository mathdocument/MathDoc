# Backend performance

## Existing project over HTTP and CLI

`graph-service.py` measures graph validation, roots, full graph transfer, searches,
name/UUID resolution, node views, direct/transitive dependencies and referrers,
leaves, dependency candidates, IOR, history, and eight concurrent readers. It
records all samples, median/p95 and response size. It never requests Lean checking
or opens a Lean editor.

```sh
python3 perf/graph-service.py --url http://127.0.0.1:7600 \
  --cli "$(command -v mdc)" --samples 20 --output /tmp/graph-baseline.json
```

The default is read-only. For writes, create a disposable database branch and run
a separate service for that branch, then pass `--writes` to its URL. This adds
temporary nodes and measures creation, rename, text saves, adding/removing edges,
and rejection of stale revisions and cycles. Delete the test branch and its
compiler-cache directory afterwards. The benchmark does not delete nodes itself.

HTTP measurements include connection establishment, response transfer and JSON
decoding. CLI measurements additionally include a fresh client process and output
decoding. Both require an already running service; record process startup separately.
No OS/database caches are flushed. These local sample percentiles are observations,
not production latency guarantees. Browser rendering is excluded.

## Synthetic graph regression

The Rust benchmark exercises the real TerminusDB-backed service on a synthetic
47,435-node / 368,017-edge graph. It reports one-time import and startup separately,
then seven graph-check samples and their median, including the database revision
lookup and API response decoding. It excludes CLI startup, HTTP client transport
and browser rendering.

```sh
cargo test --release --locked --test test_service_scale -- --ignored --nocapture
```

The test creates a separate database and a temporary compiler cache, and removes
both when it finishes, including on assertion failure. A local database and
credentials in `MDC_TERMINUS_PASSWORD` are required. CI runs with `--release`;
compare results only with the same build mode and machine.

The benchmark validates node/edge counts and acyclicity. It reports timing without
enforcing a machine-independent latency threshold. Browser measurements remain in
[`web/perf`](../web/perf/README.md).

The filesystem/SQLite benchmark targets, preparation script and active budgets
were retired with the old backend. [`reports`](reports) preserves their historical
measurements, including the earlier ETP investigation. Commands inside those
reports refer to their original Git revisions.
