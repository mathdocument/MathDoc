---
title: Release checks
---

The release check (`.github/workflows/release-check.yml`) runs on every push and
pull request:

1. Install the workspace; type-check the backend and prototype; check and unit-test
   the frontend; build backend and frontend; verify no generated files are
   committed.
2. Check and build this documentation site.
3. Test the LaTeX renderer with its pinned Python packages.
4. Start TerminusDB and PostgreSQL from `compose.yaml` and run the backend tests
   and the browser suite against them. Tests that need LeanGround are skipped.
5. Build the Docker image and run `tests/docker-smoke.py`.

Keep the TerminusDB and PostgreSQL images pinned in the Compose files. The
commands to run the same checks locally are in [Development setup](../setup/).

Database history is independent of Git: every document write is a TerminusDB
commit. `mdc history` and `mdc branch new` expose basic history and branching;
merges and rebases are done in TerminusDB directly.

Upgrading a deployment means rebuilding and restarting the API and worker
([Server deployment](../../getting-started/server-deployment/#restarts-and-upgrades));
`migrate` runs first and is idempotent. Reload open browser tabs after saving drafts.
