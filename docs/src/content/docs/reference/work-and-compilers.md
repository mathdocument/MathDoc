---
title: Lean checks and builds
---

```sh
mdc lean check 'Theorem'
mdc lean check 'Theorem' --revision REV --build
mdc lean goals 'Theorem' --line 2 --column 4
```

Line and column are zero-based LSP positions (columns use UTF-16 code units).

Results distinguish `passed` (no Lean errors), `certified` (passed plus exact managed imports and certified dependencies), and `built` (target Lake artifacts generated). Lean's `sorry` warnings remain warnings. CLI exits unsuccessfully for an uncertified check.

Native Lean Server handles incremental elaboration and goals. Native Lake handles library and target artifacts. Rocq and LaTeX remain editable, but only Lean compilation is integrated now.
