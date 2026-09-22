# Mathlib certificate recovery and editor disconnections

Apple M3 Pro (12 cores, 18 GiB), macOS ARM64, Lean 4.33.1, 8,312 managed
Mathlib nodes. Release Rust build; existing native Lake artifacts only. No Mathlib
module was compiled and no official cache was downloaded during this benchmark.

The service log contained six `Lean editor request timed out` failures. Browser
certification shared a fixed 30-second timeout with node selection; its timeout
escaped the request handler and closed the WebSocket and Lean server. The same
handler stopped forwarding incoming LSP messages and held the branch snapshot
lock throughout certification.

## Measurements

One full recovery per implementation, using temporary workspaces and empty private
certificate stores, followed by a warm repeat. Filesystem caches were not flushed;
these are local measurements, not cold-disk or cross-platform guarantees.

| Phase | Before | After |
| --- | ---: | ---: |
| Artifact/source/import metadata | 3.41 s | 3.65 s |
| Proof inspection, 8,311 modules | 30.75 s | 11.28 s |
| Certificate publication and validation | 3.99 s | 4.24 s |
| Total first certification | 38.15 s | 19.17 s |
| Warm certification | 0.086 s | 0.103 s |
| Native cached `setup-file`, separately timed | 16.09 s | 12.64 s |

A separate instrumented sequential run spent 4.25 s reading module data and
25.60 s traversing proof expressions. Thus the largest cost was CPU work checking
for `sorryAx`, not writing 8k certificates. There was no observed memory-exhaustion
failure; the disconnect was explicitly the application timeout. Cached imports
still require native setup and interactive Lean loading, independently of
certificate reuse.

The inspector now distributes large closures across at most four child processes,
with a service-wide limit of four. Each child frees module data before reading the
next module. Inspectors use `elan run` with the pinned compiler and only core Lean
libraries, avoiding repeated loading of the Lake project. Small closures amortize compiler startup with at least 128 modules
per worker. Certificate checks are unchanged: source bytes, compiler-produced
imports, private proof bodies, pinned compiler identity and dependency keys.

Custom editor requests now have their own bounded queue, separate from LSP
forwarding. Certification uses the configured Lean timeout and returns a JSON-RPC
error on timeout, preserving the connection. It captures immutable input and
rechecks graph/document versions after completion instead of locking the graph
for the entire scan.

## Reproduction

Export a matching Mathlib branch to an external JSON file. Point
`MDC_BENCH_NATIVE_CACHE` at its populated shared `lake` directory and
`MDC_BENCH_PACKAGES` at its pinned `.lake/packages` directory, then run:

```sh
MDC_BENCH_BUNDLE=/tmp/mathlib.json \
MDC_BENCH_NATIVE_CACHE=/absolute/path/to/shared/lake \
MDC_BENCH_PACKAGES=/absolute/path/to/project/.lake/packages \
cargo test --release --test test_mathlib_bench mathlib_certificate_recovery \
  --locked -- --ignored --nocapture
```

The benchmark runs `lake --no-build setup-file Mathlib.lean`, so missing artifacts
fail rather than trigger a full build. Its import-only umbrella fixture measures
certificate recovery without pretending to provide interactive LSP validation.
The browser regression uses real Lean, holds a real certificate publication lock,
and verifies concurrent goals and graph queries, a recoverable timeout, and a
successful retry on the same connection. Existing native tests cover hidden
`sorry`, module private parts, source/dependency guards and cross-branch reuse.

## Live Mathlib root session

The repaired transport was also exercised on the running `mathlib4/main` branch.
Existing certificates covered 1,662 nodes. Native diagnostics became ready in
35.81 s, followed by recovery of the remaining certificates in 23.28 s. This live
run preceded removal of the redundant Lake launchers measured in the table above.
All 8,312 nodes became verified; the same connection still answered goals.
During recovery, sampled goal requests took 1–263 ms and graph checks 13–315 ms.
A warm certification including protocol/input/graph validation took 537 ms.
These figures distinguish native interactive loading, certificate recovery, and
protocol overhead instead of attributing all time to file writes.
