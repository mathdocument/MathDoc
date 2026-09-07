---
title: Web interface
---

Open `http://127.0.0.1:7599` after starting `mdc serve`. Navigate by search, referrers, dependencies or the graph view. Nodes are created with a title; no filename is requested.

Lean blocks use Monaco on the left and native Infoview on the right, including goals, diagnostics and interactive widgets. Save writes to the database. Save & check waits for server diagnostics and strict dependency certification. Save & build also generates `.olean`. Each editor keeps an isolated draft and dependency snapshot; reload its environment after dependencies or project configuration change.

The Lean project toolbar button edits the pinned toolchain, Lake TOML and lock manifest. Text, Rocq and LaTeX retain their existing editors. Only Lean has a language server in this release. The MathDoc VS Code extension is retired.
