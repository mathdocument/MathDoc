---
title: Web interface
---

Run `mdc start DATABASE/BRANCH` and open its printed `/p/DATABASE/BRANCH/` URL.
The root page at `http://127.0.0.1:17843/` lists projects and branches, with
**Running** and **Stopped** labels, search, status filters and links to running
branches. It refreshes every five seconds while visible. Start or stop branches
with the CLI; `mdc status` returns the same inventory and access URLs.

The editor toolbar identifies the current branch. Click the MathDoc logo to
return to **All projects**; unsaved drafts require confirmation before leaving.
Within a branch, navigate by search, referrers, dependencies or the graph view.
Nodes are created with a title; no filename is requested. In Graph view, drag the
vertical divider to resize the editor sidebar. Focus the divider and use the
arrow keys, Home or End for keyboard control. Resizing preserves the active Lean
session and unsaved edits; narrow screens retain the stacked layout.

**Lean import** is inside the Lean source block and shows the module name other
nodes should import. Graph nodes share the Lean status colors: gray for no Lean
code, yellow for unchecked/failed checks or reported sorry, and green for a
successful check without reported sorry. Rocq does not affect graph colors. Knowledge view cards show both Lean and Rocq
status lights after the node ID and depth, refreshed after saved edits and Lean
certification. The bottom bar shows the node ID, status dot and title; long titles
use an ellipsis and show their full text on hover.

Lean blocks use Monaco on the left and native Infoview on the right, including goals, diagnostics and interactive widgets. **Save** writes to the database. The editor automatically certifies the saved version using its existing diagnostics and native import information, then refreshes the node status. Successfully compiled dependencies are certified from their compiler metadata with the same strict dependency checks. Unsaved drafts never certify a different database version. There is no separate check/build button or second checker on save.

Each tab keeps an isolated draft environment and one Lean connection across node and layout changes. The two most recently used Lean documents retain their native workers. Selecting a node refreshes its dependency snapshot and restarts the file worker if dependencies changed; reload the environment after project configuration changes. New or evicted documents still load imports, while Lake's shared artifact cache retains compiled dependencies after the tab closes. The target `.olean` is generated when imported or explicitly requested through CLI `--build`.

Switching nodes cancels any older preparation request. “Preparing Lean
environment…” means the selected source is displayed while its native file is
being attached; it does not indicate a full project build. Preparation that takes
more than 30 seconds triggers one automatic reconnect with the unsaved draft
retained. If recovery fails again, use **Reload environment**. Cold imports can
still take longer and appear separately as “Loading Lean imports…”.

The Lean project toolbar button edits the pinned toolchain, Lake configuration (TOML or Lean) and lock manifest, preserving supporting project files and the module namespace. All block types share a header, save/delete controls, collapse behavior and status styling. Text, Rocq and LaTeX use CodeMirror; Lean retains Monaco and Infoview. Only Lean has a language server in this release.
