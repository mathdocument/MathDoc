# Architecture performance follow-up: 2026-09-05

The architecture refactor introduced a cache-lock polling delay that slowed concurrent API requests. Notification-based deadline waits remove that regression. Moving graph response assembly outside the cache lock provides a further improvement for large graphs.

## Revisions and method

- Original: `5694368`; architecture refactor: `d2df382`; lock fix: `fe75634`; final implementation: `b9bf14c`.
- API harness: `6c4c4bb`, copied unchanged into every measured source snapshot. Full revisions and the harness digest are in [metadata.json](metadata.json).
- Apple M3 Pro, macOS arm64, rustc 1.95.0, release optimization; all runs sequential, with warmed caches and five samples. API runtime uses 12 workers.
- Each API sample is a barrier-synchronized request burst through the real Axum router, including serialization, body consumption, and JSON decoding. It excludes TCP and browser costs. The nearest-rank P95 is the slowest request in these 16/17-request bursts; the table reports the median of five burst P95 values.
- Small scenarios use 1,000 independent nodes: eight searches plus eight graph reads, optionally with one revision-checked title write. The large scenario uses 10,000 nodes / 19,993 edges and sixteen graph reads.
- Fixtures, warmed response sizes, graph counts, absence of cycles, and the applied title write are validated outside timed work. These are local benchmark observations, not production latency estimates or statistical significance claims.

## Concurrent backend results

| Scenario | Original | Refactor | Lock fix | Final |
|---|---:|---:|---:|---:|
| 16 concurrent reads | 40.06 ms | 122.09 ms | 10.90 ms | 11.02 ms |
| 16 reads + one write | 69.38 ms | 125.09 ms | 31.71 ms | 30.37 ms |
| 16 large graph reads | 549.49 ms | 263.66 ms | 230.07 ms | 220.74 ms |

Final burst latency is lower than the original by 72.5% (16 concurrent reads), 56.2% (16 reads + one write), 59.8% (16 large graph reads). The isolated graph assembly change reduces large-graph latency by 4.1% in this run.

The former `try_lock` / 10 ms sleep loop made queued readers continue sleeping after the cache became available. `DeadlineMutex` now waits on a condition variable, notifies on every unlock, wakes all waiters on poison, and passes a wakeup on when a notified waiter expires. Five-second deadlines, poison errors, write lock ordering, cross-process flock protection, and database snapshot consistency remain in place.

Full graph reads copy owned nodes and edges from one SQLite snapshot, then release the cache lock before filtering nodes and mapping edge indexes. This response work can overlap the next request.

The new [API budgets](../../api-budgets.json) correctly rejected the refactor version when compared with the lock-fixed report: both small concurrent scenarios exceeded their limits. The final version passes all three budgets against the lock-fixed version. CI prepares the same harness in the base checkout even when that version did not contain the new benchmark.

## Existing backend baseline

10,000 nodes / 19,993 edges; five warmed samples. Search averages 100 queries per sample and reports milliseconds per query. All five existing budgets pass.

| Metric | Original | Final | Change |
|---|---:|---:|---:|
| `graph.check` | 16.907 ms | 15.932 ms | -5.77% |
| `graph.fullRead` | 10.760 ms | 11.089 ms | +3.06% |
| `index.discoverNoop` | 22.970 ms | 23.759 ms | +3.43% |
| `index.fullRefresh` | 240.328 ms | 252.103 ms | +4.90% |
| `query.search200` | 5.776 ms | 5.866 ms | +1.56% |

## Frontend recheck

Node v26.8.1 / Chromium 151.0.7922.34, 1440×900, 500 editor lines and a 10,000-node graph. The existing benchmark uses mocked API data and therefore measures frontend behavior independently of the backend fix. All eight budgets pass.

| Metric | Original | Current | Change |
|---|---:|---:|---:|
| `bundle.shellTransferBytes` | 51,198 B | 51,444 B | +0.48% |
| `bundle.totalTransferBytes` | 590,764 B | 591,008 B | +0.04% |
| `runtime.editorReadyMs` | 137.200 ms | 139.000 ms | +1.31% |
| `runtime.editorHighlightMs` | 223.700 ms | 224.300 ms | +0.27% |
| `runtime.themeSwitchMs` | 280.300 ms | 285.800 ms | +1.96% |
| `runtime.latexPreviewMs` | 155.500 ms | 160.500 ms | +3.22% |
| `runtime.graphReadyMs` | 363.100 ms | 370.600 ms | +2.07% |
| `runtime.graphZoomFrameP95Ms` | 10.500 ms | 10.600 ms | +0.95% |

The earlier theme-switch observation was +8.6%; this recheck is +2.0% (280.3 → 285.8 ms). It does not establish a repeatable regression of the earlier magnitude. Shell transfer remains 246 B larger than the original (+0.48%). Small timing movements are within existing budgets; the follow-up makes no frontend production changes and does not relax or replace existing baselines.

## Verification

- 477 Rust tests passed after the deadline-lock fix, including expiry, notification contention, poison, cache freshness, and mutation consistency checks.
- All 47 Web API integration tests passed again after graph response assembly moved outside the lock.
- Four browser scenarios passed against the real rebuilt backend: stale save conflicts, navigation during save, back/forward history, and competing CLI/browser dependency writes.
- Documentation checks and production build passed. CI YAML parsed successfully, and base benchmark preparation was checked for idempotence and identical harness contents. The GitHub-hosted workflow itself was not run locally.
- Existing backend and frontend budgets passed; the new API comparator was exercised both on a passing optimization and on the intentionally regressed version, which failed as expected.

## Reproduce

Create isolated checkouts of the revisions above. For each older checkout, run the current helper to install the identical API harness, then build before starting measurements:

```sh
python3 perf/prepare-api-base.py /path/to/base
cargo bench --locked --manifest-path /path/to/base/Cargo.toml --bench api --no-run
cargo bench --locked --bench api --no-run
cargo bench --locked --manifest-path /path/to/base/Cargo.toml --bench api -- --output /tmp/api-base.json
cargo bench --locked --bench api -- --output /tmp/api-current.json --compare /tmp/api-base.json
```

Run on the same machine, without concurrent builds or measurements. See the [benchmark instructions](../../README.md) for the existing backend and workflow suites, and `web/perf/README.md` for browser measurements. Each benchmark JSON report includes raw samples and fixture/environment metadata.
