---
title: Release checks
---

The release check (`.github/workflows/release-check.yml`) runs on every push and
pull request:

1. Fail if generated files or deployment state (`dist/`, `node_modules/`,
   `mathdoc.env`, `secrets/`) are committed; install the workspace; type-check
   the backend; check and unit-test the frontend; build backend and frontend.
2. Check and build this documentation site.
3. Test the LaTeX renderer with its pinned Python packages.
4. Start TerminusDB and PostgreSQL from `compose.yaml` and run the backend tests
   and the browser suite against them. Tests that need LeanGround are skipped.
5. Build the Docker image and run `tests/docker-smoke.py` against an exported deployment.

All five run through `scripts/check`: the `release-check` job runs
`./scripts/check native` (steps 1–4; `native` includes `fast`) and uploads
`browser-diagnostics` on failure, the `container-smoke` job runs
`./scripts/check docker` (step 5). Publishing a GitHub release `vX.Y.Z` matching
the root `package.json` version runs **Publish runtime**
(`runtime-release.yml`): it repeats the container checks, pushes the image to
GHCR and attaches the deployment bundle, whose `compose.yaml` pins the image by
digest. Run by hand, it publishes a `sha-…` image and keeps the bundle as a
workflow artifact.

Two more workflows: **Frontend performance** (`frontend-perf.yml`) compares pull
requests that touch `web/` with their base
([Performance measurements](../performance/#frontend-performance)), and
**Deploy documentation** (`docs-deploy.yml`) publishes this site to GitHub Pages
on pushes to `main` that touch `docs/` or `web/src/design.css`.

The Compose files (`compose.yaml`, `compose.server.yaml`) pin TerminusDB by
digest and PostgreSQL only by the tag `postgres:17-bookworm`. The commands to run
the same checks locally are in [Development setup](../setup/).

Database history is independent of Git: every document write is a TerminusDB
commit. `mdc history` and `mdc branch new` expose basic history and branching;
merges and rebases are done in TerminusDB directly.

Upgrading a deployment means replacing its `compose.yaml` and `mdc` with those of
the new release's bundle, then `docker compose pull` and `up`
([Server deployment](../../getting-started/server-deployment/#restarts-and-upgrades));
a source checkout is rebuilt and restarted
([Installation](../../getting-started/installation/#upgrade)). `migrate` runs
first and is idempotent. Save drafts before upgrading and reload open browser
tabs afterwards.
