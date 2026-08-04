---
title: Work, Back & Compilers
description: Complete behavior of mirror reconciliation and each native compiler integration.
---

## `mdc work`

Reconcile source mirrors and compile the selected node's available source types:

```bash
mdc work <source>
```

`work` first performs workspace-wide mirror reconciliation. It may commit unrelated
clean mirror and manifest changes even if another pair conflicts. Any remaining
conflict skips every compiler and returns `1`. Existing dirty mirrors are compiled
as-is, but a selected source block whose mirror was deleted makes target construction
fail with `source mirror is missing`; run `mdc back` to import an intentional deletion.

Each source type represented by a block or a nonempty mirror is compiled in this order:
`text`, `latex`, `python`, `lean`, `rocq`. A selected node with no targets is a successful
no-op. There is no MathDoc dependency depth: native imports and includes determine
compiler dependencies.

Dirty mirror content is compiled as-is without an implicit `back` operation.

## Compiler behavior

| Type | Required command | Behavior |
| --- | --- | --- |
| `text` | None | Successful no-op |
| `latex` | `latexmk`, `xelatex` | Builds `Main.tex` with XeLaTeX |
| `python` | `python3` or `python` | Runs the selected `Lib/` file directly with `-B` |
| `lean` | `lake` | Validates and builds the standard `Lib` Lake library |
| `rocq` | `rocq` | Compiles only the selected `.v` mirror |

### Lean

Lean uses the standard layout created by `lake init Lib lib`. If absent,
`lakefile.toml` and `lean-toolchain` are copied from staged initialization; existing
files are preserved. `lakefile.lean` is unsupported.

MathDoc writes one `import Lib.<selected-module>` line to `.mdc/lean/Lib.lean` and runs:

```bash
lake --quiet --no-ansi build +Lib
```

The conventional `lakefile.toml` owns dependencies and must retain:

```toml
[[lean_lib]]
name = "Lib"
```

For example, add mathlib using ordinary Lake TOML:

```toml
[[require]]
name = "mathlib"
scope = "leanprover-community"
rev = "<tag-compatible-with-your-lean-toolchain>"
```

Lake follows the selected module's import closure and reuses `.lake/build` artifacts.
MathDoc does not clean `.olean` files.

### LaTeX

MathDoc writes the selected mirror as an `\input` in `.mdc/latex/Lib.tex`, then runs
`latexmk -pdf -xelatex -interaction=nonstopmode -halt-on-error Main.tex` inside
`.mdc/latex/`. A minimal `Main.tex` is created whenever it is absent and is otherwise
left untouched. MathDoc does not restore the `Lib.tex` input if the user removes it from
`Main.tex`.

LaTeX mirror paths must be UTF-8 and cannot contain `"`, `{`, `}`, `%`, carriage
returns, or line feeds. Lean module path components must be UTF-8 and cannot contain
`«`, `»`, carriage returns, or line feeds. These restrictions are enforced when the
compiler driver is generated, not by `mdc new`.

### Python

Python runs the selected mirror directly from the `Lib` tree with bytecode generation
disabled. MathDoc does not infer or install Python dependencies.

### Rocq

Rocq compiles only the selected mirror into the parallel `.mdc/rocq/build/` tree; it
does not follow an import closure. MathDoc digests the inventory of `Lib/**/*.v` files.
Compared with the last successful inventory, an addition, removal, rename, or content
change removes the existing build tree before the next selected compile. Non-`.v` and
directory-only changes are not part of that digest.

## Process results

Each compiler runs in a separate Unix process group. MathDoc applies configured
timeouts, intercepts SIGINT and SIGTERM, drains output without pipe deadlock, and
terminates ordinary descendants during completion, timeout, interruption, or cleanup.
Processes that deliberately escape the process group remain outside this containment.

Each stdout and stderr stream retains at most 1 MiB: the first 512 KiB and last 512 KiB
with an omission marker between them.

One failed target propagates an exit code in `1..=255`. Missing tools normally return
`127`; timeouts return `124`. Multiple failures, or a single result outside that range,
return `1`. If MathDoc catches SIGINT or SIGTERM during a compiler, it returns `130` or
`143` and skips later source types. A compiler that independently dies by signal
currently maps to `1`.

## `mdc back`

Import changed mirrors into their corresponding `.mdoc` blocks:

```bash
mdc back
```

A mirror is imported only when the mdoc block still matches the stored baseline. An
independently changed mdoc is preserved and reported, while unrelated clean imports may
still commit. Deleting a mirror removes its source block; the empty mirror is recreated
to preserve the five-file layout.

Imported content must be UTF-8. Nonempty content is normalized to end with a newline.
The command refreshes the complete index when mdocs change and exits `1` while conflicts
remain.
