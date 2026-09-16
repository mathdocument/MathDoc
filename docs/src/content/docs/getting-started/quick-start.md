---
title: Quick start
---

Complete [Installation](../installation/), then create and start a project:

```sh
mdc init myproject
mdc start myproject/main
```

Open the printed `/p/myproject/main/` URL and use the **+** button (**Create node**). The root page
`http://127.0.0.1:17843/` lists all projects and their branch service states.
Add text, Lean, Rocq or LaTeX blocks.
Lean opens the source editor and native Infoview side by side. The toolbar's Lean
project dialog configures the pinned toolchain and external libraries.

CLI agents use the same branch service from any directory:

```sh
mdc new -p myproject/main -t 'Example'
printf 'theorem exampleA : True := by trivial\n' | mdc edit Example -p myproject/main --type lean
mdc lean check Example -p myproject/main
mdc graph check -p myproject/main -m
mdc export -p myproject/main > backup.json
```

The editor checks as you type. **Save** stores the source; once that exact version
finishes checking, its status and compiled dependencies update automatically.
Editor certificates and Lake artifacts are shared with CLI checks. Use
`mdc lean check Example -p myproject/main --build` when you need the target
`.olean` immediately; imports build it on demand.

Read a node with `show` and pass its `--revision` on later writes when another
agent may edit concurrently. See [Source workflow](../../concepts/source-workflow/)
and the complete [CLI reference](../../reference/workspace-commands/).

`mdc status` returns entry and branch states with access URLs. Finish with
`mdc stop myproject/main`; graph data and caches remain available for the next
start. Optionally run `mdc stop` to stop the entry server as well.
[Export and restore](../../concepts/import-export/)
covers backups and copying the graph to a new database.
