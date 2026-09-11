# Saved editor validation and artifact reuse: 2026-09-11

Validated `493f839` against ETP (47,435 nodes, 368,017 edges), native Lean
4.29.1, on an Apple M3 Pro with 18 GiB RAM. Only “Equation 181 implies
Equation 24” and its required imports were checked. The database revision
remained unchanged. No graph-wide compilation or source edits were performed.

## Results

Two fresh browser sessions used an isolated service and compiler cache outside
the source checkout. External packages were shared with the existing pinned
package cache. Managed artifacts came from an earlier diagnostic run; the
canonical `.lake/build` was empty, so each temporary editor restored imports
through Lake's native artifact cache. Closing each context removed its draft.
OS caches were not flushed.

| Action | First measured session | Next fresh session |
| --- | ---: | ---: |
| Source visible | 1,173 ms | 1,031 ms |
| Lean ready | 22,558 ms | 7,846 ms |
| Automatic Verified visible | 22,602 ms | 7,892 ms |
| Certification result `elapsed_ms` | 34 ms | 0 ms (cached) |
| Subsequent CLI check, total wall time | 43 ms | 40 ms |

Native readiness to visible verification took 44 ms and 46 ms respectively;
`elapsed_ms` is the certificate result field, not end-to-end browser latency.
Both CLI checks were cache hits (server-reported check time: 0 ms). The editor
certified **56 nodes**, including the selected proof and its dependency closure;
all 23 direct dependencies were independently confirmed verified through the API.
Each session used one LSP initialization. Browser exceptions and server errors:
zero, including normal session and service shutdown. Raw results: [etp.json](etp.json).

These are individual observations, not percentiles or cold-compilation claims.
The initial diagnostic run built missing managed dependencies and exposed an
external-import certification bug, now covered by a native regression test.
Its incomplete run is excluded from the table. First-time missing dependencies
still require compilation; an edited target is elaborated by its native worker.
Saving uses those diagnostics and does not force a target `.olean` build.

## Regression coverage

- 12 Rust library and 8 graph tests passed, including stale worker replies and
  the existing 2,048-level JSON transport test.
- Four native Lean tests passed: incremental checking/goals, pinned external
  libraries, certification without a second checker, and artifact reuse after
  draft deletion. The artifact test uses a compiler side effect to distinguish
  a real rebuild from restoration and checks source-change invalidation.
- All 19 frontend unit tests and eight browser tests passed. Svelte reported
  zero errors or warnings; frontend and documentation builds succeeded.
- The browser Save test reached automatic Verified in 305 ms in the small fixture,
  made no extra `/lean/check` request, and covered invalid proofs and dependency
  changes. This is not an ETP Save timing.

This supersedes the artifact-loss limitation in the earlier
[editor stability report](../2026-09-11-lean-editor/README.md).
