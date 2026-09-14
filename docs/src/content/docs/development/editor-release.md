---
title: Release checks
---

Authoring is through the local browser and service API. Install the executable independently of its source checkout.

Release validation builds the frontend assets embedded into the Rust binary, checks Svelte and Rust, and runs database/browser/Lean integration tests. Keep the TerminusDB image and Lean toolchain pinned in the deployment/test configuration.

Database revisions are independent of source-code Git commits. Every graph mutation creates a TerminusDB commit; `mdc history -p myproject/main` and `mdc branch new NAME -p myproject/main` expose basic history and branching. Merge/rebase administration uses TerminusDB directly; mdc has no merge UI or command.

Use the checks in [Development setup](../setup/), including the CLI lifecycle and
real-browser tests. A binary upgrade does not replace already running services:
save browser drafts, run `mdc stop`, then run `mdc start DATABASE/BRANCH` for each
branch you want loaded. One server restart updates all backend code and embedded
assets. Reload the browser. Project URLs remain stable while the shared port
stays unchanged; branches have no internal HTTP ports.
