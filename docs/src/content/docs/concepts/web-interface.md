---
title: Web interface
---

Run `mdc start DATABASE/BRANCH` and open the URL it prints. Use `mdc status` to find the port again. Navigate by search, referrers, dependencies or the graph view. Nodes are created with a title; no filename is requested.

Lean blocks use Monaco on the left and native Infoview on the right, including goals, diagnostics and interactive widgets. **Save** writes to the database. The editor automatically certifies the saved version using its existing diagnostics and native import information, then refreshes the node status. Successfully compiled dependencies are certified from their compiler metadata with the same strict dependency checks. Unsaved drafts never certify a different database version. There is no separate check/build button or second checker on save.

Each tab keeps an isolated draft environment and one Lean connection across node and layout changes. The two most recently used Lean documents retain their native workers. Selecting a node refreshes its dependency snapshot and restarts the file worker if dependencies changed; reload the environment after project configuration changes. New or evicted documents still load imports, while Lake's shared artifact cache retains compiled dependencies after the tab closes. The target `.olean` is generated when imported or explicitly requested through CLI `--build`.

The Lean project toolbar button edits the pinned toolchain, Lake TOML and lock manifest. All block types share a header, save/delete controls, collapse behavior and status styling. Text, Rocq and LaTeX use CodeMirror; Lean retains Monaco and Infoview. Only Lean has a language server in this release.
