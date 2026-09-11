---
title: Quick start
---

Start the local service as described in [Installation](../installation/), then use New node in the browser. Add text, Lean, Rocq or LaTeX blocks. Lean opens a source editor and native Infoview side by side.

```sh
mdc new --proj myproject/main -t 'Example'
printf 'theorem exampleA : True := by trivial\n' | mdc edit --proj myproject/main Example --type lean
mdc lean check --proj myproject/main Example
mdc lean check --proj myproject/main Example --build
mdc graph check --proj myproject/main
```

The Lean editor checks as you type. **Save** writes the source to the database; once that exact version finishes checking, its status and compiled dependencies update automatically. Editor results and Lake artifacts are shared with CLI checks. Use `mdc lean check --proj myproject/main NODE --build` only when you need the target `.olean` immediately; imports build it on demand. CLI source edits read stdin. To retain an archive, run `mdc export --proj myproject/main > backup.json`.
