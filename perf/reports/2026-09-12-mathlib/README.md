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
  cargo test --release --locked --test test_mathlib_bench \
  mathlib_snapshot_and_editor_preparation -- --ignored --nocapture
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

Follow-up diagnosis: this release pins mimalloc 3.4.1. Its native crash stack
matches the upstream [macOS thread-exit allocator issue](https://github.com/microsoft/mimalloc/issues/1333).
The unmodified `lake --version` and a `lean --run` program using `IO.asTask`
failed with SIGTRAP in all six control runs, independently of mdc.
A temporary runtime was relinked from the installed Lean archives and the same
tag's command-line shell, replacing mimalloc with 3.4.4 and its pthread TLS model.
The Lean compiler archives and `.olean` standard library were reused unchanged.
All 20 corresponding smoke runs passed. A temporary toolchain layout was needed
to make child processes use that library too; five native `lake serve` document
open/check/close cycles then passed, followed by a normal shutdown.
[`macos-runtime.json`](macos-runtime.json) records these observations. This is
runtime diagnosis, not a full-Mathlib compatibility or performance result; no
replacement was installed into the user's Lean toolchain during this experiment.

Separately, mdc now reports initialization disconnects immediately and owns the
single automatic reconnect, preserving unsaved edits. Previously a transport
failure could leave the editor's initialization wait unresolved and surface as
the unrelated 30-second node-preparation timeout. The browser regression suite
passes 11/11, including disconnect-before-initialize and subsequent draft recovery.

## Dependency candidates and graph baseline

The candidate endpoint scanned the direct dependency vector for every matching
node, twice. Replacing that membership test with a set reduces the high-degree
`Mathlib` root query from 121.99 ms to 7.39 ms (five samples, median).
`graph-candidates.json` also records writes on the disposable branch: text saves
19.21 ms, rename 21.24 ms, but node creation and edge changes still about 360 ms.
Those structural writes still rebuild all input hashes and are the next measured
bottleneck. The after run uses the title query `Algebra`; the original report's
`equation` query had no matching module names, so those search timings are not
comparable. Concurrent Linux Rust compilation shared the Docker VM during this
run; small differences in unrelated endpoints should not be treated as gains.

Browser regression: 10/10 passed, including native goals/diagnostics, draft
preservation, cancellation, changed dependencies and concurrent CLI/browser edges.
The small native fixture measured 302 ms from save to automatic verification,
301 ms warm navigation and 500 ms warm edit diagnostics. These are fixture timings,
not full-Mathlib editor measurements. Documentation checking and build passed.

## Structural writes

After separating topology/index rebuilding from source hashing, the same mutation
sequence on a fresh fork gives these median HTTP latencies (five samples):

| Operation | Full source rehash | Affected keys only |
| --- | ---: | ---: |
| Create node | 360.28 ms | 56.65 ms |
| Add dependency | 358.07 ms | 53.00 ms |
| Remove dependency | 359.66 ms | 53.13 ms |

The graph is still validated, topology is recomputed, and TerminusDB persists the
transaction before responding. Unrelated source hashes and certificates remain
unchanged. An incremental-update regression compares keys and depths with a full
rebuild; the real database API/import tests and ordinary Rust tests pass.
`graph-final.json` contains all read/write samples. Startup/import/config changes
still rebuild the full projection; concurrent reads still serialize through the
service snapshot lock. No claim of eliminating those costs is made.

## Compiler comparison without official caches

The identical Lean `v4.34.0-rc2` succeeds in Linux aarch64. The disposable Docker
container used `rust:slim` at digest
`sha256:bce1476d4be4d78b83705bc5f428b86d640eeeea33e9dadafbc037b5703a53bf`,
limited to one CPU and 2 GiB RAM with `LEAN_NUM_THREADS=2`. The Docker VM has two
CPUs and 4 GiB RAM and also hosts TerminusDB. No OOM kills occurred. These timings
must not be compared numerically with the native macOS preparation/graph timings.

The target was `Mathlib.Data.Nat.Log`: 143 managed modules, with additional pinned
external-library dependencies (712 `.olean` files under each completed workspace,
including Lake configuration files). Both sides started with separate empty
artifact caches and the complete original source tree. Toolchain installation,
source materialization and Git source downloads were untimed; compiler startup,
dependency compilation and generation of the target artifact were timed.

| Scenario (one sample each) | Native Lake build | mdc check/build |
| --- | ---: | ---: |
| Cold | 174.46 s | 189.66 s |
| Unchanged | 976.52 ms | 3.55 ms |
| Append a comment to target | 2,692.69 ms | 2,798.10 ms |
| Append a comment to `Mathlib.Init` | 960.94 ms | 3,251.11 ms |

All four mdc results passed compilation, dependency matching and target-artifact
checks, with no sorry warning on the target. `linux-compile.json` retains the raw
results. These edit cases change comments, not theorem statements or proofs;
they are not measurements of a semantically significant dependency change.

The warm unchanged result benefits from mdc's exact-input certificate. Cold builds
and comment edits are **not faster than Lake** in this run. A build request still
uses LSP validation followed by Lake artifact generation; dependency changes also
reopen the LSP document environment. Lake can retain downstream artifacts when a
comment edit leaves compiled dependency outputs unchanged, while mdc conservatively
invalidates source-based keys. Those are remaining costs, separate from the graph
and source-refresh improvements above. There is no claim of accelerating Lean's
kernel or having verified every Mathlib module.

Run `mathlib_lake_and_mdc_incremental_builds` as documented in the performance guide
to reproduce with a fresh external directory. The imported `mathlib4/main` is
available for browsing with the updated global CLI. Native editing on this macOS
machine still requires resolving the upstream/toolchain crash; the successful
Linux run does not fix the installed macOS toolchain.

Cleanup: the mutation-test branch `mathlib4/bench`, its service/cache, the temporary
Linux container and its generated source/build directories were removed after
saving results. `mathlib4/main` retains the exact imported graph, served at
`http://127.0.0.1:57939` in this run. The pre-existing Mdocs service was not restarted.
The pinned source checkout remains clean. macOS Lean toolchains remain installed;
no compiler patch or global toolchain workaround was applied.
