---
title: Lean Server integration
---

The service runs native `lake serve` with JSON-RPC Content-Length framing. It sends versioned `didOpen`/`didChange` messages and waits for `textDocument/waitForDiagnostics`. Diagnostics are associated with document versions, including incremental diagnostic notifications.

Managed direct imports come from `$/lean/prepareModuleHierarchy` and `$/lean/moduleHierarchy/imports`. The graph dependency set is compared with this native information. `$/lean/plainGoal` powers CLI goal queries; the browser forwards the full native protocol for Infoview, completion, hover and widgets.

Dependency changes reopen the importing document so Lean reloads its import environment. Proof edits preserve the live worker and its elaboration snapshots. Native `lake build +MODULE` generates target artifacts on an explicit build request. Compiler processes are terminated with their process groups on cancellation/disconnect.
