---
title: Performance measurements
---

Run commands below from the source checkout, with service caches and disposable
test databases outside it. Set up dependencies using [Development setup](../setup/).
Record build mode, machine, database size and whether caches were warm.

## Backend graph operations

### Existing project over HTTP and CLI

`perf/graph-service.py` measures graph validation, roots, full graph transfer, searches,
name/UUID resolution, node views, direct/transitive dependencies and referrers,
leaves, dependency candidates, IOR, history, and eight concurrent readers. It
records all samples, median/p95 and response size. It never requests Lean checking
or opens a Lean editor.

```sh
python3 perf/graph-service.py --url http://127.0.0.1:7600 \
  --cli "$(command -v mdc)" --proj etp/main --samples 20 --output /tmp/graph-baseline.json
```

The benchmark's `--url` selects the HTTP measurement endpoint; `--proj` is required
with `--cli` and must identify the same service. The mdc CLI itself has no URL option.

The default is read-only. For writes, create a disposable database branch and run
a separate service for that branch, then pass `--writes` to its URL. This adds
temporary nodes and measures creation, rename, text saves, adding/removing edges,
and rejection of stale revisions and cycles. Afterwards, run `mdc stop DATABASE/BRANCH`
and `mdc branch del -p DATABASE/BRANCH` to remove the test branch and its caches.
The benchmark does not delete nodes itself.

HTTP measurements include connection establishment, response transfer and JSON
decoding. CLI measurements additionally include a fresh client process and output
decoding. Both require an already running service; record process startup separately.
No OS/database caches are flushed. These local sample percentiles are observations,
not production latency guarantees. Browser rendering is excluded.

### Synthetic graph regression

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
enforcing a machine-independent latency threshold. Browser measurements are described below.

The filesystem/SQLite benchmark targets, preparation script and active budgets
are no longer active. [Archived reports](https://github.com/mathdocument/MathDoc/tree/main/perf/reports)
preserve their historical measurements, including the earlier ETP investigation. Commands inside those
reports refer to their original Git revisions.

## Frontend performance

The benchmark runs the production build in Chromium at 1440x900 against fixed,
in-process API fixtures. It measures the shell and total compressed payload,
a 500-line LaTeX editor, and a 10,000-node / 19,993-edge graph. Browser timings
use a warm HTTP cache and report median values, except for the graph frame p95.

```sh
npm exec --prefix web -- playwright install --no-shell chromium
npm --prefix web run perf
```

`npm --prefix web run perf` compares a new run with `web/perf/baseline.json`. Run it on the same
machine when checking a local change. `npm --prefix web run perf:measure` writes an
uncompared report to `web/perf/latest.json`; that file is ignored. Use
`npm --prefix web run perf:record` only when intentionally accepting a new baseline.

Runtime timings vary across machines. Pull requests therefore benchmark the
base and head revisions on the same GitHub runner and apply the tolerances in
`web/perf/budgets.json`. Bundle budgets are also capped absolutely. CI uploads both raw
reports so an unexpected result can be inspected without rerunning it.

Changes to fixtures, budgets, or `web/perf/baseline.json` should be reviewed alongside
the optimization that requires them. Lower values are better for every metric.

The lazy native Monaco/Infoview bundle is included in the recorded total of
about 8.46 MB compressed, with a 9 MB absolute ceiling and an 8% regression
budget. The initial shell has a separate 56 KiB ceiling. Runtime fixtures measure
LaTeX and graph interaction; native Lean is checked by the real-server browser test.
