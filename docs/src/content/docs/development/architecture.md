---
title: Architecture
---

`store.rs` maps typed Node links and Lean project documents to TerminusDB HTTP transactions. A database data-version guards each write. `service.rs` serves the browser, API and Lean WebSocket sessions. The CLI is an HTTP client except for `status` (database/branch inventory and local service ports), `init` (database creation), `start` (launching the background service) and `stop` (local shutdown). `config.rs` reads user-level settings; `cli.rs` is the single CLI implementation.

Status reads TerminusDB's database metadata and the existing branch cache lease;
it does not load node graphs, scan source workspaces or launch compilers. Start
binds a loopback listener directly (port zero lets the OS choose), loads the branch,
and publishes its port, PID and random service token in the private held lease.
The parent waits for readiness before returning. A stopped or crashed owner releases
the OS lock, so its leftover record is ignored; a new owner clears it during startup.

Client `--proj DATABASE/BRANCH` resolves this record without a database request.
Requests carry the service token so a reused port cannot silently target another
mdc service. Stop requires the token, requests graceful shutdown over the existing
HTTP connection, and waits for the original lease to be released. Background stderr
goes to `service.log` in the branch cache. No global daemon or port scan is needed.

`Snapshot` contains a disposable graph projection, topological depths, reverse adjacency and transitive Lean input keys. Writes update the projection; external commits trigger reloads. The current projection also holds complete source blocks in memory. Startup and external-commit reload cost therefore scales with total source size.

`lean.rs` generates only requested compiler inputs and manages native Lean Server/Lake. Browser drafts are isolated from each other and from saved sources. This is process/state separation, not an operating-system sandbox: authors must be trusted local users.
