---
title: Release checks
---

Authoring is through the local browser and service API. Install the executable independently of its source checkout.

Release validation builds the frontend assets embedded into the Rust binary, checks Svelte and Rust, and runs database/browser/Lean integration tests. Keep the TerminusDB image and Lean toolchain pinned in the deployment/test configuration.

Database revisions are independent of source-code Git commits. Every graph mutation creates a TerminusDB commit; `mdc --proj myproject/main history` and `mdc --proj myproject/main branch create NAME` expose basic history and branching. Native TerminusDB provides merge/rebase operations; no merge UI is included.
