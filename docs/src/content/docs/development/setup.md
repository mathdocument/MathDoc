---
title: Development setup
---

Use the [installation instructions](../../getting-started/installation/) for a
local TerminusDB instance and pinned Lean v4.33.1 toolchain. Node.js is required
to build from source and develop the frontend/documentation, but not to run the
installed binary. The build and CI use Node.js 26.

## Build and fast checks

The shared local/CI entry point is `./scripts/check`. Install Node.js 26, stable
Rust (including rustfmt and clippy), Elan, Docker Compose, and Python with
`src/latex/requirements.txt` first. Set `MDC_LATEX_PYTHON` to your Python virtual
environment's executable when using one.

```sh
./scripts/check fast    # frontend, docs, Rust and Python; no running database
./scripts/check native  # fast checks plus native Lean, database and browser tests
./scripts/check docker  # build the runtime and test a source-free deployment
./scripts/check         # both CI suites; use before pushing
```

`native` installs the pinned Lean toolchain and Playwright Chromium if missing,
then creates its own database with a random loopback port, password and volume.
Fixtures run serially because lifecycle tests inspect the server-wide database
registry; their explicit concurrent-request checks still run concurrently.
It removes that database and volume on exit and does not reuse your development
or server database. On minimal Linux systems, install Chromium's system libraries
once with `npm exec --prefix web -- playwright install-deps chromium`.
GitHub's Linux jobs call these same `native` and `docker` commands; macOS Rust
tests additionally run on GitHub. Network outages and runner failures can still
cause remote failures after a successful local run.

For individual checks during development:

```sh
npm --prefix web ci
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
python3 -m venv /tmp/mdc-latex-venv
/tmp/mdc-latex-venv/bin/pip install -r src/latex/requirements.txt
/tmp/mdc-latex-venv/bin/python -B -m unittest discover -s src/latex -p 'test_*.py'
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
```

`web/dist` is generated locally and ignored by Git. Build it before running Cargo
in a fresh checkout; Cargo embeds those assets into the Rust binary. Rebuild it
when frontend sources or dependencies change. Backend-only changes can reuse
existing assets. CI and Docker build the frontend from source. See
[Browser frontend](../web-frontend/) for the Vite proxy.

Commit sources, dependency lockfiles, static source assets and intentional
benchmark baselines/reports. Build output (`web/dist`, `docs/dist`, `target`),
installed dependencies, Python bytecode and transient test output stay local.

## Native integration checks

Make the test TerminusDB endpoint/user/password available through
`MDC_TERMINUS_URL`, `MDC_TERMINUS_USER` and `MDC_TERMINUS_PASSWORD`. Install
Playwright Chromium once with `npm exec --prefix web -- playwright install
--no-shell chromium`.

```sh
cargo test --locked --lib --test test_database --test test_service --test test_lean_service --test test_latex_service --test test_status -- --ignored --nocapture --test-threads=1
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
