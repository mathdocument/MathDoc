# Web service migration

Approved scope: TerminusDB is the source of truth; the local HTTP service owns all
mutations and compilation. Clients resolve exact names or complete UUIDs. Keep
text, Lean, Rocq and LaTeX blocks, with Lean as the first language-server backend.
Keep strict direct managed import / dependency matching. Remove sync/back and the
MathDoc VS Code extension. Commit each tested stage using Conventional Commits.

1. TerminusDB documents, atomic version-guarded writes, graph snapshots and tests.
2. Web/API and CLI cutover; explicit import/export; remove filesystem workflows.
3. Persistent Lean sessions, version-bound checks, Lake artifacts and strict deps.
4. Monaco/Infoview split editor, remove Python/path UI, end-to-end verification.

The generated Lean project is disposable build state, never an editable archive.
Toolchain and Lake project configuration are versioned database documents. Lean
imports are obtained from Lean, not inferred by regex or generated from deps.
Each editor has an isolated draft session. CLI checks reuse persistent sessions.
Compilation success and strict dependency certification remain distinguishable.
Rocq and LaTeX remain editable; their language servers are outside this migration.

Validation: real TerminusDB CRUD, restart persistence and stale-write rejection;
real Lean edit/error/fix, goals, dependency mismatch and repeated warm checks;
frontend type checks/build/tests and browser interaction. Existing filesystem-only
tests are retired with their implementation, not used to certify the new service.

## Execution status

Storage, API/CLI cutover, native Lean sessions and the browser split editor are
implemented and committed. Real database tests, native Lean diagnostics/goals/
imports/builds, persistent certification after restart, browser editing and a
47,435-node / 368,017-edge graph test have passed. The graph test measured a debug
service median of 8.23 ms including the real database version lookup; its cold
projection load was 7.83 s. This is a synthetic graph, not an ETP data benchmark.
An independently pinned Git library also compiled successfully; editing its
consumer reused the unchanged external `.olean` artifact. Small warm proof edits
took about 0.2 s in the native integration test. Large cold library builds have
not been benchmarked.

The standalone MathDoc VS Code extension has been removed. Following explicit
user approval, the unused old implementation and its tests have also been removed:

- `src/application`, `src/cli`, `src/compiler`, `src/depgraph`, `src/formal`,
  `src/indcache`, `src/workdraft`, `src/workspace`.
- `src/web/api.rs`, `src/web/server.rs`, `src/web/deadline_mutex.rs`, `benches`.
- Their filesystem-era integration tests (`test_application_nodes`, `test_cli_*`,
  `test_compiler_process`, `test_config`, `test_depgraph`, `test_formal_status`,
  `test_indcache`, `test_sync`, `test_web_api`, `test_workspace`).

`src/core`, the portable mdoc codec and their tests remain. The codec no longer
contains the old index's header-only parser, identity recovery or file revision
helpers. Unused benchmark scripts, budgets and the direct `thiserror` dependency
have been removed; historical performance reports remain in `perf/reports`.
Default `cargo test --locked` passes all 39 regular tests without Rust warnings.
The four database/API/native Lean integration tests also pass when explicitly
enabled against local TerminusDB and Lean. No ETP files or data have been migrated
or modified.
