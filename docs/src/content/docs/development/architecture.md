---
title: Architecture
---

`store.rs` maps typed Node links and Lean project documents to TerminusDB HTTP transactions. A database data-version guards each write. `service.rs` serves the browser, API and Lean WebSocket sessions. The CLI is an HTTP client except for `status` (database/branch inventory and local service ports), `init` (database creation) and `serve` (hosting the service). `config.rs` reads user-level settings; `cli.rs` is the single CLI implementation.

Status reads TerminusDB's database metadata and the existing branch cache lease;
it does not load node graphs, scan source workspaces or launch compilers. Serve
publishes its bound socket address in the held lease. A stopped or crashed owner
releases the OS lock, so its leftover address is ignored. A new owner clears that
address before binding a listener.

`Snapshot` contains a disposable graph projection, topological depths, reverse adjacency and transitive Lean input keys. Writes update the projection; external commits trigger reloads. The current projection also holds complete source blocks in memory. Startup and external-commit reload cost therefore scales with total source size.

`lean.rs` generates only requested compiler inputs and manages native Lean Server/Lake. Browser drafts are isolated from each other and from saved sources. This is process/state separation, not an operating-system sandbox: authors must be trusted local users.
