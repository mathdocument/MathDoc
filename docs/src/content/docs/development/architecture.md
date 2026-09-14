---
title: Architecture
---

`store.rs` maps typed node links and Lean project documents to TerminusDB HTTP
transactions. Data-version guards protect graph and configuration writes.
`gateway.rs` serves the project directory and shared browser assets, and streams
branch HTTP requests and native Lean WebSockets to independent branch processes.
`service.rs` owns each branch's JSON API and native Lean sessions.
`cli.rs` is the single public command parser; `config.rs` reads user settings.

The CLI is an HTTP client for branch and node operations, except `branch del`,
which must delete a stopped branch directly in TerminusDB. `init` creates a
database, `status` reads entry/branch inventory, `start` launches/reuses the entry
server and optionally starts a branch, and `stop` uses the selected local record
to request shutdown. See the
[CLI/API mapping](../../reference/http-api/).

## Local service lifecycle

Status reads TerminusDB metadata and existing cache leases. It does not load node
graphs, scan source workspaces or launch compilers. The entry server binds a
loopback listener on port 17843 by default and owns the endpoint's `.gateway`
lease. Each branch binds port zero internally, loads its graph and publishes
its private port, PID and random service token in its own lease. The parent
waits up to 180 seconds for each launched process to become ready.

Background launch is encapsulated in `start`: the process re-executes the public
command with an inherited Unix socket carrying a private bootstrap marker and
internal port queued before process creation. Ordinary stdin sockets, including Node.js subprocess
pipes, do not activate this path or wait for a marker.
There is no hidden server subcommand or shell launch script. The child creates a
new process session and runs from its branch cache or `.gateway` directory,
independent of the caller's working directory. Stderr goes to `service.log` there.

Client `-p/--proj DATABASE/BRANCH` resolves local entry and branch records without
querying the database. It addresses `/p/DATABASE/BRANCH/api/...` at the entry
server. Requests carry the branch service token so a replaced lease cannot
silently select a different process. Stop requests graceful shutdown at the direct
private address and waits for the original lease to be released; it also works
when the entry server is stopped. A crashed owner releases its OS
lock; a leftover record is ignored and cleared by the next owner.

Locks and discovery are scoped to the configured cache root. Branch deletion
acquires that same lock, clears artifacts and logs, and removes the database
branch reference. It keeps the lock inode so a concurrent start cannot bypass
coordination. Discovery requires neither a port scan nor a registry database.

The entry reads the requested branch lease on each API request. It strips the
project prefix, preserves encoded paths and queries, rewrites Host/Origin for
loopback and binds the forwarded request to the current branch token. Incoming
Host/Origin checks run first. Bodies stream through the existing HTTP client;
WebSocket upgrades use a bidirectional byte tunnel. Starting/stopping branches
needs no route reload and leaves other branch sessions connected. Graph state,
locks and Lean budgets remain per branch. Stopping the entry disconnects its
browser connections but leaves branch processes running.

## Graph and Lean state

`Snapshot` holds a disposable graph projection, topological depths, reverse
adjacency and transitive Lean input keys. Service writes update it; requests
check the database commit and reload after external commits. The current
projection also contains complete source blocks in memory. Immutable nodes and project configuration are shared with Lean requests, avoiding copies of source closures for each editor. Startup and external
reload costs therefore scale with total source size, even though ordinary
commands no longer pay filesystem synchronization costs.

`lean.rs` generates requested compiler inputs and manages native Lean Server and
Lake. Browser drafts are isolated from each other and saved sources, while
artifacts are shared within a branch. This is process/state separation, not an
operating-system sandbox. See [Compiler internals](../compiler-internals/) and
[Graph and compilation caches](../index-cache/).
