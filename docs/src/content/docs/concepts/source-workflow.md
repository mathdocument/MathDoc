---
title: Source Workflow
description: Reconcile mdoc blocks with stable language mirrors and native compilers.
---

`.mdoc` is the structured storage format, but language tools work best with stable,
ordinary source files. MathDoc bridges the two representations with `sync`, `work`, and
`back`.

## Export with sync

```bash
mdc sync
```

`sync` strongly refreshes the complete index, then exports every structurally parseable
document into five mirrors under `.mdc/<srctype>/Lib/`. Relative paths are preserved;
for example, `data/A.mdoc` produces:

```text
.mdc/text/Lib/data/A.txt
.mdc/latex/Lib/data/A.tex
.mdc/python/Lib/data/A.py
.mdc/lean/Lib/data/A.lean
.mdc/rocq/Lib/data/A.v
```

Missing source blocks produce empty mirrors. Duplicate-fnode files remain exportable
because duplication is an index issue, while structurally unparseable files retain
their previous mirrors.

## Edit with native tools

Use the mirrors with the editor, language server, and project tooling intended for each
language. MathDoc keeps them in stable project layouts so imports, includes, and build
artifacts can be reused.

## Reconcile and compile with work

```bash
mdc work notes/theorem
```

`work` first runs workspace-wide mirror reconciliation. If no conflict remains, it
compiles each source type represented by a block or a nonempty mirror on the selected
node. Compilation follows built-in source type order; language imports and includes,
not the MathDoc dependency graph, determine compiler dependencies.

Dirty mirror content is compiled without being written back implicitly. A node with no
compiler targets is a successful no-op.

## Import with back

```bash
mdc back
```

`back` writes changed mirror content into the matching source block only when the mdoc
still matches the stored baseline. Deleting a mirror removes that block; an empty
placeholder mirror is recreated afterward to retain the predictable five-file layout.
Imported nonempty content is normalized to end with a newline and must be UTF-8.

## Conflict behavior

`.mdc/source-blocks.json` records the last synchronized content and block-presence
baseline for every source/type pair.

| Change since baseline | `sync` result | `back` result |
| --- | --- | --- |
| Only mdoc changed | Update clean mirror | Keep mdoc |
| Only mirror changed | Preserve dirty mirror | Update mdoc |
| Both changed identically | Reconcile baseline | Reconcile baseline |
| Both changed differently | Preserve both; report conflict | Preserve both; report conflict |

Conflicts are isolated per source/type pair, so unrelated clean changes may still be
committed. `sync` and `back` exit with status `1` while dirty mirrors or conflicts remain;
`work` skips all compilers if reconciliation reports a conflict.
