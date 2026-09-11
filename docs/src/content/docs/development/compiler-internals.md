---
title: Lean Server integration
---

The service runs native `lake serve` with JSON-RPC Content-Length framing. It sends versioned `didOpen`/`didChange` messages and waits for `textDocument/waitForDiagnostics`. Diagnostics are associated with document versions, including incremental diagnostic notifications.

CLI checks open only the requested target. Lake builds its necessary imports,
then the same native artifact-certification path used by the browser validates
their dependency edges. Already certified input keys skip repeated metadata
reads. A dependency is not elaborated again in a separate LSP worker merely to
refresh its status. Missing or mismatched evidence remains unverified.

The browser bridge forwards framed JSON without constructing a recursive JSON tree.
URI values are translated independently, so deeply nested Infoview expressions do
not hit a 128-level deserialization limit. JSON syntax, frame headers and the
32 MiB message bound are still checked. Client input and native output are driven
independently through bounded queues, including while custom requests prepare
files or certify saved inputs. A blocked write cannot prevent output from draining.
A browser that stops reading messages is disconnected after 30 seconds.

Monaco displays the selected source before waiting for Lean initialization or
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
saving it to the database. Repeated failures require “Reload environment”, which
also preserves the draft.

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
Missing evidence is `null`, which cannot produce a green UI status and requires
a native check before certificate reuse. `certified` still means compilation and
dependency matching succeeded, so admitted declarations remain usable during
staged formalization. The green status additionally requires `has_sorry: false`.
This uses compiler warnings, including their configured `warn.sorry` behavior;
it is not an axiom audit. Graph colors do not propagate admitted assumptions
through downstream nodes.

Dependency changes reopen the importing document so Lean reloads its import environment. Proof edits preserve the live worker and its elaboration snapshots. Native `lake build +MODULE` generates target artifacts on an explicit build request. On disconnect, the shared CLI/browser transport sends LSP `shutdown` and `exit`, draining stdout until Lean has reaped its workers (which use separate process groups). The writer gets up to one second to finish queued frames before the two-second shutdown handshake. A broken or unfinished frame forces termination without appending shutdown bytes to it. Service shutdown waits for editor and CLI cleanup before stopping the async runtime.
