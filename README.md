# MathDoc

MathDoc is a locally hosted, versioned graph of mathematical knowledge. TerminusDB
stores nodes, dependencies, source blocks and Lean project settings. The browser
and `mdc` CLI use one HTTP service. Blocks support **text, Lean, Rocq and LaTeX**;
Lean has native Monaco editing, Infoview and incremental Lean Server checks.

## Install and run

Install Rust, Docker and Elan. Install one executable, independent of the checkout:

```sh
cargo install --path . --locked
elan toolchain install leanprover/lean4:v4.33.1
```

For a new TerminusDB instance, choose a private password in `MDC_TERMINUS_PASSWORD`
and run `docker compose up -d` using this repository's `compose.yaml`. The pinned
container exposes port 6363 on loopback and stores data in the Docker volume
`mathdoc-terminus-data`. An existing instance and volume must be reused, not reset.

Save service settings in `~/.config/mdc/config.toml` (mode 600), or
`$XDG_CONFIG_HOME/mdc/config.toml`:

```toml
terminus_url = "http://127.0.0.1:6363"
terminus_user = "admin"
terminus_password = "the-existing-database-password"
url = "http://127.0.0.1:7599"
# cache_dir = "/absolute/path/to/cache"
# lean_timeout_seconds = 300
```

`MDC_CONFIG` selects another config file. Environment overrides are
`MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_TERMINUS_PASSWORD`, `MDC_URL`,
`MDC_CACHE_DIR` and `MDC_LEAN_TIMEOUT_SECONDS`. No per-project launch script,
local executable, `.env`, or working directory is required.

```sh
mdc init myproject                    # once: creates this project's database
mdc serve myproject                   # start its HTTP service
```

Open http://127.0.0.1:7599. `init` requires a running TerminusDB instance; it creates
a new database, schema and default Lean settings. It does not initialize Docker,
reset existing projects, or create a file workspace. `serve` must remain running
while the browser and CLI use it. This deployment is for trusted local authors:
Lean metaprograms execute as the service user. Bind and browser-origin checks
restrict the HTTP service to the local machine.

## Projects, branches and storage

Each project uses a separate database in the shared TerminusDB instance. Each
HTTP service serves one branch. For example, `mdc serve other --bind 127.0.0.1:7600`
serves another project; `mdc --url http://127.0.0.1:7600 graph check` selects it as
a client. `mdc branch create agent` creates a branch in the current project; serve
it with `mdc serve myproject --branch agent --bind 127.0.0.1:7601`.

Caches live under `$XDG_CACHE_HOME/mdc` or `~/.cache/mdc`, separated by database
endpoint, database name, branch and Lean environment. `serve` creates them.
They contain generated sources, Lake artifacts, check certificates and temporary
browser drafts. Stop the service before removing its cache. Durable graph/source
history lives in TerminusDB's volume; a cache directory is not a database backup.
The installed executable is separate from this cache.

## Authoring and checks

```sh
mdc new -t 'My theorem'
printf 'theorem exampleA : True := by trivial\n' | mdc edit 'My theorem' --type lean
mdc lean check 'My theorem'
mdc lean check 'My theorem' --build
mdc lean goals 'My theorem' --line 0 --column 31
mdc show 'My theorem'
mdc dep add 'My theorem' --target 'Earlier lemma'
mdc graph check
```

References accept exact names or complete UUIDs; duplicate names require a UUID.
`edit` reads a complete block from stdin. Agents should pass `--revision` from
`show` when editing/checking a previously read node. Browser mutations use the
same guards. Stale writes fail; cycles and missing dependencies are rejected.

Checks wait for diagnostics for the requested source version. `passed` means no
Lean errors. `certified` also requires direct managed `Lib.*` imports to equal
`dep`, with every dependency certified for its current inputs. `--build` produces
the target `.olean`; imported dependencies are built as needed. Lean's `sorry`
warnings remain warnings. Only Lean compilation is integrated.

Each browser tab has an isolated draft environment. Switching nodes preserves
one Lean connection and the two most recent document workers. Evicted workers
must reload imports. Proof edits reuse Lean snapshots; changed dependencies
reload the affected environment. Saved check results and live editor progress
are displayed separately. Large cold imports still cost time.

Configure toolchain, Lake TOML and the complete lock manifest in the Lean project
dialog or `mdc project show` / `mdc project set` (JSON on stdin). Libraries are not
limited to Mathlib; Git dependencies must be pinned to full commits. Title/text
edits do not invalidate Lean results. Ordinary commands never scan source files.

## Export and restore

```sh
mdc export > backup.json
mdc export 'My theorem' > theorem.json
mdc --url http://127.0.0.1:7600 import backup.json
```

Full JSON bundles contain all current nodes, module identities and Lean project
settings. A single-node bundle omits project settings and requires its dependencies
to exist when imported. Import never overwrites existing UUIDs; project settings
can only be imported into an empty database. JSON export is a snapshot, not the
full commit history: back up the TerminusDB volume to preserve history/branches.
Legacy `.mdoc` conversion is an external, read-only migration utility; the runtime
has no legacy parser, sync/back commands, path references or workspace scanner.

## Development

```sh
npm --prefix web ci
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
cargo test --locked
cargo build --locked
cargo test --locked --test test_database --test test_service --test test_lean_service -- --ignored --nocapture
npm --prefix web run test:e2e
npm --prefix docs run check
npm --prefix docs run build
```

The browser assets in `web/dist` are embedded in the binary and committed with
frontend changes. Native integration tests require TerminusDB, its credentials,
Lean v4.33.1 and Playwright Chromium. Test databases and compiler caches are
isolated from production and removed after the tests. Do not run a project service
or import project data into the source checkout. Graph benchmarks are described
in [perf/README.md](perf/README.md); they do not compile the node graph.
