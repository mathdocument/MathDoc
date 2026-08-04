---
title: Compiler Internals
description: Mirror reconciliation, compiler workspace boundaries, process containment, and configuration flow.
---

The work/back subsystem separates structured `.mdoc` storage from stable language
working trees while preserving independently edited generations on either side.

## Reconciliation

`mdc work <source>` first runs workspace-wide `workdraft::sync()`. Clean changes can
commit while another source/type pair reports a conflict. Any conflict prevents all
selected compilation; existing dirty mirror warnings do not. A selected source block
whose mirror remains missing after reconciliation fails target construction before
compiler invocation; first-time missing mirrors are created by `sync`.

`mdc back` applies the inverse mirror-to-mdoc direction. The manifest baseline stores
both content digest and block presence. A deleted mirror removes a block, after which an
empty placeholder restores the five-file layout.

`src/workdraft/` owns manifest parsing, safe layout, shared change classification,
snapshot-checked batch application, and reverse-order rollback attempts. Rollback is
best-effort and batches are not crash-atomic.

## Compiler boundary

`src/compiler/mod.rs` owns:

- compiler request and result data types
- the `SrcCompiler` interface
- the compile-time language registry
- `CompilerWorkspace`

`CompilerWorkspace` centralizes compiler-root validation, source resolution, and
primitive generated-file operations. Language modules retain orchestration for setup,
driver generation, cleanup, and inventory tracking.

`SrcCompiler` requires `srctype()` and `compile(req)`. The default registry contains
`text`, `python`, `latex`, `lean`, and `rocq`. A result succeeds exactly when `rtcode`
is zero; interruption is separate so CLI dispatch can skip later source types.

## Language state

Lean mirrors use the standard `Lib/` Lake library. Existing `lakefile.toml` and
`lean-toolchain` are preserved; `lakefile.lean` is rejected. MathDoc rewrites
`Lib.lean` to import one selected module and invokes
`lake --quiet --no-ansi build +Lib`. Lake owns imports and reuses `.olean` artifacts.

LaTeX rewrites `Lib.tex` with the selected mirror input and compiles the user-editable
`Main.tex` inside `.mdc/latex/` with
`latexmk -pdf -xelatex -interaction=nonstopmode -halt-on-error Main.tex`. `Main.tex` is
created only while absent.

Python executes the selected mirror directly. Rocq stores `.vo` outputs in a parallel
`build/` tree. Its successful `Lib/**/*.v` inventory digest controls complete build-tree
cleanup, but compilation itself targets only the selected module.

## Subprocess control

`src/compiler/process.rs` owns synchronous execution, exit mapping, signal interception,
process-group termination, deadlines, bounded stdout/stderr draining, and diagnostics.

Every command starts in a new Unix process group. Completion, timeout, interruption,
and I/O cleanup terminate ordinary descendants. Deliberately escaped process groups are
outside the containment boundary. Output is continuously drained to avoid deadlock,
while each stream retains only a bounded head and tail.

## Configuration flow

Per-source overrides are parsed from `[src.<srctype>]` sections of `.mdc/config.toml`
into positive integer durations. `Config::load()` reports malformed values with section
context; `Config::src_config()` merges them with `default_for_srctype()` before building
a `CompilerReq`.

Compilers therefore receive validated, typed deadlines and never inspect TOML or apply
defaults. If either managed Lean setup file is missing, `setup_timeout_sec` gives
`lake init` and `lake env lean --version` separate deadlines. Neither setup process runs
when both files already exist.
