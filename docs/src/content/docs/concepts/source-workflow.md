---
title: Source workflow
---

Edit blocks in the browser or send source on stdin to `mdc edit --proj myproject/main NAME --type lean`. Accepted types are text, lean, rocq and latex. Python editing is removed.

Browser mutations carry a node revision. CLI agents can use `--revision` from `mdc show --proj myproject/main` for the same optimistic concurrency protection. A stale write is rejected, preserving the current database version. Generated Lean files are owned by the service.

Lean Server elaborates changed documents incrementally. Native Lake builds imported dependencies and, on explicit build requests, the target `.olean`. Source edits and checks are separate versioned operations; a successful check cannot certify a newer dependency state that changed while checking.
