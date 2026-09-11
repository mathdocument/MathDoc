# Mathlib source-management benchmark

Source: mathlib4 `ac3d0c9671540e5ea16ebbbf0699e4f62f4e9f96`, Lean `v4.34.0-rc2`.
Imported 8,525 modules (`Mathlib/**/*.lean` plus `Mathlib.lean`), 35,195 edges,
98,873,504 source bytes. Module names, source bytes, metadata, direct dependencies
and project settings survived an exact JSON database roundtrip. Other non-hidden
UTF-8 tracked files are supporting project files, not mathematical graph nodes.
The metadata report explicitly lists excluded supporting files.

All service caches and generated workspaces are outside the MathDoc checkout.
`mathlib4/main` retains the imported baseline. Mutation tests use `mathlib4/bench`.
No official precompiled mathlib cache was downloaded. `LAKE_NO_CACHE=true` and
`MATHLIB_NO_CACHE_ON_UPDATE=1` disable remote artifact downloads for compilation
experiments; local incremental artifacts remain enabled for both implementations.

## Source preparation

Release Rust build, same source bundle and machine. Ten input-capture samples,
three unchanged-refresh samples per module. The baseline is the native-project
implementation at `6fa7c66`; the after measurements use shared immutable snapshots,
cached project/module indexes and source materialization state. These are mdc
overhead measurements, **not** Lean proof-checking speedups or comparisons with Lake.

| Operation | Before | After |
| --- | ---: | ---: |
| Capture 143-module closure (median) | 9.16 ms | 0.081 ms |
| Capture 1,687-module closure (median) | 13.65 ms | 1.52 ms |
| Capture all 8,525 modules (median) | 34.37 ms | 10.06 ms |
| Refresh unchanged 1,687-module closure (median) | 52.26 ms | 0.042 ms |
| Refresh unchanged all-module closure (median) | 286.37 ms | 0.193 ms |
| Edit `Mathlib.Init`, invalidate affected keys (one sample) | 324.18 ms | 25.64 ms |

Initial full source materialization still costs about one second. Startup still
reads and hashes the complete database snapshot. Neither cost is paid again for
an unchanged node selection in an existing editor environment.

## Reproduce

```sh
python3 perf/mathlib-import.py --source .external/mathlib4 --output /tmp/mdc-mathlib/mathlib.json
mdc init mathlib4
mdc start mathlib4/main
mdc import /tmp/mdc-mathlib/mathlib.json -p mathlib4/main
MDC_BENCH_BUNDLE=/tmp/mdc-mathlib/mathlib.json \
MDC_BENCH_REPORT=/tmp/mdc-mathlib/preparation.json \
  cargo test --release --locked --test test_mathlib_bench -- --ignored --nocapture
python3 perf/graph-service.py --url SERVICE_URL --proj mathlib4/main --cli /path/to/mdc \
  --samples 5 --output /tmp/mdc-mathlib/graph.json
```

`SERVICE_URL` is the URL returned by `mdc start`. Import only into a new empty
branch. The source adapter uses the pinned compiler's import-header parser; it
does not elaborate mathematical proofs or rewrite imports. The parser exits
explicitly after flushing successful output to avoid the observed macOS thread
cleanup crash; parsing errors still fail the command.

## Native compiler limitation observed

On this machine (macOS 27, Apple Silicon), the unmodified Lean `v4.34.0-rc2`
`lake --version`, `lake build`, and returning from a `lean --run` program can
exit with SIGTRAP in `_pthread_tsd_cleanup` / the system allocator. Plain Lean
elaboration of a small theorem succeeds. These failures also occur independently
of mdc, before a mathlib compilation baseline can complete. Main-thread and
single-worker settings did not make the build reliable. No failed process is
counted as successful compilation, and no whole-mathlib verification is claimed.
