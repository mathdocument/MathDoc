# Lean editor initialization and refresh

Apple M3 Pro, 18 GiB, macOS ARM64, Lean 4.33.1. Three fresh native file workers
per source, run sequentially with existing artifacts. Mathlib's 8,312 modules
and artifact/package trees were copied into an external benchmark workspace
using APFS clones. No Mathlib dependency was compiled or downloaded. Production
port 17843 was not restarted or modified.

## Initialization phases

Median seconds, including the first sample:

| Source | Worker/Lake setup | Imports/environment | Body/diagnostics | Total |
| --- | ---: | ---: | ---: | ---: |
| One `1 + 1 = 2` theorem, implicit Init | 0.195 | 0.136 | 0.003 | 0.333 |
| Mathlib.Algebra.Algebra.Defs | 0.643 | 0.749 | 0.733 | 2.121 |
| Mathlib umbrella | 3.525 | 3.802 | 0.026 | 7.352 |

The umbrella's first sample took 27.962 s: 11.562 s setup, 16.375 s imports,
0.026 s body. Its next two totals were 7.352 s and 5.453 s. The first ordinary
Mathlib sample took 3.622 s, followed by 2.121 s and 2.109 s. Filesystem caches
were not flushed, so “first” does not mean a reproducible cold disk.

These are observations on a shared development machine, not idle-machine or
cross-platform guarantees. Later unprofiled controls took 3.949 s and 19.943 s
for the ordinary module and umbrella. A subsequent resource snapshot showed
multiple unrelated `vvp` processes near 90% CPU each and 4.65 GiB of swap in
use. Do not interpret the control difference as profiler overhead or a code
regression; load and cache residency were not controlled.

## Method

`perf/lean-editor-phases.py` launches the same official file worker used by
`lake serve`, with the project's Lake environment. It times:

1. Worker initialization through `$/lean/ileanHeaderSetupInfo`, including native
   `lake setup-file` and dynamic-library setup.
2. From that notification to the native profiler's `import took` completion
   marker. This is wall time through import/environment initialization, not the
   exclusive time printed in the profiler message.
3. From import completion to the `textDocument/waitForDiagnostics` response.

The script drains both streams concurrently, records any cross-stream timestamp
skew, checks diagnostics and enforces a 120 s sample deadline and 4 GiB Lean
allocation limit. Detailed native profiling is enabled for stage boundaries;
its overhead is included. The asynchronous import-closure notification is not
used as a boundary because it can arrive after a short body has finished.

These phase measurements exclude browser rendering, mdc source materialization
and certificate publication. Dependencies are opened with build mode `never`,
so missing artifacts fail instead of compiling the library. Use a disposable
workspace with a matching toolchain, source tree, packages and populated cache:

```sh
python3 perf/lean-editor-phases.py /absolute/external/workspace \
  Mathlib.Algebra.Algebra.Defs Mathlib --runs 3 > /tmp/lean-phases.jsonl
```

Raw measurements are in `phases.json`. The browser refresh comparison is measured
separately on the isolated development service at port 17844.
