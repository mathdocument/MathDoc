---
title: Lean Server integration
---

The service runs native `lake serve` with JSON-RPC Content-Length framing. It sends versioned `didOpen`/`didChange` messages and waits for `textDocument/waitForDiagnostics`. Diagnostics are associated with document versions, including incremental diagnostic notifications.

CLI checks open only the requested target. Lake builds its necessary imports,
then the same native artifact-certification path used by the browser validates
their dependency edges. Already certified input keys with complete sorry evidence
skip repeated metadata reads; their recorded imports still lead to any incomplete
transitive certificates. A dependency is not elaborated again in a separate LSP worker merely to
refresh its status. Missing or mismatched evidence remains unverified.

Each branch has a CLI worker pool controlled by `lean_cli_workers` (default 4).
Requests first reuse certificates or wait for an identical input's in-flight check,
then acquire a worker through a FIFO semaphore. No client session or agent identity
is required. Each worker retains one LSP and one hot document, with private mutable
sources and build paths. Workers initialize on demand and remain reusable; cached
checks require no worker. Goal queries use the same pool. Interrupted LSP requests
discard their protocol state before the slot can be reused.

The first worker retains `projects/PROJECT_KEY`; additional workers use its
`.workers/SLOT` subdirectories. They share the pinned package checkout and native
artifact pool with editor workspaces. Certification keeps request-local evidence
so concurrent checks of different revisions cannot overwrite each other's inputs
while publishing results. Branch shutdown closes the queue and shuts down all
worker transports. Browser sessions have a separate `lean_web_sessions` limit
(default 4); they do not consume CLI slots. Both limits are per branch and can be
adjusted in the host or deployment `config.toml`.

The browser bridge forwards framed JSON without constructing a recursive JSON tree.
URI values are translated independently, so deeply nested Infoview expressions do
not hit a 128-level deserialization limit. JSON syntax, frame headers and the
32 MiB message bound are still checked. Client input and native output are driven
independently through bounded queues, including while custom requests prepare
files or certify saved inputs. A blocked write cannot prevent output from draining.
A browser that stops reading messages is disconnected after 30 seconds.

Opening a node initializes only browser-side Monaco with an in-memory model.
The user must click **Start Lean server** to create a session and connect the
native bridge; local highlighting, editing and saving need no native process.
Once started, Monaco displays the selected source before waiting for Lean initialization or
environment preparation. New files temporarily use an in-memory Lean model,
which does not start another Lean worker. When preparation finishes, the native
file model takes over with the current draft and cursor intact. Superseded
selection requests are cancelled locally and through the LSP cancellation token;
the latest selection proceeds without waiting for an older reply. Native model
attachment still checks cancellation before changing the visible editor. Selection
preparation has a 30-second deadline, including client initialization and model
attachment. A timeout uses the same draft-preserving reconnect path as a broken
connection. Backend editor requests also have a 30-second deadline. These deadlines
do not limit native proof elaboration or cold import loading.

Two native document workers remain warm; cold or evicted documents still need to
load their imports. Hidden editors retain their last nonzero layout so restoring
a block does not corrupt its viewport or hide line-one diagnostic markers.

Editor and CLI processes enable Lake's native content-addressed artifact cache.
Each editor links `.lake/cache` to its database branch's canonical project cache,
so compiled dependencies survive temporary-session deletion. Lake validates the
source, toolchain and transitive build inputs before restoring artifacts. Standard
artifact paths are restored for metadata readers; writable draft sources and live
document environments remain isolated.

An unexpected connection failure triggers one automatic session reconnect. The
parent editor retains unsaved text and restores it into the new runtime without
saving it to the database. Repeated failures require stopping and starting the
page's server, which also preserves the draft. Recheck reopens only the selected
native document in the same language client; it does not replace the session or
restart `lake serve`. An explicit stop cancels reconnects and pending starts.
The LSP client's own reconnect is disabled: a consumed session socket cannot be
reused. Connection closure during initialization and transport-factory failures
enter this same recovery path immediately, instead of waiting for the selection
deadline and misreporting a native process exit as a preparation timeout.

Managed direct imports come from `$/lean/prepareModuleHierarchy` and `$/lean/moduleHierarchy/imports`. The graph dependency set is compared with this native information. `$/lean/plainGoal` powers CLI goal queries; the browser forwards the full native protocol for Infoview, completion, hover and widgets.

Saved editor documents publish certification through the same connection. The
bridge observes native diagnostics, `waitForDiagnostics` completion and module
imports; it binds them to the exact saved source, LSP document version and
dependency environment. Full document synchronization keeps that source binding
exact without a second text-editing engine. Replies from closed workers cannot
certify reopened documents. The browser cannot supply its own success verdict.

Successfully imported managed dependencies provide `.olean` and `.ilean`
compiler metadata. Their direct imports undergo the same graph dependency checks
as CLI diagnostics. Only observed, compiled dependencies are certified; a graph
edge alone is insufficient. These certificates share the CLI's versioned memory
and disk cache, so a subsequent check needs no second elaboration. Save does not
force a target artifact build; native Lake restores or builds it when required.

Certificates retain `has_sorry`: native LSP warnings supply it for opened modules,
and nonsynthetic Lake `.trace` warning logs supply it for compiled imports.
Lake cache restoration produces synthetic traces without those logs. For these
imports the service batches a read of existing `.olean` proof bodies using the
project's pinned Lean, including `.olean.server` and `.olean.private` for modules.
It neither rebuilds nor elaborates these dependencies. Results are persisted in
the same certificates, so this recovery is not repeated on subsequent checks.
Missing or unreadable evidence stays `null` and cannot produce a green UI status.
A cached target cannot bypass incomplete certificates in its dependency closure.
Editor certification refreshes the graph after these dependency statuses are saved;
users need not visit each dependency. Uncompiled reverse dependents are not
automatically certified by checking a node they import.

`certified` still means compilation and
dependency matching succeeded, so admitted declarations remain usable during
staged formalization. Direct sorry evidence produces red Sorry. The service
propagates evidence through current managed dependencies in topological order:
a sorry-free module depending on an admitted node is yellow Conditional, including
through multiple dependency levels. Green Verified requires complete sorry-free
evidence throughout that closure. Missing or stale evidence is gray Unverified.
This derived dependency state is recomputed from certificates when restoring a
graph, including existing certificates, and adds no compiler work to graph queries.
The warning-based paths respect configured `warn.sorry` behavior; artifact
inspection detects direct uses even with warnings disabled. This is not a full
axiom audit of external libraries.

Dependency changes reopen the importing document so Lean reloads its import environment. Proof edits preserve the live worker and its elaboration snapshots. Native `lake build +MODULE` generates target artifacts on an explicit build request. On disconnect, the shared CLI/browser transport sends LSP `shutdown` and `exit`, draining stdout until Lean has reaped its workers (which use separate process groups). The writer gets up to one second to finish queued frames before the two-second shutdown handshake. A broken or unfinished frame forces termination without appending shutdown bytes to it. Service shutdown waits for editor and CLI cleanup before stopping the async runtime.
