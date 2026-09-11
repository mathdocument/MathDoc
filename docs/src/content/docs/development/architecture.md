---
title: Architecture
---

`store.rs` maps typed node links and Lean project documents to TerminusDB HTTP
transactions. Data-version guards protect graph and configuration writes.
`service.rs` serves the browser, JSON API and native Lean WebSocket sessions.
`cli.rs` is the single public command parser; `config.rs` reads user settings.

The CLI is an HTTP client for branch and node operations, except `branch del`,
which must delete a stopped branch directly in TerminusDB. `init` creates a
database, `status` reads database/branch inventory, `start` launches the service,
and `stop` uses the local service record to request shutdown. See the
[CLI/API mapping](../../reference/http-api/).

## Local service lifecycle

Status reads TerminusDB metadata and existing cache leases. It does not load node
graphs, scan source workspaces or launch compilers. Start binds a loopback
listener directly; port zero internally lets the OS choose an available port.
It loads the branch and publishes the port, PID and random service token in the
private held lease. The parent waits up to 180 seconds for readiness.

Background launch is encapsulated in `start`: the process re-executes the public
command with an inherited Unix socket carrying a private bootstrap marker queued
before process creation. Ordinary stdin sockets, including Node.js subprocess
pipes, do not activate this path or wait for a marker.
There is no hidden server subcommand or shell launch script. The child creates a
new process session and runs from its branch cache, independent of the caller's
working directory. Stderr goes to `service.log` there.

Client `-p/--proj DATABASE/BRANCH` resolves the local record without querying the
database. Requests carry a service token so port reuse cannot silently select a
different mdc process. Stop requests graceful shutdown over that HTTP connection
and waits for the original lease to be released. A crashed owner releases its OS
lock; a leftover record is ignored and cleared by the next owner.

Locks and discovery are scoped to the configured cache root. Branch deletion
acquires that same lock, clears artifacts and logs, and removes the database
branch reference. It keeps the lock inode so a concurrent start cannot bypass
coordination. No global daemon or port scan is needed.

## Graph and Lean state

`Snapshot` holds a disposable graph projection, topological depths, reverse
adjacency and transitive Lean input keys. Service writes update it; requests
check the database commit and reload after external commits. The current
projection also contains complete source blocks in memory. Startup and external
reload costs therefore scale with total source size, even though ordinary
commands no longer pay filesystem synchronization costs.

`lean.rs` generates requested compiler inputs and manages native Lean Server and
Lake. Browser drafts are isolated from each other and saved sources, while
artifacts are shared within a branch. This is process/state separation, not an
operating-system sandbox. See [Compiler internals](../compiler-internals/) and
[Graph and compilation caches](../index-cache/).
