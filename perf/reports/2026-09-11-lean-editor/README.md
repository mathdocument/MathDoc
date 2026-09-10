# Lean editor stability and ETP navigation: 2026-09-11

Validated release `52feb1b` on the existing ETP database: **47,435 nodes,
368,017 edges**, pinned Lean **4.29.1**. The database revision did not change.
Only two nodes and their required imports were opened; no graph-wide compilation
or database source edits were performed.

## Passing ETP browser probe

Apple M3 Pro, 18 GiB RAM, native Lean, headless Chromium at 1440 × 1000.
The service and editable compiler workspace ran under a temporary directory.
Pinned external packages were shared with the existing cache. The measured run
reused dependency build artifacts produced by the two-node preliminary probe;
it started a fresh browser and Lean server. OS caches were not flushed.

| Action | Source visible | Lean ready |
| --- | ---: | ---: |
| Open Equation 181 implies Equation 24 | 975 ms | 8,331 ms |
| First switch to Equation 83 implies Equation 117 | 384 ms | 4,202 ms |
| Return to Equation 181 implies Equation 24 | 670 ms | 671 ms |
| Return to Equation 83 implies Equation 117 | 498 ms | 499 ms |

These are single observations, not percentiles. Switch times include opening the
search dialog, choosing the node, moving to the top of its source and verifying
the rendered import text. The first node has a 55-node dependency closure; the
second has 43 direct dependencies. The probe then moved into their proofs to
exercise Infoview. Raw results: [etp.json](etp.json).

The entire run used **one LSP initialize** and made **32 native Infoview RPC
calls**. Goals rendered correctly. There were no browser exceptions, unexpected
server-stop messages, recursion-limit errors or broken-pipe errors. The server
log remained clean through session and service shutdown. Its temporary compiler
workspace was removed afterwards.

## What remains expensive

A preliminary run with the original dependency-artifact cache displayed source
in **959 ms**, but needed **54,170 ms** for the first Lean-ready status. A later
test locator failed on Monaco's wrapped import lines; that incomplete probe is
excluded from the passing-run JSON above. Process inspection showed Lake
`setup-file` compiling missing managed imports, including eight simultaneous
compiler children at roughly 1.3 GiB RSS each. This wait includes dependency
compilation and memory pressure, not just reading Mathlib imports.

The UI can now display and accept edits throughout that wait. Two live document
workers make returns fast; new or evicted workers still load imports. Dependency
artifacts built only inside an isolated editor workspace are still discarded
when that session closes unless they also exist in the canonical build cache.
The passing warmed-artifact timings therefore do not promise an 8-second cold
open for every node or a reload with the original cache.

## Regression coverage

- 11 Rust library tests and 8 graph algorithm tests passed, including raw framed
  JSON and URI translation with **2,048 nested levels**. The real ETP RPC sample
  reached depth 14; the synthetic regression covers the original depth failure.
- Two native Lean integration tests passed, covering incremental diagnostics,
  goals, import matching, external artifacts and clean shutdown. A changed proof
  check took **211 ms** in the small fixture; this is not an ETP timing.
- Svelte check: zero errors or warnings. All 19 frontend unit tests passed.
- All eight browser tests passed, including delayed initialization, delayed and
  superseded selections, draft handoff and undo, forced disconnect recovery,
  worker reaping and clean service shutdown. Test databases were deleted.

The runnable regressions remain in `src/lean.rs`, `src/service.rs`,
`tests/test_lean_service.rs` and `web/e2e/run.mjs`.
