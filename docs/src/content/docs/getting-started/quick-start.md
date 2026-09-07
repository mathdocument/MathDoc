---
title: Quick start
---

Start the local service as described in [Installation](../installation/), then use New node in the browser. Add text, Lean, Rocq or LaTeX blocks. Lean opens a source editor and native Infoview side by side.

```sh
mdc new -t 'Example'
printf 'theorem exampleA : True := by trivial\n' | mdc edit Example --type lean
mdc lean check Example
mdc lean check Example --build
mdc graph check
```

`Save & check` validates the current database source and dependency versions. `Save & build` also produces Lake artifacts including `.olean`. CLI source edits read stdin. `sync` and `back` no longer exist. To retain an archive, run `mdc export > backup.json`.
