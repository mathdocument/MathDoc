---
title: Release checks
---

The MathDoc VS Code editing extension has been removed. Authoring is through the local browser and service API.

Release validation builds the frontend assets embedded into the Rust binary, checks Svelte and Rust, and runs database/browser/Lean integration tests. Keep the TerminusDB image and Lean toolchain pinned in the deployment/test configuration.

Database revisions are independent of source-code Git commits. Every graph mutation creates a TerminusDB commit; `mdc history` and `mdc branch create NAME` expose basic history and branching. Native TerminusDB provides merge/rebase operations; no merge UI is included.
