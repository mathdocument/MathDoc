---
title: Web Interface
description: Navigate and edit a workspace through MathDoc's local browser interface.
---

`mdc serve` starts a local HTTP server for the embedded single-page application and
prints its URL.

```bash
mdc serve
mdc serve notes/theorem.mdoc
mdc serve --bind 127.0.0.1:7878
```

The default bind is `127.0.0.1:0`, where port `0` asks the operating system for a free
port. Open the printed URL in a browser. `--bind` must be a numeric loopback socket such
as `127.0.0.1:7878` or `[::1]:7878`; hostnames such as `localhost:7878` are rejected as
bind values.

Without an initial source, the interface opens the deepest root in the same ordering as
`mdc graph roots`. The optional source accepts a path, exact fnode, unique prefix, or
unique case-insensitive exact title.

## Knowledge view

The primary view uses three columns to keep the focused node, its referrers, and its
dependencies visible together. It supports:

- title and source-block editing
- source block creation and removal
- dependency search, addition, and removal
- standalone node creation through **New node**
- create-and-link dependency creation through **Add dependency** when no candidate matches
- search and reference navigation

Press `/` to open node search. Press `Ctrl/Cmd+S` or `Ctrl/Cmd+Enter` to save the
focused source block. Shortcuts are ignored while ordinary text input should take
precedence. Block deletion asks for confirmation, and dirty or pending drafts are
guarded during navigation, refresh, view switching, and page unload.

Search performs a case-insensitive literal substring match over title and fnode. Fnode
prefix matches rank first, followed by earlier title matches, shorter titles, and path.
The browser waits 120 ms before requesting up to 50 results. Broken results remain
visible for diagnosis but cannot be opened.

Dependency candidate search uses the same matching order while excluding the focused
node, existing direct dependencies, and invalid or duplicate nodes. Create-and-link is
offered only when there are no matches, not when all matches were excluded or hidden by
the result limit.

## Graph view

Press `g` to switch between Knowledge and Graph views. The graph view loads the valid
full graph on first use and renders a deterministic, static layout grouped by
topological depth. Layers run from deeper to shallower nodes; titles and fnodes provide
stable ordering within each layer. Node radius incorporates the smoothed
in-degree/out-degree ratio, making highly referenced nodes more prominent.

## Local security boundary

The server accepts only numeric loopback bind addresses. Requests require a loopback or
`localhost` Host header, including names ending in `.localhost`, and the API does not
grant cross-origin access.

:::danger[No authentication]
Any local client able to send a valid HTTP request can mutate the workspace. Do not
proxy or expose `mdc serve` to a network, and do not treat loopback binding as user
authentication.
:::

Writes share the same snapshot checks, workspace lock, index, and graph mutation rules
as the CLI. An `AppState`-scoped mutex shared by all handlers on the server router
serializes each browser write's mutation critical section; the workspace lock is the
workspace-wide and interprocess boundary.
