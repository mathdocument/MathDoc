---
title: Development setup
---

Set up the databases as in [Installation](../../getting-started/installation/).
The repository is one npm workspace: `coordinator/` (backend, worker, CLI),
`web/` (editor), `app/` (archived collaboration prototype). `renderer/` is the
Python LaTeX renderer, `docs/` this site, `perf/` benchmarks. CI uses Node.js 26.

## Build and fast checks

```sh
npm ci
npm run check                     # coordinator and app type checks
npm run check -w mdc-web          # svelte-check
npm test -w mdc-web               # frontend unit tests
npm run build -w @mathdoc/coordinator && npm run build -w mdc-web

python3 -m venv /tmp/mdc-latex-venv
/tmp/mdc-latex-venv/bin/pip install -r renderer/requirements.txt
/tmp/mdc-latex-venv/bin/python -B -m unittest discover -s renderer -p 'test_*.py'
```

`coordinator/dist`, `web/dist` and `docs/dist` are generated and ignored by Git;
CI fails if they are committed. Commit sources, lockfiles and intentional
benchmark baselines. `npm run format` applies Prettier to the backend and the
prototype.

During development, `npx tsx coordinator/src/main.ts serve` (and `worker`,
`migrate`) runs the backend from source without building.

## Backend tests

```sh
docker compose up -d
MDC_TEST_DATABASE_URL=postgresql://mathdoc:PASSWORD@127.0.0.1:5432/mathdoc \
MDC_TEST_TERMINUS_URL=http://127.0.0.1:6363 \
MDC_TEST_TERMINUS_PASSWORD=PASSWORD \
  npm test
```

`npm test` runs `coordinator/test/*.test.ts`. Unit tests (domain, contracts,
scanner) need nothing; tests with real PostgreSQL and TerminusDB run when the
variables above are set and are skipped otherwise. They create disposable
databases; never point them at production data.

Tests that need a real LeanGround (the full collaboration scenario, the CLI test
and the Mathlib conversion test) additionally need `MDC_TEST_LEANGROUND_URL` and
`MDC_TEST_LEANGROUND_TOKEN`; the Mathlib test also needs
`MDC_TEST_MATHLIB_BASE_KEY` naming a Mathlib base. The LeanGround server must
meet the [requirements](../../getting-started/installation/#requirements), in
particular a worker built at or after definition support. The scenario takes
about half a minute, most of it waiting for a lease to expire. LeanGround keeps
its data between runs, so rerunning with a fixed idempotency key would reuse
earlier projects; the tests generate fresh keys.

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
(`MDC_E2E_BROWSER=webkit` for WebKit). Without `MDC_LATEX_PYTHON` it points the
renderer at a nonexistent path rather than installing it, and the LaTeX cases
fail. LeanGround-dependent cases need `LEANGROUND_SERVER_URL`,
`LEANGROUND_FACT_TOKEN` and `LEANGROUND_ACTOR`; by default the worker is pointed at
an unreachable address.

Failures keep a screenshot, a Playwright `trace.zip`, the service log and
diagnostics in a temporary directory printed by the test; `MDC_E2E_ARTIFACTS`
chooses the parent directory. CI uploads them as `browser-diagnostics`. Open a
trace with `npx playwright show-trace /path/to/trace.zip`.

## Container

```sh
docker build -t mathdoc-runtime:local .
python3 tests/docker-smoke.py
```

See [Server deployment](../../getting-started/server-deployment/#verification).

## Documentation

```sh
npm --prefix docs ci
npm --prefix docs run check
npm --prefix docs run build
```

Usage belongs in these pages; keep the READMEs short and link here.
Design and migration records are in `docs/*.md` outside the site.
