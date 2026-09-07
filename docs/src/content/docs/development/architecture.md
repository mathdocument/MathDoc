---
title: Architecture
---

`store.rs` maps typed Node links and Lean project documents to TerminusDB HTTP transactions. A database data-version guards each write. `service.rs` serves the browser, API and Lean WebSocket sessions. The CLI is an HTTP client except for database initialization and explicit import decoding.

`Snapshot` contains a disposable graph projection, topological depths, reverse adjacency and transitive Lean input keys. Writes update the projection; external commits trigger reloads. Source blocks remain in database documents.

`lean.rs` generates only requested compiler inputs and manages native Lean Server/Lake. Browser drafts are isolated from each other and from saved sources. This is process/state separation, not an operating-system sandbox: authors must be trusted local users.
