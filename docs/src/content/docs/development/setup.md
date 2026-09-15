---
title: Development setup
---

Use the [installation instructions](../../getting-started/installation/) for a
local TerminusDB instance and pinned Lean v4.33.1 toolchain. Node.js is required
for frontend/documentation development, but not for the installed binary.

## Build and fast checks

```sh
npm --prefix web ci
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
python3 -m venv /tmp/mdc-latex-venv
/tmp/mdc-latex-venv/bin/pip install -r src/latex/requirements.txt
/tmp/mdc-latex-venv/bin/python -B -m unittest discover -s src/latex -p 'test_*.py'
cargo test --locked
cargo build --locked
```

`web/dist` is committed and embedded into the Rust binary. Rebuild and commit
assets when frontend sources change; CI rejects drift. Backend-only changes can
use existing assets. See [Browser frontend](../web-frontend/) for the Vite proxy.

## Native integration checks

Make the test TerminusDB endpoint/user/password available through
`MDC_TERMINUS_URL`, `MDC_TERMINUS_USER` and `MDC_TERMINUS_PASSWORD`. Install
Playwright Chromium once with `npm exec --prefix web -- playwright install
--no-shell chromium`.

```sh
cargo test --locked --test test_database --test test_service --test test_lean_service --test test_latex_service --test test_status -- --ignored --nocapture
MDC_LATEX_PYTHON=/tmp/mdc-latex-venv/bin/python npm --prefix web run test:e2e
```

These tests create disposable databases, project files and Lean caches in system
temporary directories outside the source checkout and remove them afterwards.
Do not use a production database or start a test project inside the checkout.
`test_status` covers process lifecycle, branch deletion and CLI transactions;
`test_service` and `test_lean_service` exercise real Lean and database behavior.
The browser suite uses the actual backend and native editor, including draft
preservation, revision conflicts, certification reuse and process cleanup. The
LaTeX case exercises uploads, scoped completions, macro expansion, bibliography,
draft previews and cross-node navigation in Chromium or WebKit
(`MDC_E2E_BROWSER=webkit`).
`MDC_BIN` optionally selects a prebuilt binary for browser tests. The Lean cases
also exercise bidirectional backpressure, superseded selections and a real
30-second preparation timeout. Error checks require native diagnostics for the
edited document version before asserting that Monaco displays the marker.

Browser failures retain a screenshot, Playwright `trace.zip`, `service.log` and
`diagnostics.json` (recent LSP traffic, document versions and editor viewport
state). The test prints their directory; it defaults to a separate system temp
directory that survives fixture cleanup. Set `MDC_E2E_ARTIFACTS` to choose the
parent directory. Release CI uploads these files as `browser-diagnostics` on
failure. Inspect a trace with `npx playwright show-trace /path/to/trace.zip`.

The optional `test_service_scale` target measures real graph requests on 47,435
nodes and 368,017 edges without compiling that graph. It is a performance test,
not part of the normal correctness loop. See [Performance measurements](../performance/)
for read-only measurements and isolated write benchmarks.

## Documentation

```sh
npm --prefix docs ci
npm --prefix docs run check
npm --prefix docs run build
```

Detailed usage belongs in these documentation pages. Keep root and frontend
READMEs short and link here instead of duplicating command instructions.
