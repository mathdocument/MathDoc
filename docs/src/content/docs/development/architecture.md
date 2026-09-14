---
title: Architecture
---

`store.rs` maps typed node links and Lean project documents to TerminusDB HTTP
transactions. Data-version guards protect graph and configuration writes.
`server.rs` owns the single HTTP listener, branch registry and project directory.
`service.rs` owns each branch's graph, JSON API and native Lean sessions.
`cli.rs` is the public parser/client; `config.rs` reads host settings.

The CLI uses the shared server for branch and node operations. `init`, `status`
and stopped-branch deletion access TerminusDB directly. See the
[CLI/API mapping](../../reference/http-api/).

## Server and branch lifecycle

`mdc start` launches or reuses one background process on loopback port 17843 by
default. The process owns `ENDPOINT_HASH/.server/service.lock` and writes its
runtime log beside that record. `mdc start DATABASE/BRANCH` loads the branch into
this process through an authenticated local control request. The branch owns its
existing cache lease and records the same server PID/port with its own random
token. Native Lean workers remain child processes; there are no per-branch HTTP
listeners, backend processes or proxy connections.

Background launch re-executes `mdc start` with an inherited Unix socket carrying
a private bootstrap marker and the listening port. Ordinary stdin sockets do
not activate this path. The child detaches from the terminal and runs from the
server cache, independent of the caller's directory. No hidden CLI command or
shell wrapper is involved. Startup allows up to 180 seconds for readiness.

The registry holds a short lock only to reserve, publish or look up a branch;
graph loading and Lean cleanup happen outside it. Management tasks finish even
if the requesting client disconnects. The server strips the project path prefix
and calls the existing Axum router directly, preserving encoded paths, queries
and the native WebSocket upgrade. Host/Origin checks run before dispatch; branch
tokens reject CLI requests from a previous load. No request body is copied into
a second HTTP request.

`mdc stop DATABASE/BRANCH` makes that branch unavailable, cancels its pending API
requests, closes its browser sessions and shuts down its CLI Lean worker. Request
lifetimes are drained before editor cleanup, preventing late session creation.
Other branches continue operating. Bare `mdc stop` also waits for management
operations and closes every branch before the server exits. Save drafts and wait
for writes before stopping; a request interrupted during a database transaction
may require reading back its result. Restarting the server starts no branches
implicitly: load the desired branches with `mdc start DATABASE/BRANCH`.

`status` reads TerminusDB inventory and held cache leases without loading graphs
or starting Lean. When the server exits or crashes, all branches are stopped.
Data and compiled artifacts remain on disk. One consistent cache root is
required for discovery and exclusive ownership. Branch deletion takes the same
cache lock and retains its inode to prevent concurrent starts bypassing it.

## Graph and Lean state

`Snapshot` holds a disposable graph projection, topological depths, reverse
adjacency and transitive Lean input keys. Service writes update it; requests
check the database commit and reload after external commits. The current
projection also contains complete source blocks in memory. Immutable nodes and project configuration are shared with Lean requests, avoiding copies of source closures for each editor. Startup and external
reload costs therefore scale with total source size, even though ordinary
commands no longer pay filesystem synchronization costs.

`lean.rs` generates requested compiler inputs and manages native Lean Server and
Lake. Browser drafts are isolated from each other and saved sources, while
artifacts are shared within a branch. This is state separation, not an
operating-system sandbox. See [Compiler internals](../compiler-internals/) and
[Graph and compilation caches](../index-cache/).
