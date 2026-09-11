---
title: Lean Server integration
---

The service runs native `lake serve` with JSON-RPC Content-Length framing. It sends versioned `didOpen`/`didChange` messages and waits for `textDocument/waitForDiagnostics`. Diagnostics are associated with document versions, including incremental diagnostic notifications.

The browser bridge forwards framed JSON without constructing a recursive JSON tree.
URI values are translated independently, so deeply nested Infoview expressions do
not hit a 128-level deserialization limit. JSON syntax, frame headers and the
32 MiB message bound are still checked.

Monaco displays the selected source before waiting for Lean initialization or
environment preparation. New files temporarily use an in-memory Lean model,
which does not start another Lean worker. When preparation finishes, the native
file model takes over with the current draft and cursor intact. Superseded
selection responses cannot replace the visible node. Two native document workers
remain warm; cold or evicted documents still need to load their imports.

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

Dependency changes reopen the importing document so Lean reloads its import environment. Proof edits preserve the live worker and its elaboration snapshots. Native `lake build +MODULE` generates target artifacts on an explicit build request. On disconnect, the shared CLI/browser transport sends LSP `shutdown` and `exit`, draining stdout until Lean has reaped its workers (which use separate process groups). A two-second timeout retains forced termination for an unresponsive server. Service shutdown waits for editor and CLI cleanup before stopping the async runtime.
