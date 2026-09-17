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
Nodes are created with a title; no filename is requested. In Graph view, the
editor sidebar uses a fixed 3/8 of the available width and the graph uses 5/8.
Narrow screens retain the stacked layout. Switching views preserves the active
Lean session and unsaved edits.

**Lean import** is inside the Lean source block and shows the module name other
nodes should import. Graph nodes share the Lean status colors: gray for no Lean
code, yellow for unchecked/failed checks or reported sorry, and green for a
successful check without reported sorry. Rocq does not affect graph colors. Knowledge view cards show both Lean and Rocq
status lights after the node ID and depth, refreshed after saved edits and Lean
certification. The bottom bar shows the node ID, status dot and title; long titles
use an ellipsis and show their full text on hover.

Lean blocks initially show a local Monaco editor with syntax highlighting, editing and **Save**, without starting a Lean server or reserving a server slot. Click **Start Lean server** (the play icon) to enable checking and native Infoview on the right, including goals, diagnostics and interactive widgets. This button then becomes **Recheck Lean**, which reopens the current file for checking in the existing server connection. **Stop Lean server**, immediately to its right, closes only this page's session and returns to local editing. Other pages and CLI checks are unaffected. Both modes use the same Lean grammar, font and light/dark themes; unsaved text survives starting, checking and stopping.

**Save** writes to the database. Once started, the editor automatically certifies the saved version using its existing diagnostics and native import information, then refreshes the node status. Successfully compiled dependencies are certified from their compiler metadata with the same strict dependency checks. Unsaved drafts never certify a different database version. There is no separate check/build button or second checker on save. Saving without a server does not perform a Lean check; use the start button or CLI `lean check` when needed.

After explicit startup, each tab keeps an isolated draft environment and one Lean connection across node and layout changes. The two most recently used Lean documents retain their native workers. Selecting or rechecking a node refreshes its dependency snapshot; changed dependencies restart the file worker. Stop and start after project toolchain or Lake configuration changes. New or evicted documents still load imports, while Lake's shared artifact cache retains compiled dependencies after the tab closes. Reopening or reloading the page returns to local editing until started again. The target `.olean` is generated when imported or explicitly requested through CLI `--build`.

Switching nodes cancels any older preparation request. “Preparing Lean
environment…” means the selected source is displayed while its native file is
being attached; it does not indicate a full project build. Preparation that takes
more than 30 seconds triggers one automatic reconnect with the unsaved draft
retained. If recovery fails again, stop and start the server. Cold imports can
still take longer and appear separately as “Loading Lean imports…”.

The **Project settings → Lean** tab edits the pinned toolchain, Lake configuration (TOML or Lean) and lock manifest, preserving supporting project files and the module namespace. All block types share a header, save/delete controls, collapse behavior and status styling. Text, Rocq and LaTeX use CodeMirror; Lean retains Monaco and Infoview. Only Lean has a language server in this release. The **LaTeX** settings tab accepts shared macros and a bibliography; LaTeX blocks provide Edit/Preview, scoped reference and citation completion, and clickable cross-node references. See [LaTeX previews](../latex/).
