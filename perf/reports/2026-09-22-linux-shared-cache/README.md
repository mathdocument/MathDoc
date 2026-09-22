# Native Lake shared-cache prototype on Linux/ext4

This is a capability experiment, not a MathDoc backend change or a Mathlib
throughput benchmark. No TerminusDB project or running MathDoc service was used.

## Environment

- Unmodified Lean **4.33.1**, commit `819816b2e0a3bf405af45ae5c7af2491d8f5bee6`;
  bundled Lake `5.0.0-src+819816b`.
- Debian bookworm container, Linux aarch64, kernel `6.8.0-50-generic`;
  Colima VM has 2 CPUs and about 3.8 GiB RAM.
- Docker named volume backed by **ext4**, confirmed with `findmnt`.
  `cp --reflink=always` failed with `Operation not supported`.
- Build/LSP processes ran as UID 10001. Workspaces and the pool were on the same
  filesystem. No reflink or downloaded package cache was used.
- Two modules, including 1,500 generated definitions; compile-time markers
  distinguish compilation from trace replay. The markers only record events.

All workspaces used a **writable** shared `LAKE_CACHE_DIR`, with
`LAKE_ARTIFACT_CACHE=true` and `LAKE_RESTORE_ARTIFACTS=false`. This is the native
Lake path, without a custom artifact format, publish service, or copied cache.

## Results

Raw measurements: [results.json](results.json).
Runnable stdlib-only check: [lean-shared-cache.py](../../lean-shared-cache.py).

| Experiment | Result |
| --- | --- |
| Empty pool, initial build | 1,964 ms in the recorded final run; toolchain/OS file caches were already warm |
| Four empty workspaces concurrently reuse the pool | 136–146 ms each, **zero compiler events** |
| Per-workspace dependency `.olean` copies | **Zero** in the four consumer workspaces |
| Required `.ilean` files | Same device/inode as their pool objects: **hardlinks, not copies** |
| Additional file blocks for four consumer `.lake` directories | **65,536 bytes total**, excluding directory metadata and counting each shared inode once |
| Baseline pool | 6 artifact objects; 2,269,184 allocated bytes including mappings |
| Change the dependency from value 1 to value 2 | Both affected modules compile; all original pool object checksums remain unchanged |
| Two LSP sessions use different branch inputs | One proves `value = 1`, the other `value = 2`; wrong-branch proof fails and correcting the draft succeeds |
| LSP without local dependency `.olean` | Diagnostics, goals, and module hierarchy/imports work |
| Stop one LSP and remove its workspace plus the original producer | Other LSP keeps working; new original-version consumer builds in 135 ms without recompilation |
| Four simultaneous cold requests for identical input | **Four compilations of each module**; no native cross-process single-flight |
| Remove only the stopped variant's exclusive objects | Original branch still hits; requesting the removed variant recompiles successfully |
| New `module` format | 123 ms cache reuse, no local `.olean*`; pool includes `.ir`, `.ir.sig`, `.olean.server`, `.olean.private` |
| Read-only pool, cache hit | Works, 131 ms |
| Read-only pool, cache miss | **Build fails with permission denied**; no automatic private write layer |

The parallel cold experiment deliberately delays elaboration to expose duplicate
work. Its timing is not a throughput comparison. Output mapping JSON was valid
at the end of these runs; this does **not** establish atomic publication or
crash safety for all interleavings.

## What this establishes

The large compiled objects can be stored once on ext4 and used by separate
workspaces and LSP processes. Each workspace still needs its own mutable
`.lake` configuration/traces and newly built outputs. Native restoration uses
hardlinks for required local files; new producer files may also remain linked
to pool objects. Counting directory sizes separately can therefore overstate
physical usage. Do not edit those linked artifacts in place.

This supports using native Lake sharing instead of copying a complete cache
per session. The local experiment does not require changing Lake itself.

It does not establish a production cache lifecycle:

- Cross-process cold-request deduplication is missing.
- A read-only pool plus private writes is not provided by these flags. A
  writable native pool or an additional adapter is necessary.
- Lake `Cache.writeOutputsCore` writes mapping JSON directly. Our runs did not
  expose a corrupt mapping, but publication/reader interleavings still need
  handling; cache misses must remain recoverable.
- Reclamation was controlled, **after the variant's reader stopped**. There is
  no GC implementation or reader-lease guarantee here. Live LSPs can open more
  import artifacts later; protecting only already-open files is insufficient.
- The fixture has no external package checkout, native plugin, or full Mathlib.
  Sharing writable `.lake/packages` and Linux x86_64 behavior remain untested.
- The processes share a trusted UID and one filesystem. Cross-filesystem
  restoration can fall back to copies; no NFS or multi-host guarantees follow.

## Reproduce

Use a disposable Linux container with Python 3, `util-linux`, coreutils, and the
unmodified Lean 4.33.1 toolchain on `PATH`. Keep both the workspaces and cache on
one ext4 volume; Docker's writable overlay or a macOS bind mount is not the
filesystem tested here. The script asserts ext4 and the absence of reflink.

```sh
# /experiment must be an empty/disposable, writable ext4 volume.
PATH=/opt/lean/bin:$PATH python3 perf/lean-shared-cache.py /experiment
```

The script creates a unique `lake-sharing-*` directory and prints its path. It
leaves `results.json`, workspace logs, and cache files there for inspection;
its caller must remove that directory after retaining any wanted evidence.
No network is required after installing the toolchain. The original experiment
used the official `lean-4.33.1-linux_aarch64.tar.zst` release archive, with its
reported compiler commit checked above. The temporary container and volume
were removed after collecting these results.
