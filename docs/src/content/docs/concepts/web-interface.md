---
title: Web Interface
description: Navigate and edit a workspace through MathDoc's local browser interface.
---

`mdc serve` starts a local HTTP server and opens the embedded single-page application.

```bash
mdc serve
mdc serve notes/theorem.mdoc
mdc serve --bind 127.0.0.1:7878 --no-open
```

Without an initial source, the interface opens the deepest root in the same ordering as
`mdc graph roots`. The optional source accepts a path, exact fnode, unique prefix, or
unique case-insensitive exact title.

## Knowledge view

The primary view uses three columns to keep the focused node, its referrers, and its
dependencies visible together. It supports:

- title and source-block editing
- source block creation and removal
- dependency search, addition, and removal
- child node creation
- search and reference navigation

Press `/` to open node search. Press `Ctrl/Cmd+S` or `Ctrl+Enter` to save the focused
source block. Shortcuts are ignored while ordinary text input should take precedence.

## Graph view

Press `g` to switch between Knowledge and Graph views. The graph view loads the valid
full graph on first use and runs a force-directed layout. Node radius incorporates the
smoothed in-degree/out-degree ratio, making highly referenced nodes more prominent.

## Local security boundary

The server accepts only numeric loopback bind addresses. Requests require a loopback or
`localhost` Host header, and the API does not grant cross-origin access.

:::danger[No authentication]
Any local client able to send a valid HTTP request can mutate the workspace. Do not
proxy or expose `mdc serve` to a network, and do not treat loopback binding as user
authentication.
:::

Writes share the same snapshot checks, workspace lock, index, and graph mutation rules
as the CLI. A process-wide mutation mutex serializes complete browser write operations.
