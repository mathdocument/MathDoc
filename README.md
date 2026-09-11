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
# cache_dir = "/absolute/path/to/cache"
# lean_timeout_seconds = 300
```

`MDC_CONFIG` selects another config file. Environment overrides are
`MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_TERMINUS_PASSWORD`,
`MDC_CACHE_DIR` and `MDC_LEAN_TIMEOUT_SECONDS`. No per-project launch script,
local executable, `.env`, or working directory is required.

```sh
mdc init myproject                    # once: creates this project's database
mdc start myproject/main              # background service on an available port
mdc status                           # see project branches and running ports
```

Open the URL printed by `start`. Use `--port 7600` to request a specific port;
an occupied port is an error. `init` requires a running TerminusDB instance; it creates
a new database, schema and default Lean settings. It does not initialize Docker,
reset existing projects, or create a file workspace. `start` returns when the
service is ready; it keeps running after the terminal closes. `mdc stop myproject/main`
waits for the HTTP service and Lean workers to shut down, retaining database data
and caches. This deployment is for trusted local authors:
Lean metaprograms execute as the service user. Bind and browser-origin checks
restrict the HTTP service to the local machine.

## Projects, branches and storage

`mdc status` lists all MathDoc project branches in the configured TerminusDB:

```json
{
  "etp/main": { "port": 7600 },
  "mdocs/main": { "port": null }
}
```

The JSON object maps `DATABASE/BRANCH` to a status object. Its `port` is `null` when no service owns
the branch's cache with a registered listener. Status uses the configured database
credentials and cache directory. Stopped or crashed services do not leave a stale
running port in the output. An empty inventory produces `{}`.

Each project uses a separate database in the shared TerminusDB instance. Each
HTTP service serves one existing branch. There is no implicit client target:
client commands require `--proj DATABASE/BRANCH` and find that service's local port
without contacting TerminusDB. This option belongs to the client command and must
follow it; the CLI root and management commands do not accept it. Start/stop take
the project as a positional argument;
init takes a database name, and status lists all projects.

```sh
mdc start other/main --port 7600
mdc graph check --proj other/main
mdc branch new --proj myproject/main agent
mdc start myproject/agent
mdc stop myproject/agent
```

Branch creation forks the selected service's current branch head and leaves that
service on its original branch. Missing branches and duplicate starts are errors.

Caches live under `$XDG_CACHE_HOME/mdc` or `~/.cache/mdc`, separated by database
endpoint, database name, branch and Lean environment. `start` creates them.
They contain generated sources, Lake artifacts, check certificates and temporary
browser drafts. The branch cache also holds `service.log` and a private service
record. Stop the service before removing its cache. Durable graph/source
history lives in TerminusDB's volume; a cache directory is not a database backup.
The installed executable is separate from this cache.

## Authoring and checks

```sh
mdc new --proj myproject/main -t 'My theorem'
printf 'theorem exampleA : True := by trivial\n' | mdc edit --proj myproject/main 'My theorem' --type lean
mdc lean check --proj myproject/main 'My theorem'
mdc lean check --proj myproject/main 'My theorem' --build
mdc lean goals --proj myproject/main 'My theorem' --line 0 --column 31
mdc show --proj myproject/main 'My theorem'
mdc dep add --proj myproject/main 'My theorem' --target 'Earlier lemma'
mdc graph check --proj myproject/main
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
dialog or `mdc project show --proj myproject/main` / `mdc project set --proj myproject/main` (JSON on stdin). Libraries are not
limited to Mathlib; Git dependencies must be pinned to full commits. Title/text
edits do not invalidate Lean results. Ordinary commands never scan source files.

## Export and restore

```sh
mdc export --proj myproject/main > backup.json
mdc export --proj myproject/main 'My theorem' > theorem.json
mdc import --proj other/main backup.json
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
cargo test --locked --test test_database --test test_service --test test_lean_service --test test_status -- --ignored --nocapture
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
