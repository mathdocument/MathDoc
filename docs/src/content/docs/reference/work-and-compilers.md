---
title: Lean checks and builds
---

```sh
mdc lean check 'Theorem' -p myproject/main --revision NODE_REV
mdc lean check 'Theorem' -p myproject/main --build
mdc lean goals 'Theorem' -p myproject/main --line 2 --column 4 --revision NODE_REV
```

`check` certifies the selected node against its current sources, managed imports
and dependency certificates. It reuses matching saved-editor certificates and
existing Lean/Lake caches. A cold check starts a native Lean worker and prepares
required imports. `--build` additionally requires the target's `.olean`; it does
not force a rebuild when matching artifacts already exist.

`goals` queries native Lean proof goals at the requested position. Line and column
are zero-based LSP positions; columns count UTF-16 code units and default to 0.
Both commands accept `--revision` from `show` to reject a changed target. Lean
inputs are checked again after compilation so a changed dependency cannot be
certified using an old result.

Results distinguish `passed` (no Lean errors), `certified` (passed plus exact
managed imports and certified dependencies), and `built` (target artifacts ready).
`cache_hit` indicates a reused result. A check exits with code 1 when uncertified,
while retaining its JSON result. Lean's `sorry` warnings remain warnings.

Saving in the browser stores source; the existing editor automatically certifies
that exact saved version once native diagnostics and import information are ready.
Save does not run a second checker or force a target artifact build. CLI source
edits and checks remain separate commands.

Native Lean Server handles incremental elaboration and goals; native Lake handles
library and target artifacts. Rocq and LaTeX remain editable, but only Lean
compilation is integrated. See [Lean internals](../../development/compiler-internals/)
for editor sessions and cache reuse.
