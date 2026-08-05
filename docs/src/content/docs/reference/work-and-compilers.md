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

The `Lib` declaration cannot override `moreLeanArgs`, `weakLeanArgs`, `moreLinkArgs`,
or `moreLeancArgs`; those options can inject compiler inputs outside receipt tracking.

For example, add mathlib using ordinary Lake TOML:

```toml
[[require]]
name = "mathlib"
scope = "leanprover-community"
rev = "<tag-compatible-with-your-lean-toolchain>"
```

Lake follows the selected module's import closure and reuses `.lake/build` artifacts.
MathDoc does not clean `.olean` files.

For formal status, MathDoc asks Lean for the selected source's direct `--src-deps` and
`--deps`. Direct imports of managed `Lib.*` modules must match the node's `@dep` entries
exactly in both directions. External imports remain allowed and their resolved artifacts
are included in the compilation receipt.

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

Python captures the selected mirror and executes those bytes from an inherited
read-only, unlinked file descriptor with bytecode generation disabled. It retains normal
direct-script `__main__`, `sys.argv`, `__file__`, and sibling-import behavior. A source or
working-tree generation change during execution makes the target fail. MathDoc does not
infer or install Python dependencies.

### Rocq

Rocq compiles only the selected mirror into the parallel `.mdc/rocq/build/` tree; it
does not follow an import closure. MathDoc digests the inventory of `Lib/**/*.v` files.
Compared with the last successful inventory, an addition, removal, rename, or content
change removes the existing build tree before the next selected compile. Non-`.v` and
directory-only changes are not part of that digest.

MathDoc uses `rocq dep` to resolve direct `Require`, `Require Import`, `Require Export`,
and `From ... Require ...` dependencies. Managed modules must match `@dep` exactly.
`Load` is rejected because it consumes source text rather than an independently checked
module artifact. Compilation uses `-q`, so user rcfiles cannot add hidden dependencies.

## Formal verification status

Lean and Rocq blocks start `Unverified`. A language becomes `Verified` only after a
successful `mdc work` publishes a matching compilation receipt. The receipt binds the
node's archive and mirror content, target module, selected artifact, compiler binary,
formal environment, direct managed dependency artifacts, and direct external dependency
artifacts. `.mdc/formal-attestations.json` persists these receipts as managed state.

Every managed direct import must have a matching `@dep`, and every `@dep` must be a
direct managed import in the same language. A dependency without a verified block in
that language keeps its referrers unverified. Changes propagate through all transitive
referrers; dependency cycles do not bootstrap themselves into verified status.

`mdc work` revokes the selected node's previous Lean and Rocq attestations before target
construction. Languages publish independently, so one formal compiler may verify even
when another target fails. Malformed or inaccessible attestation state never grants
verification and does not prevent ordinary indexing and editing.

Workspaces created before receipt-backed status was introduced have no attestations, so
their formal blocks become `Unverified` on upgrade. Run `mdc work` on dependencies before
their referrers to establish new verified status.

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
