---
title: Development setup
---

Set up the databases as in [Installation](../../getting-started/installation/).
The repository is one npm workspace: `coordinator/` (backend, worker, CLI) and
`web/` (editor). `renderer/` is the Python LaTeX renderer, `docs/` this site,
`perf/` backend benchmarks, `scripts/` the check and deployment scripts, `tests/`
the container test. CI uses Node.js 26.

## One command

`scripts/check` runs the same checks as CI: `fast` (type checks, unit tests, builds,
this site, the LaTeX renderer), `native` (`fast`, then backend and browser tests
against disposable TerminusDB and PostgreSQL containers from `compose.yaml` on free
ports, one test file at a time), `docker` (image and exported deployment), or
`all` (default). Set `MDC_LATEX_PYTHON` to a Python with the renderer's packages
(default `python3`). `native` drops every other `MDC_*`, `MATHDOC_*` and
`COMPOSE_*` variable, so it never reaches a running deployment; it keeps
`MDC_TEST_LEANGROUND_*`, `MDC_TEST_MATHLIB_*`, `MDC_E2E_*` and `LEANGROUND_*`, so
the LeanGround tests below run when those are set.

## Build and fast checks

```sh
npm ci
npm run check                     # coordinator tsc and web svelte-check
npm test -w mdc-web               # frontend unit tests (vitest, web/src/lib/*.test.ts)
npm run build -w @mathdoc/coordinator && npm run build -w mdc-web

python3 -m venv /tmp/mdc-latex-venv
/tmp/mdc-latex-venv/bin/pip install -r renderer/requirements.txt
/tmp/mdc-latex-venv/bin/python -B -m unittest discover -s renderer -p 'test_*.py'
```

`coordinator/dist`, `web/dist` and `docs/dist` are generated and ignored by Git;
CI fails if they, `node_modules`, `mathdoc.env` or `secrets/` are committed.
Commit sources, lockfiles and intentional benchmark baselines. `npm run format`
applies Prettier to the backend.

During development, `npx tsx coordinator/src/main.ts serve` (and `worker`,
`migrate`) runs the backend from source without building;
`npx tsx coordinator/src/cli.ts` runs `mdc` the same way.

## Backend tests

```sh
docker compose up -d
MDC_TEST_DATABASE_URL=postgresql://mathdoc:PASSWORD@127.0.0.1:5432/mathdoc \
MDC_TEST_TERMINUS_URL=http://127.0.0.1:6363 \
MDC_TEST_TERMINUS_PASSWORD=PASSWORD \
  npm test
```

`npm test` runs `coordinator/test/*.test.ts` with Node's test runner;
`npx tsx --test coordinator/test/NAME.test.ts` runs one file. `scripts/check
native` runs the files one at a time (`--test-concurrency=1`) because they share
one TerminusDB server. A test whose services are not configured is skipped.

| File | Needs | Covers |
| --- | --- | --- |
| `domain`, `lean-source`, `contracts`, `config`, `goal-file`, `documents` | nothing | Board transitions, the Lean scanner, contract fixtures and generated schemas, secrets, goal files, the TerminusDB node adapter (against a local stand-in). |
| `docs` | nothing; the last test PostgreSQL and TerminusDB | Names, Lake and LaTeX project rules, the TOML subset, snapshots, the LaTeX bridge with a fake renderer; then roles, revisions and conflicts of the document API. |
| `robustness` | nothing; the third test PostgreSQL, the fourth also TerminusDB | LeanGround timeouts, resent TerminusDB rollbacks, job backoff, a proof submitted while LeanGround is down. |
| `integration` | PostgreSQL | Concurrent claims, idempotency, job fencing, revision guards, event cursor; LeanGround is an HTTP stand-in. |
| `scale` | PostgreSQL; the sync test also TerminusDB | The lease path, budget and expiry, migration of older boards, syncs that skip unchanged branches. |
| `project` | PostgreSQL, TerminusDB | Project boards with branches as routes. |
| `verbs`, `identity`, `refute`, `maintain` | PostgreSQL, TerminusDB; the second test of each a real LeanGround | `pull`/`split`/`prove`/`done` and replays; delegated tokens and `mdc mcp`; refutation and `next`; maintainer commands and their replays. |
| `cli` | PostgreSQL, TerminusDB and the LeanGround variables (it makes no LeanGround call) | `mdc` document commands. |
| `scenario` | PostgreSQL, TerminusDB, a real LeanGround | The full collaboration scenario. |
| `mathlib` | as `scenario`, plus `MDC_TEST_MATHLIB_BASE_KEY` naming a Mathlib base | Node conversion on Mathlib. |

