---
title: Release checks
---

Authoring is through the local browser and service API. Install the executable independently of its source checkout.

Release validation builds the frontend assets embedded into the Rust binary, checks Svelte and Rust, and runs database/browser/Lean integration tests. Keep the TerminusDB image and Lean toolchain pinned in the deployment/test configuration.

Database revisions are independent of source-code Git commits. Every graph mutation creates a TerminusDB commit; `mdc history -p myproject/main` and `mdc branch new NAME -p myproject/main` expose basic history and branching. Merge/rebase administration uses TerminusDB directly; mdc has no merge UI or command.

Use the checks in [Development setup](../setup/), including the CLI lifecycle and
real-browser tests. A binary upgrade does not replace already running services:
save browser drafts, run `mdc stop DATABASE/BRANCH`, then `mdc start DATABASE/BRANCH`
and reload the browser. With automatic port selection, the new URL may differ.
