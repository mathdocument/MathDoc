---
title: Architecture
---

`store.rs` maps typed Node links and Lean project documents to TerminusDB HTTP transactions. A database data-version guards each write. `service.rs` serves the browser, API and Lean WebSocket sessions. The CLI is an HTTP client except for `init` (database creation) and `serve` (hosting the service). `config.rs` reads user-level settings; `cli.rs` is the single CLI implementation.

`Snapshot` contains a disposable graph projection, topological depths, reverse adjacency and transitive Lean input keys. Writes update the projection; external commits trigger reloads. The current projection also holds complete source blocks in memory. Startup and external-commit reload cost therefore scales with total source size.

`lean.rs` generates only requested compiler inputs and manages native Lean Server/Lake. Browser drafts are isolated from each other and from saved sources. This is process/state separation, not an operating-system sandbox: authors must be trusted local users.