PostgreSQL is `MDC_TEST_DATABASE_URL`; TerminusDB is `MDC_TEST_TERMINUS_URL` and
`MDC_TEST_TERMINUS_PASSWORD`. Each test deletes the TerminusDB databases it
creates (`removeDatabases` in `coordinator/test/harness.ts`); when the test
TerminusDB is the one a local server uses, a database left behind is listed
there as a project. Without a real LeanGround, tests use the contract
double in `coordinator/test/leanground-double.ts`, which checks shapes, not proofs.
A real LeanGround needs `MDC_TEST_LEANGROUND_URL` and `MDC_TEST_LEANGROUND_TOKEN`,
and `MDC_TEST_LEANGROUND_ACTOR` when the token's LeanGround actor is not
`coordinator` (the coordinator registers that actor in every LeanGround project);
these tests use base key 1. The LeanGround server must meet the
[requirements](../../getting-started/installation/#requirements), in particular a
worker built at or after definition support. The scenario takes about half a
minute, most of it waiting for a lease to expire. LeanGround keeps its data
between runs, so rerunning with a fixed idempotency key would reuse earlier
projects; the tests generate fresh keys.

Each test creates its own PostgreSQL schema and TerminusDB database; never point
them at production data. `maintain` gives each identity its own temporary
`MDC_STATE_DIR`, so the tests never write replay records to `~/.local/state/mdc`.

## Browser tests

```sh
npm exec -w mdc-web -- playwright install --no-shell chromium
MDC_DATABASE_URL=postgresql://mathdoc:PASSWORD@127.0.0.1:5432/mathdoc \
MDC_TERMINUS_URL=http://127.0.0.1:6363 MDC_TERMINUS_PASSWORD=PASSWORD \
MDC_LATEX_PYTHON=/tmp/mdc-latex-venv/bin/python \
  npm run test:e2e -w mdc-web
```

`web/e2e/run.mjs` builds nothing: build `web/dist` first. It runs `migrate`,
`serve` and `worker` from source with a generated token, then drives Chromium
(`MDC_E2E_BROWSER=webkit` or `firefox` for the other engines). Without `MDC_LATEX_PYTHON` it points the
renderer at a nonexistent path rather than installing it, and the LaTeX cases
fail. The LeanGround case runs only with `LEANGROUND_SERVER_URL`, plus
`LEANGROUND_FACT_TOKEN` and `LEANGROUND_ACTOR` (default `coordinator`); its base
key is `MDC_E2E_BASE_KEY` (default 1) and must contain `Nat`. Otherwise the worker
is pointed at an unreachable address.

Failures keep a screenshot, a Playwright `trace.zip`, the service log and
diagnostics in a temporary directory printed by the test; `MDC_E2E_ARTIFACTS`
chooses the parent directory. CI uploads them as `browser-diagnostics`. Open a
trace with `npx playwright show-trace /path/to/trace.zip`.

## Container

```sh
./scripts/check docker
```

This builds `mathdoc-runtime:local` and runs `tests/docker-smoke.py` on a
deployment exported by `scripts/package-deployment`. With
`MDC_SMOKE_LEANGROUND_URL` (as seen from the containers),
`MDC_SMOKE_LEANGROUND_TOKEN` and `MDC_SMOKE_LEANGROUND_ACTOR` set, an agent also
proves a theorem through the deployment (`MDC_SMOKE_BASE_KEY`, default 1, must
contain `Nat`). On macOS with colima, run it with `TMPDIR` under your home
directory.

See [Server deployment](../../getting-started/server-deployment/#verification).

## Documentation

```sh
npm --prefix docs ci
npm --prefix docs run check
npm --prefix docs run build
```

Usage belongs in these pages; keep the READMEs short and link here.
The internal plan, progress and technical notes are in `docs/mathdoc.md`
(Chinese), outside the site.
