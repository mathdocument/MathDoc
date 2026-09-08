---
title: Web interface
---

Open `http://127.0.0.1:7599` after starting `mdc serve DATABASE`. Navigate by search, referrers, dependencies or the graph view. Nodes are created with a title; no filename is requested.

Lean blocks use Monaco on the left and native Infoview on the right, including goals, diagnostics and interactive widgets. Save writes to the database. Save & check waits for server diagnostics and strict dependency certification. Save & build also generates `.olean`. Each tab keeps an isolated draft environment and one Lean connection across node and layout changes. The two most recently used Lean documents retain their native workers. Selecting a node refreshes its dependency snapshot and restarts the file worker if dependencies changed; reload the environment after project configuration changes. The editor status follows native Lean progress separately from the saved check/build result. New or evicted documents still need to load their imports.

The Lean project toolbar button edits the pinned toolchain, Lake TOML and lock manifest. All block types share a header, save/delete controls, collapse behavior and status styling. Text, Rocq and LaTeX use CodeMirror; Lean retains Monaco and Infoview. Only Lean has a language server in this release.
